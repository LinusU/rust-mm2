//! Proximity drawbridge leaves over the wire (F26-A.4, protocol v26).
//!
//! A timed leaf derives from the world clock, so every peer already
//! agrees on it ([`crate::worldclock`]). A proximity leaf opens for
//! whoever came near it — a fact only the authority sees, since it
//! alone has every participant's car — so the authority decides the
//! trigger and publishes the leaf's state as [`Message::Leaves`]; a
//! client never triggers one itself ([`crate::drawbridge`]). Between
//! frames each peer steps the leaf with the same fixed-step cycle, so a
//! frame only has to carry the state at one instant: the authority
//! sends one when any leaf changes phase and, as healing for a lost
//! frame, about once a second of world time. A client lands the state
//! the frame names and so trails the authority by the one-way delay —
//! a few thousandths of a radian at the retail raise rate.
//!
//! No retail bridge file authors a `prox` path (WLD-26), so this is
//! mod-content only; what the original does for a proximity leaf in a
//! networked session is not recovered, and the host-authoritative
//! reading here is a design choice (see `docs/original-rules.md`).

use bevy::prelude::*;
use mm2_game::drawbridge::{DrawbridgeMode, LeafMotion};
use mm2_game::{Session, SessionPhase};
use mm2_net::{MAX_SNAP_LEAVES, Message, SnapLeaf};

use crate::drawbridge::DrawbridgeLeaf;
use crate::net::HostLink;
use crate::netdrive::RemoteSnaps;
use crate::worldclock::{PUBLISH_EVERY_TICKS, WorldClock};

/// Generations the client-side inbox tracks at once, as the world
/// clock's does.
const STAGED_GENERATIONS: usize = 4;

#[derive(Debug, Clone)]
struct StagedLeaves {
    generation: u64,
    tick: u64,
    rows: Vec<SnapLeaf>,
    fresh: bool,
}

/// The client-side leaves inbox, a field of [`RemoteSnaps`] so the
/// stream's authority boundaries reset it with everything else. Holds
/// the newest frame per generation: a frame is state, so an older or
/// equal one adds nothing and is dropped counted.
#[derive(Default)]
pub struct LeafStage {
    frames: Vec<StagedLeaves>,
    stale: u64,
    refused: u64,
    landed: u64,
    rows_applied: u64,
    rows_refused: u64,
}

impl LeafStage {
    /// Queue one received frame.
    pub fn push(&mut self, generation: u64, tick: u64, rows: Vec<SnapLeaf>) {
        if let Some(entry) = self.frames.iter_mut().find(|f| f.generation == generation) {
            if tick <= entry.tick {
                self.stale += 1;
            } else {
                *entry = StagedLeaves {
                    generation,
                    tick,
                    rows,
                    fresh: true,
                };
            }
            return;
        }
        if self.frames.len() >= STAGED_GENERATIONS
            && let Some(oldest) = (0..self.frames.len()).min_by_key(|&i| self.frames[i].generation)
        {
            self.frames.swap_remove(oldest);
        }
        self.frames.push(StagedLeaves {
            generation,
            tick,
            rows,
            fresh: true,
        });
    }

    /// Drop everything staged — the authority's stream ended. Counters
    /// are evidence and survive.
    pub fn reset(&mut self) {
        self.frames.clear();
    }

    /// The rows waiting for `wire`'s generation; fresh frames of any
    /// other generation are dropped refused.
    fn take_for(&mut self, wire: u64) -> Option<Vec<SnapLeaf>> {
        let mut found = None;
        for entry in &mut self.frames {
            if !entry.fresh {
                continue;
            }
            entry.fresh = false;
            if entry.generation == wire {
                found = Some(entry.rows.clone());
            } else {
                self.refused += 1;
            }
        }
        found
    }

    /// Frames dropped as no newer than one already seen.
    pub fn stale(&self) -> u64 {
        self.stale
    }

    /// Frames of another generation refused at apply time.
    pub fn refused(&self) -> u64 {
        self.refused
    }

    /// Frames folded into the leaves.
    pub fn landed(&self) -> u64 {
        self.landed
    }

    /// Rows that set a leaf's state.
    pub fn rows_applied(&self) -> u64 {
        self.rows_applied
    }

    /// Rows refused: a slot that names no proximity leaf here, or a
    /// state the cycle cannot reach.
    pub fn rows_refused(&self) -> u64 {
        self.rows_refused
    }
}

/// What [`publish_leaves`] last sent: the generation, the world tick,
/// and each leaf's phase (a change in any is what makes a frame due).
type Sent = (u64, u64, Vec<u8>);

/// Host: tell the clients where every proximity leaf stands. Sends when
/// a leaf changes phase and about every [`PUBLISH_EVERY_TICKS`] of
/// world time, only for a session that has proximity leaves.
pub fn publish_leaves(
    host: Res<HostLink>,
    session: Res<Session>,
    clock: Option<Res<WorldClock>>,
    leaves: Query<&DrawbridgeLeaf>,
    mut last: Local<Option<Sent>>,
) {
    let Some(clock) = clock else {
        return;
    };
    if !session.authority_role().is_authority()
        || !matches!(
            session.phase(),
            SessionPhase::Countdown | SessionPhase::Playing | SessionPhase::Results
        )
    {
        return;
    }
    let mut prox: Vec<&DrawbridgeLeaf> = leaves
        .iter()
        .filter(|l| l.motion.mode == DrawbridgeMode::Proximity)
        .collect();
    if prox.is_empty() {
        return;
    }
    prox.sort_by_key(|l| l.slot);
    prox.truncate(MAX_SNAP_LEAVES as usize);
    let phases: Vec<u8> = prox.iter().map(|l| l.motion.phase.to_wire()).collect();
    let generation = session.wire_generation();
    let due = match &*last {
        Some((g, t, p)) if *g == generation => {
            *p != phases || clock.ticks < *t || clock.ticks - *t >= PUBLISH_EVERY_TICKS
        }
        _ => true,
    };
    if !due {
        return;
    }
    let rows = prox
        .iter()
        .map(|l| SnapLeaf {
            slot: l.slot,
            phase: l.motion.phase.to_wire(),
            timer: l.motion.timer,
            angle: l.motion.angle,
        })
        .collect();
    let frame = Message::Leaves {
        generation,
        tick: clock.ticks,
        rows,
    };
    if host.ctl().broadcast(&frame).is_ok() {
        *last = Some((generation, clock.ticks, phases));
    }
}

/// Client: land the newest staged leaves frame on the proximity leaves.
/// An authority never applies, a `Loading` or paused session holds the
/// frame, and a session that is gone drops it.
pub fn apply_leaves(
    mut snaps: ResMut<RemoteSnaps>,
    session: Res<Session>,
    mut leaves: Query<&mut DrawbridgeLeaf>,
) {
    if session.authority_role().is_authority() {
        return;
    }
    match session.phase() {
        SessionPhase::Loading | SessionPhase::Paused => return,
        SessionPhase::Ready
        | SessionPhase::Countdown
        | SessionPhase::Playing
        | SessionPhase::Results => {}
        _ => {
            snaps.leaves.reset();
            return;
        }
    }
    let stage = &mut snaps.leaves;
    let Some(rows) = stage.take_for(session.wire_generation()) else {
        return;
    };
    stage.landed += 1;
    for row in rows {
        let motion = LeafMotion::from_wire(row.phase, row.timer, row.angle);
        let leaf = leaves
            .iter_mut()
            .find(|l| l.slot == row.slot && l.motion.mode == DrawbridgeMode::Proximity);
        match (leaf, motion) {
            (Some(mut leaf), Some(motion)) => {
                leaf.motion = motion;
                stage.rows_applied += 1;
            }
            _ => {
                stage.rows_refused += 1;
                warn!(
                    slot = row.slot,
                    phase = row.phase,
                    "leaves row refused: no such proximity leaf or an unreachable state"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use avian3d::prelude::*;
    use mm2_game::drawbridge::{LeafPhase, OPEN_ANGLE};
    use mm2_game::{Player, PlayerControl, PlayerId, SessionAuthority, SessionConfig};

    fn row(slot: u16) -> SnapLeaf {
        SnapLeaf {
            slot,
            phase: LeafPhase::Resting.to_wire(),
            timer: 0.0,
            angle: 0.0,
        }
    }

    #[test]
    fn the_stage_keeps_the_newest_frame_per_generation() {
        let mut stage = LeafStage::default();
        stage.push(1, 10, vec![row(0)]);
        stage.push(1, 10, vec![row(1)]);
        stage.push(1, 5, vec![row(2)]);
        assert_eq!(stage.stale(), 2);
        stage.push(1, 20, vec![row(3)]);
        let rows = stage.take_for(1).unwrap();
        assert_eq!(rows[0].slot, 3);
        assert!(stage.take_for(1).is_none(), "a frame lands once");
    }

    #[test]
    fn another_generation_is_refused_not_applied() {
        let mut stage = LeafStage::default();
        stage.push(2, 10, vec![row(0)]);
        assert!(stage.take_for(1).is_none());
        assert_eq!(stage.refused(), 1);
    }

    #[test]
    fn the_stage_tracks_a_bounded_set_of_generations() {
        let mut stage = LeafStage::default();
        for g in 0..10 {
            stage.push(g, 1, Vec::new());
        }
        assert_eq!(stage.frames.len(), STAGED_GENERATIONS);
    }

    fn session(authority: SessionAuthority, generation: u64) -> Session {
        let mut session = Session::default();
        session
            .begin_generation(
                SessionConfig {
                    authority,
                    ..SessionConfig::default()
                },
                generation,
            )
            .unwrap();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Countdown).unwrap();
        session.transition(SessionPhase::Playing).unwrap();
        session
    }

    fn leaf(slot: u16, mode: DrawbridgeMode) -> DrawbridgeLeaf {
        DrawbridgeLeaf {
            motion: LeafMotion::new(mode),
            partner: None,
            hinge: Vec3::ZERO,
            base: Quat::IDENTITY,
            slot,
        }
    }

    fn client(generation: u64) -> App {
        let mut app = App::new();
        app.insert_resource(session(SessionAuthority::Remote, generation))
            .init_resource::<RemoteSnaps>()
            .add_systems(Update, apply_leaves);
        app
    }

    fn motion(app: &App, e: Entity) -> LeafMotion {
        app.world().get::<DrawbridgeLeaf>(e).unwrap().motion
    }

    #[test]
    fn a_client_takes_the_hosts_leaf_state() {
        let mut app = client(3);
        let prox = app
            .world_mut()
            .spawn(leaf(1, DrawbridgeMode::Proximity))
            .id();
        let timed = app.world_mut().spawn(leaf(2, DrawbridgeMode::Timed)).id();
        app.world_mut().resource_mut::<RemoteSnaps>().push_leaves(
            3,
            100,
            vec![
                SnapLeaf {
                    slot: 1,
                    phase: LeafPhase::Raised.to_wire(),
                    timer: 4.0,
                    angle: OPEN_ANGLE,
                },
                // Slot 2 is a timed leaf: the clock owns it, not this frame.
                SnapLeaf {
                    slot: 2,
                    phase: LeafPhase::Raised.to_wire(),
                    timer: 4.0,
                    angle: OPEN_ANGLE,
                },
                // An unreachable state and an unknown slot.
                SnapLeaf {
                    slot: 1,
                    phase: LeafPhase::Resting.to_wire(),
                    timer: f32::NAN,
                    angle: 0.0,
                },
                SnapLeaf {
                    slot: 9,
                    phase: 0,
                    timer: 0.0,
                    angle: 0.0,
                },
            ],
        );
        app.update();
        let got = motion(&app, prox);
        assert_eq!(got.phase, LeafPhase::Raised);
        assert_eq!(got.timer, 4.0);
        assert_eq!(got.angle, OPEN_ANGLE);
        assert_eq!(motion(&app, timed), LeafMotion::new(DrawbridgeMode::Timed));
        let snaps = app.world().resource::<RemoteSnaps>();
        assert_eq!(snaps.leaves().landed(), 1);
        assert_eq!(snaps.leaves().rows_applied(), 1);
        assert_eq!(snaps.leaves().rows_refused(), 3);
    }

    #[test]
    fn a_frame_of_another_generation_changes_nothing() {
        let mut app = client(3);
        let prox = app
            .world_mut()
            .spawn(leaf(0, DrawbridgeMode::Proximity))
            .id();
        app.world_mut().resource_mut::<RemoteSnaps>().push_leaves(
            4,
            100,
            vec![SnapLeaf {
                slot: 0,
                phase: LeafPhase::Raised.to_wire(),
                timer: 0.0,
                angle: OPEN_ANGLE,
            }],
        );
        app.update();
        assert_eq!(motion(&app, prox).phase, LeafPhase::Resting);
        assert_eq!(app.world().resource::<RemoteSnaps>().leaves().refused(), 1);
    }

    /// One fixed step of `drive_drawbridges` with a car at the hinge.
    fn drive_with_car_at_hinge(authority: SessionAuthority) -> LeafPhase {
        let mut app = App::new();
        let mut fixed = Time::<Fixed>::from_hz(60.0);
        fixed.advance_by(std::time::Duration::from_secs_f32(1.0 / 60.0));
        app.insert_resource(fixed)
            .insert_resource(session(authority, 1))
            .add_systems(Update, crate::drawbridge::drive_drawbridges);
        let prox = app
            .world_mut()
            .spawn((
                leaf(0, DrawbridgeMode::Proximity),
                Position::default(),
                Rotation::default(),
                LinearVelocity::default(),
                AngularVelocity::default(),
            ))
            .id();
        app.world_mut().spawn((
            Player {
                id: PlayerId(0),
                control: PlayerControl::Local,
            },
            Position(Vec3::new(5.0, 0.0, 0.0)),
        ));
        app.update();
        motion(&app, prox).phase
    }

    #[test]
    fn only_the_authority_opens_a_proximity_leaf_for_a_car() {
        assert_eq!(
            drive_with_car_at_hinge(SessionAuthority::Host),
            LeafPhase::Opening
        );
        assert_eq!(
            drive_with_car_at_hinge(SessionAuthority::Remote),
            LeafPhase::Resting,
            "a client waits for the host's frame"
        );
    }
}
