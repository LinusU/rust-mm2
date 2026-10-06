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
//!
//! A Cruise session has no race row, so the host also publishes the
//! clock itself ([`publish_world_clock`], protocol v20
//! `Message::World`): its tick once at the start of the countdown and
//! then about once a second of world time. A client folds the newest
//! frame of its generation into [`WorldClock::sync`]
//! ([`apply_world_clock`]), so a late joiner lands on the host's
//! drawbridge phase and mover positions within one frame's gap. The
//! frame is *state*: a lost one self-corrects on the next, and an older
//! reordered one is dropped. No link round trip is measured, so a
//! client trails the host by the one-way delay — a few ticks on a LAN,
//! inside the tolerance — and a longer path drifts out of it and
//! re-seeks each frame.

use bevy::prelude::*;
use mm2_game::drawbridge::{DrawbridgeMode, LeafMotion};
use mm2_game::movers::{PathFollower, TrainMotion};
use mm2_game::race::{RacePhase, RaceState};
use mm2_game::{Session, SessionPhase};
use mm2_net::Message;

use crate::drawbridge::DrawbridgeLeaf;
use crate::movers::{Mover, Train};
use crate::net::HostLink;
use crate::netdrive::{NetDriveReport, RemoteSnaps};

/// A peer within this many ticks of the host's world time is left
/// alone: a seek replays the whole history, and snapshot jitter would
/// otherwise trigger one on every row.
pub const SYNC_TOLERANCE_TICKS: u64 = 6;

/// World ticks between the host's clock frames: about a second at the
/// fixed rate, so a lost frame costs a second of drift, not a session's.
pub const PUBLISH_EVERY_TICKS: u64 = 120;

/// The furthest tick a client will replay its scenery to. A seek steps
/// every actor from its start, so a hostile or corrupt tick must not be
/// able to ask for 2^64 steps: past this bound (about five hours at the
/// fixed rate) the frame is refused counted and the client keeps its
/// own clock.
pub const MAX_SEEK_TICKS: u64 = 1 << 21;

/// Generations the client-side inbox tracks at once — a frame of a
/// generation that is not the session's is held only so it can be
/// refused counted, never so it can poison the session's own.
const STAGED_GENERATIONS: usize = 4;

/// The client-side world-clock inbox, a field of [`RemoteSnaps`] so the
/// stream's authority boundaries (an accepted `Start`, the link's
/// `Closed`) reset it with everything else.
///
/// Holds the newest tick seen *per generation*: a clock frame is state,
/// so an older or equal one adds nothing and is dropped counted, and a
/// frame of another generation can neither displace nor stale-mark the
/// session's own.
#[derive(Default)]
pub struct WorldStage {
    /// `(generation, newest tick, not yet applied)`, at most
    /// [`STAGED_GENERATIONS`] — the oldest generation is evicted.
    frames: Vec<(u64, u64, bool)>,
    stale: u64,
    refused: u64,
    landed: u64,
    seeks: u64,
}

impl WorldStage {
    /// Queue one received clock frame.
    pub fn push(&mut self, generation: u64, ticks: u64) {
        // Refused before it can move a watermark: one corrupt frame
        // must not turn every honest one after it into a stale drop.
        if ticks > MAX_SEEK_TICKS {
            self.refused += 1;
            return;
        }
        if let Some(entry) = self.frames.iter_mut().find(|(g, ..)| *g == generation) {
            if ticks <= entry.1 {
                self.stale += 1;
            } else {
                *entry = (generation, ticks, true);
            }
            return;
        }
        if self.frames.len() >= STAGED_GENERATIONS
            && let Some(oldest) = (0..self.frames.len()).min_by_key(|&i| self.frames[i].0)
        {
            self.frames.swap_remove(oldest);
        }
        self.frames.push((generation, ticks, true));
    }

    /// Drop everything staged and every watermark — the authority's
    /// stream ended. Counters are evidence, not stream state, and
    /// survive.
    pub fn reset(&mut self) {
        self.frames.clear();
    }

    /// Frames dropped as no newer than one already seen.
    pub fn stale(&self) -> u64 {
        self.stale
    }

    /// Frames refused (a tick past [`MAX_SEEK_TICKS`], or another
    /// generation's at apply time).
    pub fn refused(&self) -> u64 {
        self.refused
    }

    /// Frames folded into the clock.
    pub fn landed(&self) -> u64 {
        self.landed
    }

    /// Re-seeks those frames queued (the rest were inside tolerance).
    pub fn seeks(&self) -> u64 {
        self.seeks
    }

    /// The tick waiting to be applied for `wire`'s generation; frames
    /// of any other generation still waiting are dropped refused.
    fn take_for(&mut self, wire: u64) -> Option<u64> {
        let mut found = None;
        for entry in &mut self.frames {
            if !entry.2 {
                continue;
            }
            entry.2 = false;
            if entry.0 == wire {
                found = Some(entry.1);
            } else {
                self.refused += 1;
            }
        }
        found
    }
}

/// Host: tell the clients where the world clock stands.
///
/// Sends only on a hosted session in a phase where the clock runs, at
/// the start of a session and then every [`PUBLISH_EVERY_TICKS`] of
/// world time — so a pause, which stops the clock, also stops the
/// frames.
pub fn publish_world_clock(
    host: Res<HostLink>,
    session: Res<Session>,
    clock: Option<Res<WorldClock>>,
    mut last: Local<Option<(u64, u64)>>,
    report: Option<ResMut<NetDriveReport>>,
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
    let generation = session.wire_generation();
    let due = match *last {
        Some((g, t)) if g == generation => {
            // A clock that went backwards started over: send at once.
            clock.ticks < t || clock.ticks - t >= PUBLISH_EVERY_TICKS
        }
        _ => true,
    };
    if !due {
        return;
    }
    let frame = Message::World {
        generation,
        ticks: clock.ticks,
    };
    if host.ctl().broadcast(&frame).is_ok() {
        *last = Some((generation, clock.ticks));
        if let Some(mut report) = report {
            report.world_sent += 1;
        }
    }
}

/// Client: fold the newest staged clock frame into [`WorldClock`]. An
/// authority never applies, a `Loading` session holds the frame, and a
/// session that is gone drops it.
pub fn apply_world_clock(
    mut snaps: ResMut<RemoteSnaps>,
    session: Res<Session>,
    clock: Option<ResMut<WorldClock>>,
    report: Option<ResMut<NetDriveReport>>,
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
            snaps.world.reset();
            return;
        }
    }
    let stage = &mut snaps.world;
    if let Some(ticks) = stage.take_for(session.wire_generation())
        && let Some(mut clock) = clock
    {
        stage.landed += 1;
        if clock.sync(ticks) {
            stage.seeks += 1;
        }
    }
    if let Some(mut report) = report {
        report.world_landed = stage.landed;
        report.world_seeks = stage.seeks;
        report.world_refused = stage.refused;
    }
}

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
    match session.phase() {
        SessionPhase::Countdown | SessionPhase::Playing | SessionPhase::Results => {
            clock.ticks += 1;
        }
        // Between sessions the clock starts over: a second race in the
        // same process must count from its own countdown, not from the
        // last session's total, or the tolerance check compares the
        // new host's tick against a stale one. `Ready` is not here — a
        // joiner's first race row seeks while it is still `Ready`.
        SessionPhase::Menu
        | SessionPhase::Loading
        | SessionPhase::Unloading
        | SessionPhase::Failed(_) => *clock = WorldClock::default(),
        SessionPhase::Ready | SessionPhase::Paused => {}
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
    fn the_stage_keeps_the_newest_tick_per_generation() {
        let mut stage = WorldStage::default();
        stage.push(3, 500);
        stage.push(3, 400);
        stage.push(3, 500);
        assert_eq!(stage.stale(), 2, "older and equal frames add nothing");
        assert_eq!(stage.take_for(3), Some(500));
        assert_eq!(stage.take_for(3), None, "a frame applies once");
        stage.push(3, 500);
        assert_eq!(stage.stale(), 3, "the watermark outlives the drain");
        stage.push(3, 501);
        assert_eq!(stage.take_for(3), Some(501));
    }

    #[test]
    fn another_generation_is_refused_and_never_marks_the_sessions_own_stale() {
        let mut stage = WorldStage::default();
        stage.push(4, 9_000);
        stage.push(3, 100);
        assert_eq!(stage.stale(), 0);
        assert_eq!(stage.take_for(3), Some(100));
        assert_eq!(stage.refused(), 1, "generation 4's frame is not ours");
    }

    #[test]
    fn an_implausible_tick_is_refused_before_it_can_become_a_watermark() {
        let mut stage = WorldStage::default();
        stage.push(1, MAX_SEEK_TICKS + 1);
        stage.push(1, u64::MAX);
        assert_eq!(stage.refused(), 2);
        stage.push(1, 10);
        assert_eq!(stage.stale(), 0);
        assert_eq!(stage.take_for(1), Some(10));
        stage.push(1, MAX_SEEK_TICKS);
        assert_eq!(
            stage.take_for(1),
            Some(MAX_SEEK_TICKS),
            "the bound is inclusive"
        );
    }

    #[test]
    fn the_stage_tracks_a_bounded_set_of_generations() {
        let mut stage = WorldStage::default();
        for g in 0..64 {
            stage.push(g, 1);
        }
        assert_eq!(stage.frames.len(), STAGED_GENERATIONS);
        assert!(
            stage.frames.iter().all(|(g, ..)| *g >= 60),
            "the oldest generations are the ones evicted"
        );
    }

    #[test]
    fn a_reset_forgets_the_stream_but_not_the_evidence() {
        let mut stage = WorldStage::default();
        stage.push(1, 50);
        stage.push(1, 40);
        stage.reset();
        assert_eq!(stage.take_for(1), None);
        stage.push(1, 40);
        assert_eq!(stage.stale(), 1, "counters survive");
        assert_eq!(stage.take_for(1), Some(40), "the watermark does not");
    }

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
        let mut session = Session::default();
        session.transition(SessionPhase::Loading).unwrap();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Countdown).unwrap();
        app.insert_resource(fixed)
            .insert_resource(session)
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

    #[test]
    fn the_clock_starts_over_between_sessions() {
        let mut app = App::new();
        let mut fixed = Time::<Fixed>::from_hz(60.0);
        fixed.advance_by(std::time::Duration::from_secs_f32(DT));
        app.insert_resource(fixed)
            .insert_resource(Session::default())
            .insert_resource(WorldClock {
                ticks: 900,
                seek: Some(1_200),
            })
            .add_systems(Update, advance_world_clock);
        // `Menu` (the default phase): last session's count and any
        // undelivered seek are dropped.
        app.update();
        assert_eq!(*app.world().resource::<WorldClock>(), WorldClock::default());

        // `Ready` keeps a seek queued by a joiner's first race row.
        let mut session = Session::default();
        session.transition(SessionPhase::Loading).unwrap();
        session.transition(SessionPhase::Ready).unwrap();
        app.insert_resource(session);
        app.world_mut().resource_mut::<WorldClock>().seek = Some(50);
        app.update();
        let clock = app.world().resource::<WorldClock>();
        assert_eq!((clock.ticks, clock.seek), (50, None));
    }

    fn fixed_dt() -> f32 {
        std::time::Duration::from_secs_f32(DT).as_secs_f32()
    }
}
