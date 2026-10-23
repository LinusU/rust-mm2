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

use std::collections::{BTreeMap, VecDeque};

use bevy::prelude::*;
use mm2_game::{PlayerVehicle, Session};
use mm2_net::{
    MAX_REMOTE_RUN, Message, RemoteCommand, RemoteCommands, RemoteSeat, Source, TickCommand,
};
use mm2_vehicle::replay::MAX_REPLAY_STEPS;
use mm2_vehicle::{HumanDriver, VehicleInput};

use crate::net::{HostLink, LobbyState};
use crate::netcmd::tick_command;
use crate::netdrive::{
    NetPlayer, PredictedRemoteBody, RemotePick, ResetEpoch, decode_tick_command,
};
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
/// predictor that fills its gaps, one per client. `net::drain_lobby`
/// absorbs the host's `RemoteCommands` frames into `store`; the contact
/// bodies below read it.
#[derive(Resource, Debug)]
pub struct RemoteFeed {
    /// Commands the host applied to remote seats, by seat and tick.
    pub store: RemoteCommands,
    /// The policy for ticks the store lacks ([`LIVE_POLICY`]).
    pub predictor: RemotePredictor,
    /// The command each contact body was actually stepped with, by seat and
    /// client tick. [`RemoteCommands::consume`] forgets a tick once it is
    /// used, so a replay (NETPLAY-05A-4) re-reads its past from here.
    applied: BTreeMap<(u16, u64), TickCommand>,
    /// Host tick minus client tick for the same simulation instant, as the
    /// newest snapshot showed it: the host finalizes a seat's command `n`
    /// at its own tick `n + host_offset`, and the store is keyed by host
    /// tick. Set by the own-car reconcile (`netpredict::remote_drives`).
    host_offset: i64,
}

/// Ticks of applied commands kept per seat: past [`MAX_REPLAY_STEPS`]
/// nothing can be replayed, so older entries are dead weight.
const APPLIED_KEEP: u64 = MAX_REPLAY_STEPS as u64 * 2;

/// The policy the live contact-body step predicts with. Every live command
/// is older than the newest received frame (the client runs ahead of the
/// host), and a closed-loop driver's steering held that long shoves the
/// contact bodies into the own car, which the replay then reproduces each
/// snapshot; fading stale commands passes the three-car wall pileup that
/// pure hold-last fails (`docs/netcode-redesign.md`, "Live policy").
const LIVE_POLICY: Policy = Policy::AgeAware { hold: 4, fade: 12 };

impl Default for RemoteFeed {
    fn default() -> Self {
        Self {
            store: RemoteCommands::new(),
            predictor: RemotePredictor::new(LIVE_POLICY),
            applied: BTreeMap::new(),
            host_offset: 0,
        }
    }
}

impl RemoteFeed {
    /// Remember what `player`'s body was stepped with at `tick`, and drop
    /// that seat's entries older than the replay bound.
    pub fn note_applied(&mut self, player: u16, tick: u64, command: TickCommand) {
        self.applied.insert((player, tick), command);
        let floor = tick.saturating_sub(APPLIED_KEEP);
        self.applied.retain(|(p, t), _| *p != player || *t >= floor);
    }

    /// The host tick the client's `tick` runs at, per the newest snapshot.
    pub fn host_tick(&self, tick: u64) -> u64 {
        tick.saturating_add_signed(self.host_offset)
    }

    /// Record that client tick `client` ran at host tick `host`.
    pub fn set_offset(&mut self, client: u64, host: u64) {
        self.host_offset = i128::from(host)
            .saturating_sub(i128::from(client))
            .clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64;
    }

    /// The command to replay `player`'s body with at client tick `tick`
    /// (host tick `host`): the stored authoritative one when it is still
    /// there (it may have arrived after the tick was predicted), else what
    /// the live step used. `None` when neither is known. Read-only: the
    /// predictor and cursor are untouched.
    pub fn command_at(&self, player: u16, tick: u64, host: u64) -> Option<TickCommand> {
        self.store
            .get(player, host)
            .map(|c| c.command)
            .or_else(|| self.applied.get(&(player, tick)).copied())
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
        let host_tick = feed.host_tick(session.tick());
        let p = feed
            .predictor
            .command_for(&mut feed.store, body.wire, host_tick);
        *input = decode_tick_command(&p.command);
        feed.note_applied(body.wire, session.tick(), p.command);
    }
}

/// One seat's recent finalized commands on the host: a contiguous run of
/// at most [`MAX_REMOTE_RUN`] ticks ending at the newest.
#[derive(Debug)]
struct SeatLog {
    epoch: u8,
    first_tick: u64,
    commands: VecDeque<RemoteCommand>,
}

/// Host-side: the commands actually applied to each human seat, by
/// authoritative tick (NETPLAY-05J). Remote seats are recorded by
/// `netdrive::consume_seat_commands` from what the mailbox finalized
/// (including substitutes, tagged by their [`Source`]), the host's own
/// seat by [`record_local_seat`]. [`publish_remote_commands`] sends each
/// client everything but its own seat.
#[derive(Resource, Debug, Default)]
pub struct HostCommandLog {
    seats: BTreeMap<u16, SeatLog>,
    /// `(generation, tick)` of the last frame sent, so an `Update` that
    /// ran no fixed step sends nothing.
    sent: Option<(u64, u64)>,
}

impl HostCommandLog {
    /// Record the command applied to `player` on `tick`. A reset epoch
    /// change or a gap in ticks starts the seat's run afresh.
    pub fn record(&mut self, player: u16, epoch: u8, tick: u64, command: RemoteCommand) {
        let seat = self.seats.entry(player).or_insert_with(|| SeatLog {
            epoch,
            first_tick: tick,
            commands: VecDeque::new(),
        });
        let next = seat.first_tick + seat.commands.len() as u64;
        if seat.epoch != epoch || (!seat.commands.is_empty() && next != tick) {
            seat.epoch = epoch;
            seat.first_tick = tick;
            seat.commands.clear();
        } else if seat.commands.is_empty() {
            seat.first_tick = tick;
        }
        seat.commands.push_back(command);
        if seat.commands.len() > MAX_REMOTE_RUN {
            seat.commands.pop_front();
            seat.first_tick += 1;
        }
    }

    /// Forget every seat (the session is not running).
    pub fn clear(&mut self) {
        self.seats.clear();
        self.sent = None;
    }

    /// The frame owed to `receiver`: a run per other seat, `None` when
    /// there is nothing to say. `tick` is the host session tick.
    pub fn frame_for(&self, receiver: u16, generation: u64, tick: u64) -> Option<Message> {
        let seats: Vec<RemoteSeat> = self
            .seats
            .iter()
            .filter(|(player, seat)| **player != receiver && !seat.commands.is_empty())
            .map(|(player, seat)| RemoteSeat {
                player: *player,
                epoch: seat.epoch,
                first_tick: seat.first_tick,
                commands: seat.commands.iter().copied().collect(),
            })
            .filter(|run| run.last_tick() <= tick)
            .take(mm2_net::MAX_REMOTE_SEATS)
            .collect();
        (!seats.is_empty()).then_some(Message::RemoteCommands {
            generation,
            tick,
            seats,
        })
    }
}

/// The host's own car: wire id, settled controls, driver options, epoch.
type LocalSeat = (
    &'static NetPlayer,
    &'static VehicleInput,
    Option<&'static HumanDriver>,
    Option<&'static ResetEpoch>,
);

/// Fixed-tick, after the player's input settled: record the host's own
/// seat command for the tick, so clients can predict the host's car from
/// what it really applied.
pub fn record_local_seat(
    session: Res<Session>,
    mut log: ResMut<HostCommandLog>,
    local: Query<LocalSeat, (With<PlayerVehicle>, Without<RemotePick>)>,
) {
    if !session.is_playing() {
        return;
    }
    for (wire, input, human, epoch) in &local {
        log.record(
            wire.0,
            epoch.map_or(0, |e| e.0),
            session.tick(),
            RemoteCommand {
                command: tick_command(input, human),
                source: Source::Command,
            },
        );
    }
}

/// Update: send every rostered client the other seats' recent commands
/// on the datagram lane (TCP for a client without one — `send_to` picks).
/// Each frame repeats the last [`MAX_REMOTE_RUN`] ticks, so a dropped
/// datagram costs nothing; it leaves once per fixed tick advanced.
pub fn publish_remote_commands(
    host: Res<HostLink>,
    session: Res<Session>,
    lobby: Res<LobbyState>,
    mut log: ResMut<HostCommandLog>,
) {
    if !session.is_playing() {
        log.clear();
        return;
    }
    let key = (session.wire_generation(), session.tick());
    if log.sent == Some(key) {
        return;
    }
    log.sent = Some(key);
    for entry in &lobby.roster {
        if let Some(frame) = log.frame_for(entry.player_id, key.0, key.1) {
            // A failed send is the next frame's repeat to cover.
            let _ = host.ctl().send_to(entry.player_id, &frame);
        }
    }
}

/// Host systems: the finalized-command log and its publisher.
pub fn add_host_systems(app: &mut App) {
    app.init_resource::<HostCommandLog>()
        .add_systems(
            FixedUpdate,
            record_local_seat
                .after(crate::input::apply_player_input)
                .run_if(crate::netplay_timeline::timeline_enabled)
                .run_if(resource_exists::<HostLink>),
        )
        .add_systems(
            Update,
            publish_remote_commands
                .after(crate::net::drive_host)
                .run_if(crate::netplay_timeline::timeline_enabled)
                .run_if(resource_exists::<HostLink>),
        );
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

    /// A consumed tick leaves the store, so a replay finds it in the
    /// applied log; a stored tick wins over the log; the log is bounded.
    #[test]
    fn command_at_reads_the_store_then_the_applied_log() {
        let mut feed = RemoteFeed {
            store: store_with(10, &[cmd(100, 0), cmd(110, 0), cmd(120, 0)]),
            ..RemoteFeed::default()
        };
        let p = feed.predictor.command_for(&mut feed.store, 3, 10);
        feed.note_applied(3, 10, p.command);
        // Tick 10 is consumed (gone from the store) but replayable.
        assert!(feed.store.get(3, 10).is_none());
        assert_eq!(feed.command_at(3, 10, 10), Some(cmd(100, 0)));
        // A late authoritative command for a predicted tick beats the log.
        feed.note_applied(3, 11, cmd(1, 0));
        assert_eq!(feed.command_at(3, 11, 11), Some(cmd(110, 0)));
        assert_eq!(feed.command_at(3, 99, 99), None);
        assert_eq!(feed.command_at(4, 10, 10), None);
        // Bounded per seat, and another seat's entries are left alone.
        feed.note_applied(4, 1, cmd(7, 0));
        for t in 100..100 + 3 * APPLIED_KEEP {
            feed.note_applied(3, t, cmd(5, 0));
        }
        assert_eq!(feed.command_at(3, 10, 10), None);
        assert_eq!(feed.command_at(4, 1, 1), Some(cmd(7, 0)));
        assert!(feed.applied.len() as u64 <= APPLIED_KEEP + 2);
    }
    fn rc(throttle: u8) -> RemoteCommand {
        RemoteCommand {
            command: cmd(throttle, 0),
            source: Source::Command,
        }
    }

    #[test]
    fn host_log_frames_skip_the_receiver_and_bound_each_run() {
        let mut log = HostCommandLog::default();
        for tick in 1..=40u64 {
            log.record(0, 0, tick, rc(tick as u8));
            log.record(1, 0, tick, rc(100 + tick as u8));
            log.record(2, 0, tick, rc(200));
        }
        let Some(Message::RemoteCommands { seats, tick, .. }) = log.frame_for(2, 7, 40) else {
            panic!("a frame is owed");
        };
        assert_eq!(tick, 40);
        let ids: Vec<u16> = seats.iter().map(|s| s.player).collect();
        assert_eq!(ids, [0, 1], "the receiver's own seat is never sent");
        for run in &seats {
            assert_eq!(run.commands.len(), MAX_REMOTE_RUN);
            assert_eq!(run.last_tick(), 40);
        }
        assert_eq!(seats[0].commands[0], rc(40 - MAX_REMOTE_RUN as u8 + 1));
        assert!(HostCommandLog::default().frame_for(1, 7, 1).is_none());
    }

    #[test]
    fn host_log_restarts_a_run_on_a_gap_or_a_reset_epoch() {
        let mut log = HostCommandLog::default();
        log.record(1, 0, 5, rc(1));
        log.record(1, 0, 6, rc(2));
        log.record(1, 0, 9, rc(3));
        let run = |log: &HostCommandLog| {
            let Some(Message::RemoteCommands { mut seats, .. }) = log.frame_for(0, 1, 20) else {
                panic!("a frame is owed");
            };
            seats.remove(0)
        };
        let r = run(&log);
        assert_eq!((r.first_tick, r.commands.len()), (9, 1));
        log.record(1, 1, 10, rc(4));
        let r = run(&log);
        assert_eq!((r.epoch, r.first_tick, r.commands.len()), (1, 10, 1));
    }

    #[test]
    fn a_client_replays_the_hosts_commands_not_its_guess() {
        let mut log = HostCommandLog::default();
        for tick in 1..=20u64 {
            // The remote driver alternates, so hold-last would be wrong.
            log.record(1, 0, tick, rc(if tick % 2 == 0 { 10 } else { 200 }));
        }
        let frame = log.frame_for(2, 3, 20).unwrap();
        let mut feed = RemoteFeed::default();
        feed.store.set_generation(Some(3));
        assert!(feed.store.absorb(&frame));
        for tick in 5..=20u64 {
            let p = feed.predictor.command_for(&mut feed.store, 1, tick);
            assert_eq!(p.basis, Basis::Authoritative, "tick {tick}");
            assert_eq!(p.command.throttle, if tick % 2 == 0 { 10 } else { 200 });
        }
    }
}
