//! The world clock: one tick count the timed scenery (drawbridge
//! leaves, boats, ferries, Underground trains) all derive from, so
//! peers can agree on it.
//!
//! Each peer steps its scenery locally from the moment its session
//! entered `Countdown`, which only aligns peers to within their
//! session-start skew (F26-A). The host's race row already carries a
//! clock; [`WorldClock::sync`] turns it into a target tick and
//! [`advance_world_clock`] re-seeks the scenery there by replaying the
//! same fixed steps from each actor's recorded start, so a re-seeked
//! peer is step-for-step what the host would have. Proximity leaves
//! are not seeked — their state depends on where cars were, not on
//! time alone, and they stay local until the host carries a trigger.

use bevy::prelude::*;
use mm2_game::drawbridge::{DrawbridgeMode, LeafMotion};
use mm2_game::movers::{PathFollower, TrainMotion};
use mm2_game::race::{RacePhase, RaceState};
use mm2_game::{Session, SessionPhase};

use crate::drawbridge::DrawbridgeLeaf;
use crate::movers::{Mover, Train};

/// A peer within this many ticks of the host's world time is left
/// alone: a seek replays the whole history, and snapshot jitter would
/// otherwise trigger one on every row.
pub const SYNC_TOLERANCE_TICKS: u64 = 6;

/// Fixed steps the scenery has run, and a pending re-seek.
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct WorldClock {
    /// Steps since the session entered `Countdown`.
    pub ticks: u64,
    /// Target of the next re-seek, consumed by [`advance_world_clock`].
    pub seek: Option<u64>,
}

impl WorldClock {
    /// Ask for a re-seek to `target` unless the clock is already
    /// within [`SYNC_TOLERANCE_TICKS`] of it. Returns whether one was
    /// queued.
    pub fn sync(&mut self, target: u64) -> bool {
        if self.ticks.abs_diff(target) <= SYNC_TOLERANCE_TICKS {
            return false;
        }
        self.seek = Some(target);
        true
    }
}

/// The world tick a race row stands at: the countdown steps already
/// taken, then the race clock. `None` once complete — the race clock
/// freezes there while the scenery keeps running.
pub fn world_ticks(race: &RaceState) -> Option<u64> {
    let countdown = u64::from(race.definition.countdown_ticks);
    match race.phase {
        RacePhase::Countdown { remaining } => Some(countdown.saturating_sub(u64::from(remaining))),
        RacePhase::Running => Some(countdown + race.clock),
        RacePhase::Complete => None,
    }
}

/// A scenery actor's state at the moment it spawned.
#[derive(Component, Debug, Clone)]
pub struct WorldStart<T>(pub T);

/// `motion` stepped `ticks` times by `dt` from its start.
pub fn replay_leaf(mode: DrawbridgeMode, ticks: u64, dt: f32) -> LeafMotion {
    let mut motion = LeafMotion::new(mode);
    for _ in 0..ticks {
        motion.step(dt);
    }
    motion
}

/// `start` advanced `ticks` times by `dt`.
pub fn replay_follower(start: &PathFollower, ticks: u64, dt: f32) -> PathFollower {
    let mut f = start.clone();
    for _ in 0..ticks {
        f.advance(dt);
    }
    f
}

/// `start` stepped `ticks` times by `dt`.
pub fn replay_train(start: &TrainMotion, ticks: u64, dt: f32) -> TrainMotion {
    let mut t = start.clone();
    for _ in 0..ticks {
        t.step(dt);
    }
    t
}

/// Record each actor's spawn state the first time it is seen.
pub fn capture_world_start(
    mut commands: Commands,
    movers: Query<(Entity, &Mover), Added<Mover>>,
    trains: Query<(Entity, &Train), Added<Train>>,
) {
    for (entity, mover) in &movers {
        commands
            .entity(entity)
            .insert(WorldStart(mover.follower.clone()));
    }
    for (entity, train) in &trains {
        commands
            .entity(entity)
            .insert(WorldStart(train.motion.clone()));
    }
}

/// Count the running world's steps and carry out a pending re-seek.
/// Runs before the drivers, which then take this step from the
/// re-seeked state.
pub fn advance_world_clock(
    session: Res<Session>,
    time: Res<Time<Fixed>>,
    mut clock: ResMut<WorldClock>,
    mut leaves: Query<&mut DrawbridgeLeaf>,
    mut movers: Query<(&mut Mover, &WorldStart<PathFollower>)>,
    mut trains: Query<(&mut Train, &WorldStart<TrainMotion>)>,
) {
    if matches!(
        session.phase(),
        SessionPhase::Countdown | SessionPhase::Playing | SessionPhase::Results
    ) {
        clock.ticks += 1;
    }
    let Some(target) = clock.seek.take() else {
        return;
    };
    let dt = time.delta_secs();
    for mut leaf in &mut leaves {
        if leaf.motion.mode == DrawbridgeMode::Timed {
            leaf.motion = replay_leaf(DrawbridgeMode::Timed, target, dt);
        }
    }
    for (mut mover, start) in &mut movers {
        mover.follower = replay_follower(&start.0, target, dt);
    }
    for (mut train, start) in &mut trains {
        train.motion = replay_train(&start.0, target, dt);
    }
    clock.ticks = target;
}

#[cfg(test)]
mod tests {
    use super::*;
    use mm2_game::drawbridge::{CLOSED_WAIT, LeafPhase};

    const DT: f32 = 1.0 / 60.0;

    #[test]
    fn sync_ignores_jitter_and_queues_real_drift() {
        let mut clock = WorldClock {
            ticks: 100,
            seek: None,
        };
        assert!(!clock.sync(100 + SYNC_TOLERANCE_TICKS));
        assert!(!clock.sync(100 - SYNC_TOLERANCE_TICKS));
        assert_eq!(clock.seek, None);
        assert!(clock.sync(100 + SYNC_TOLERANCE_TICKS + 1));
        assert_eq!(clock.seek, Some(100 + SYNC_TOLERANCE_TICKS + 1));
    }

    #[test]
    fn replayed_leaf_equals_stepping_live() {
        // A skewed peer and a replayed peer land on the same state.
        let ticks = (CLOSED_WAIT / DT) as u64 + 300;
        let mut live = LeafMotion::new(DrawbridgeMode::Timed);
        for _ in 0..ticks {
            live.step(DT);
        }
        assert_eq!(live.phase, LeafPhase::Opening);
        assert_eq!(replay_leaf(DrawbridgeMode::Timed, ticks, DT), live);
    }

    #[test]
    fn replayed_follower_and_train_equal_stepping_live() {
        let pts = vec![
            Vec3::ZERO,
            Vec3::new(100.0, 0.0, 0.0),
            Vec3::new(100.0, 0.0, 100.0),
            Vec3::new(0.0, 0.0, 100.0),
        ];
        let start = PathFollower::new(pts.clone(), 10.0).unwrap();
        let mut live = start.clone();
        for _ in 0..5_000 {
            live.advance(DT);
        }
        let back = replay_follower(&start, 5_000, DT);
        assert_eq!(back.pose(), live.pose());

        let tstart = TrainMotion::new(&pts).unwrap();
        let mut tlive = tstart.clone();
        for _ in 0..3_000 {
            tlive.step(DT);
        }
        let tback = replay_train(&tstart, 3_000, DT);
        assert_eq!(tback.phase, tlive.phase);
        assert_eq!(tback.car_pose(0), tlive.car_pose(0));
    }

    #[test]
    fn a_skewed_peer_converges_after_a_seek() {
        let mut host = LeafMotion::new(DrawbridgeMode::Timed);
        let mut client = LeafMotion::new(DrawbridgeMode::Timed);
        for _ in 0..2_000 {
            host.step(DT);
        }
        for _ in 0..1_000 {
            client.step(DT);
        }
        assert_ne!(host, client);
        let mut clock = WorldClock {
            ticks: 1_000,
            seek: None,
        };
        assert!(clock.sync(2_000));
        client = replay_leaf(DrawbridgeMode::Timed, clock.seek.unwrap(), DT);
        assert_eq!(host, client);
    }

    #[test]
    fn the_system_seeks_timed_leaves_and_leaves_proximity_alone() {
        let mut app = App::new();
        let mut fixed = Time::<Fixed>::from_hz(60.0);
        fixed.advance_by(std::time::Duration::from_secs_f32(DT));
        app.insert_resource(fixed)
            .insert_resource(Session::default())
            .insert_resource(WorldClock {
                ticks: 0,
                seek: Some(2_000),
            })
            .add_systems(Update, advance_world_clock);
        let leaf = |mode| DrawbridgeLeaf {
            motion: LeafMotion::new(mode),
            partner: None,
            hinge: Vec3::ZERO,
            base: Quat::IDENTITY,
        };
        let timed = app.world_mut().spawn(leaf(DrawbridgeMode::Timed)).id();
        let prox = app.world_mut().spawn(leaf(DrawbridgeMode::Proximity)).id();
        app.update();
        let world = app.world();
        assert_eq!(
            world.get::<DrawbridgeLeaf>(timed).unwrap().motion,
            replay_leaf(DrawbridgeMode::Timed, 2_000, fixed_dt()),
        );
        assert_eq!(
            world.get::<DrawbridgeLeaf>(prox).unwrap().motion,
            LeafMotion::new(DrawbridgeMode::Proximity)
        );
        let clock = world.resource::<WorldClock>();
        assert_eq!((clock.ticks, clock.seek), (2_000, None));
    }

    fn fixed_dt() -> f32 {
        std::time::Duration::from_secs_f32(DT).as_secs_f32()
    }
}
