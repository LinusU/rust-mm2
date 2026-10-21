//! Remote-input prediction policy (NETPLAY-05A-2, `docs/netcode-redesign.md`).
//!
//! A client drives another human's car from the commands the host applied
//! to it ([`RemoteCommands`], NETPLAY-05A-1). When the command for the tick
//! being simulated has not arrived, a [`RemotePredictor`] decides what the
//! car is driven with. It is pure: it reads the store and returns a
//! [`TickCommand`], touching no body.
//!
//! # Contract
//!
//! - **Received commands are never altered.** A tick present in the store
//!   is returned exactly as stored ([`Basis::Authoritative`]).
//! - **A finalized command is consumed once.** The authoritative path goes
//!   through [`RemoteCommands::consume`]; a predicted tick consumes nothing
//!   and a command that shows up after its tick was predicted past is
//!   dropped by the store's cursor, never replayed here.
//! - **Nothing discrete is invented.** A prediction only repeats, or
//!   scales toward neutral, the analog controls of the newest command at
//!   or before the tick; `forced_gear` and `auto_reverse` are copied
//!   verbatim and no control is ever raised above what the driver sent
//!   (so no reset, horn or gear change, and no handbrake from nothing).
//! - **No body is touched**: no momentum damping or state nudging.
//!
//! Two policies exist so they can be measured against each other
//! (`tests/net_remote_predict.rs`): [`Policy::HoldLast`] repeats the newest
//! command unchanged however old it is; [`Policy::AgeAware`] repeats it for
//! `hold` ticks of age and then fades the analog controls linearly to
//! neutral over `fade` ticks.

use std::collections::BTreeMap;

use bevy::prelude::*;
use mm2_game::Session;
use mm2_net::{MAX_REMOTE_RUN, RemoteCommands, TickCommand};
use mm2_vehicle::VehicleInput;

use crate::netdrive::{PredictedRemoteBody, decode_tick_command};
use crate::netplay_timeline::NetplayTimeline;

/// How a missing tick is filled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// Repeat the newest known command unchanged, however old.
    HoldLast,
    /// Repeat for `hold` ticks of age, then scale the analog controls
    /// linearly to neutral over the next `fade` ticks.
    AgeAware {
        /// Age in ticks up to which the command is repeated unchanged.
        hold: u32,
        /// Ticks over which the controls then reach neutral (`>= 1`).
        fade: u32,
    },
}

/// What a [`RemotePredictor::command_for`] result rests on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Basis {
    /// The tick's own command, exactly as the host applied it.
    Authoritative,
    /// Extrapolated from the command `age` ticks older than the tick.
    Predicted {
        /// `tick - source_tick`, at least 1.
        age: u64,
    },
    /// No command is known for the seat: neutral controls.
    Neutral,
}

/// A tick's command and where it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Predicted {
    /// The controls to drive the remote car with.
    pub command: TickCommand,
    /// Whether they were received, extrapolated or absent.
    pub basis: Basis,
}

/// The neutral controls (nothing pressed, centred, automatic gearbox).
pub const NEUTRAL: TickCommand = TickCommand {
    throttle: 0,
    brake: 0,
    steer: 0,
    handbrake: 0,
    auto_reverse: false,
    forced_gear: None,
};

/// Predictor. Holds the policy and, per seat, the last command it consumed:
/// [`RemoteCommands::consume`] drops a tick once it is consumed, so without
/// this a missing tick right after an authoritative one would find nothing
/// to repeat and fall to neutral.
#[derive(Debug, Clone)]
pub struct RemotePredictor {
    /// The fill policy for missing ticks.
    pub policy: Policy,
    consumed: BTreeMap<u16, (u64, TickCommand)>,
}

impl RemotePredictor {
    /// A predictor using `policy`.
    pub fn new(policy: Policy) -> Self {
        Self {
            policy,
            consumed: BTreeMap::new(),
        }
    }

    /// The controls for `player` on authoritative tick `tick`. Consumes the
    /// tick's command from `store` when present; otherwise extrapolates
    /// from the newest stored command at or before `tick`.
    pub fn command_for(&mut self, store: &mut RemoteCommands, player: u16, tick: u64) -> Predicted {
        if let Some(c) = store.consume(player, tick) {
            self.consumed.insert(player, (tick, c.command));
            return Predicted {
                command: c.command,
                basis: Basis::Authoritative,
            };
        }
        let stored = newest_at_or_before(store, player, tick);
        let remembered = self
            .consumed
            .get(&player)
            .copied()
            .filter(|(t, _)| *t < tick);
        match stored.into_iter().chain(remembered).max_by_key(|(t, _)| *t) {
            Some((at, command)) => Predicted {
                command: extrapolate(self.policy, command, tick - at),
                basis: Basis::Predicted { age: tick - at },
            },
            None => Predicted {
                command: NEUTRAL,
                basis: Basis::Neutral,
            },
        }
    }
}

/// The newest stored command at or before `tick`, looking back at most one
/// frame run (older ones would have been repeated in a newer frame).
fn newest_at_or_before(
    store: &RemoteCommands,
    player: u16,
    tick: u64,
) -> Option<(u64, TickCommand)> {
    let newest = store.newest_tick(player)?.min(tick);
    (newest.saturating_sub(MAX_REMOTE_RUN as u64 * 4)..=newest)
        .rev()
        .find_map(|t| store.get(player, t).map(|c| (t, c.command)))
}

/// `command` aged by `age` ticks under `policy`.
fn extrapolate(policy: Policy, command: TickCommand, age: u64) -> TickCommand {
    let Policy::AgeAware { hold, fade } = policy else {
        return command;
    };
    let past = age.saturating_sub(u64::from(hold));
    if past == 0 {
        return command;
    }
    // Remaining fraction in 0..=fade of `fade`.
    let left = u64::from(fade.max(1)).saturating_sub(past);
    let scale = |v: u32| (u64::from(v) * left / u64::from(fade.max(1))) as u32;
    TickCommand {
        throttle: scale(u32::from(command.throttle)) as u8,
        brake: scale(u32::from(command.brake)) as u8,
        steer: {
            let m = scale(u32::from(command.steer.unsigned_abs())) as i8;
            if command.steer < 0 { -m } else { m }
        },
        handbrake: scale(u32::from(command.handbrake)) as u8,
        ..command
    }
}

/// The client's store of the host's remote-seat commands and the
/// predictor that fills its gaps, one per client. Frames are absorbed
/// into `store` by the receive path (not wired yet); the contact bodies
/// below read it.
#[derive(Resource, Debug)]
pub struct RemoteFeed {
    /// Commands the host applied to remote seats, by seat and tick.
    pub store: RemoteCommands,
    /// The policy for ticks the store lacks: hold-last (the measured
    /// choice, see `docs/netcode-redesign.md`).
    pub predictor: RemotePredictor,
}

impl Default for RemoteFeed {
    fn default() -> Self {
        Self {
            store: RemoteCommands::new(),
            predictor: RemotePredictor::new(Policy::HoldLast),
        }
    }
}

/// Fixed-tick: give every predicted contact body exactly one command for
/// the step about to run, from [`RemoteFeed`]. The tick is the client's
/// fixed-tick count (`Session::tick`), the clock its own commands carry.
pub fn drive_predicted_bodies(
    session: Res<Session>,
    mut feed: ResMut<RemoteFeed>,
    mut bodies: Query<(&PredictedRemoteBody, &mut VehicleInput)>,
) {
    if !session.is_playing() {
        return;
    }
    let feed = &mut *feed;
    for (body, mut input) in &mut bodies {
        let p = feed
            .predictor
            .command_for(&mut feed.store, body.wire, session.tick());
        *input = decode_tick_command(&p.command);
    }
}

/// Register the contact-body driver behind the [`NetplayTimeline`]
/// selector, before the physics schedule steps the bodies.
pub fn add_systems(app: &mut App) {
    app.init_resource::<RemoteFeed>().add_systems(
        FixedUpdate,
        drive_predicted_bodies
            .run_if(|timeline: Option<Res<NetplayTimeline>>| timeline.is_some_and(|t| t.enabled)),
    );
}

#[cfg(test)]
mod tests {
    use mm2_net::{Message, RemoteSeat, Source};

    use super::*;

    fn cmd(throttle: u8, steer: i8) -> TickCommand {
        TickCommand {
            throttle,
            brake: 40,
            steer,
            handbrake: 0,
            auto_reverse: true,
            forced_gear: Some(2),
        }
    }

    fn store_with(first: u64, cmds: &[TickCommand]) -> RemoteCommands {
        let mut s = RemoteCommands::new();
        s.set_generation(Some(1));
        let seat = RemoteSeat {
            player: 3,
            epoch: 0,
            first_tick: first,
            commands: cmds
                .iter()
                .map(|c| mm2_net::RemoteCommand {
                    command: *c,
                    source: Source::Command,
                })
                .collect(),
        };
        let last = seat.last_tick();
        assert!(s.absorb(&Message::RemoteCommands {
            generation: 1,
            tick: last,
            seats: vec![seat],
        }));
        s
    }

    const AGE_AWARE: Policy = Policy::AgeAware { hold: 2, fade: 10 };

    #[test]
    fn a_present_tick_is_returned_unaltered_and_consumed_once() {
        for policy in [Policy::HoldLast, AGE_AWARE] {
            let mut s = store_with(5, &[cmd(200, -100), cmd(10, 3)]);
            let mut p = RemotePredictor::new(policy);
            let first = p.command_for(&mut s, 3, 5);
            assert_eq!(first.command, cmd(200, -100));
            assert_eq!(first.basis, Basis::Authoritative);
            // The same tick again is not a second consumption, and is not
            // answered from the past either.
            assert_eq!(p.command_for(&mut s, 3, 5).basis, Basis::Neutral);
            assert_eq!(s.stats(3).unwrap().consumed, 1);
        }
    }

    #[test]
    fn hold_last_repeats_the_newest_command_at_any_age() {
        let mut s = store_with(5, &[cmd(200, -100)]);
        let mut p = RemotePredictor::new(Policy::HoldLast);
        for tick in [6, 10, 50] {
            let got = p.command_for(&mut s, 3, tick);
            assert_eq!(got.command, cmd(200, -100));
            assert_eq!(got.basis, Basis::Predicted { age: tick - 5 });
        }
    }

    #[test]
    fn age_aware_holds_then_fades_to_neutral_without_inventing_anything() {
        let mut s = store_with(5, &[cmd(200, -100)]);
        let mut p = RemotePredictor::new(AGE_AWARE);
        assert_eq!(p.command_for(&mut s, 3, 7).command, cmd(200, -100));
        let mid = p.command_for(&mut s, 3, 12).command; // age 7: 5 of 10 left
        assert_eq!((mid.throttle, mid.brake, mid.steer), (100, 20, -50));
        let gone = p.command_for(&mut s, 3, 40).command;
        assert_eq!((gone.throttle, gone.brake, gone.steer), (0, 0, 0));
        // Discrete fields are copied, never changed.
        assert!(mid.auto_reverse && mid.forced_gear == Some(2));
        // Never above what was sent, at every age.
        for tick in 6..60 {
            let c = p.command_for(&mut s, 3, tick).command;
            assert!(c.throttle <= 200 && c.brake <= 40 && c.steer.unsigned_abs() <= 100);
            assert_eq!(c.handbrake, 0);
        }
    }

    #[test]
    fn a_missing_tick_after_a_consumed_one_repeats_it() {
        for policy in [Policy::HoldLast, AGE_AWARE] {
            let mut s = store_with(5, &[cmd(200, -100)]);
            let mut p = RemotePredictor::new(policy);
            assert_eq!(p.command_for(&mut s, 3, 5).basis, Basis::Authoritative);
            let next = p.command_for(&mut s, 3, 6);
            assert_eq!(next.command, cmd(200, -100));
            assert_eq!(next.basis, Basis::Predicted { age: 1 });
            assert_eq!(s.stats(3).unwrap().consumed, 1);
        }
    }

    #[test]
    fn no_known_command_is_neutral() {
        let mut s = RemoteCommands::new();
        let got = RemotePredictor::new(Policy::HoldLast).command_for(&mut s, 3, 9);
        assert_eq!((got.command, got.basis), (NEUTRAL, Basis::Neutral));
    }

    #[test]
    fn a_command_newer_than_the_tick_is_not_used() {
        let mut s = store_with(20, &[cmd(200, 0)]);
        let got = RemotePredictor::new(Policy::HoldLast).command_for(&mut s, 3, 10);
        assert_eq!(got.basis, Basis::Neutral);
    }
}
