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
//! reordered one is dropped. A seek replays every actor from its start,
//! so the stage also bounds how often and how far a host can ask for
//! one ([`WorldLimits`]). No link round trip is measured, so a
//! client trails the host by the one-way delay — a few ticks on a LAN,
//! inside the tolerance — and a longer path drifts out of it and
//! re-seeks each frame.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use avian3d::prelude::{Position, Rotation};
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
pub const SYNC_TOLERANCE_TICKS: u64 = mm2_game::RACE_TICK_HZ as u64 / 20;

/// World ticks between the host's clock frames: about a second at the
/// fixed rate, so a lost frame costs a second of drift, not a session's.
pub const PUBLISH_EVERY_TICKS: u64 = mm2_game::RACE_TICK_HZ as u64;

/// The furthest tick a client will replay its scenery to. A seek steps
/// every actor from its start, so a hostile or corrupt tick must not be
/// able to ask for 2^64 steps: past this bound (about five hours at the
/// fixed rate) the frame is refused counted and the client keeps its
/// own clock.
pub const MAX_SEEK_TICKS: u64 = 1 << 20;

/// How hard a host may drive a client's re-seeks. A seek replays every
/// actor from its start — its cost grows with the target — so a frame
/// is refused unless it arrives at a sane rate and carries a tick a
/// running host could have reached since the last one this client took.
/// An honest host sends about one frame per [`PUBLISH_EVERY_TICKS`] at
/// the fixed rate and its clock cannot outrun real time, so none of
/// this touches it; a hostile or corrupt one is held to a couple of
/// replays a second and a clock that runs at a bounded multiple of real
/// time (Implementation choice — the numbers are generous margins, not
/// retail facts).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorldLimits {
    /// Least wall time between two frames of one generation that
    /// count; an earlier one is dropped throttled.
    pub min_interval: Duration,
    /// Most host ticks per wall second a clock may be seen to run at.
    pub max_ticks_per_second: f64,
    /// Ticks of allowance on top of that, so a stalled host or a
    /// bunched-up burst after a link blackout is not mistaken for a
    /// runaway clock.
    pub slack_ticks: u64,
}

impl Default for WorldLimits {
    fn default() -> Self {
        Self {
            min_interval: Duration::from_millis(500),
            // Allow four times the shared fixed rate.
            max_ticks_per_second: f64::from(mm2_game::RACE_TICK_HZ) * 4.0,
            slack_ticks: 4 * PUBLISH_EVERY_TICKS,
        }
    }
}

impl WorldLimits {
    /// No rate or growth bound — for a harness that feeds frames back
    /// to back and is not testing the bound.
    pub const UNBOUNDED: Self = Self {
        min_interval: Duration::ZERO,
        max_ticks_per_second: f64::INFINITY,
        slack_ticks: u64::MAX,
    };

    /// Whether `ticks` could follow `prev` after `elapsed` of wall time.
    fn allows(&self, prev: u64, ticks: u64, elapsed: Duration) -> bool {
        let reach = self.max_ticks_per_second * elapsed.as_secs_f64();
        let reach = if reach.is_finite() {
            self.slack_ticks.saturating_add(reach as u64)
        } else {
            u64::MAX
        };
        ticks.saturating_sub(prev) <= reach
    }
}

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
    /// Newest accepted frame per generation, at most
    /// [`STAGED_GENERATIONS`] — the oldest generation is evicted.
    frames: Vec<StagedClock>,
    limits: WorldLimits,
    /// Zero point of the wall-time stamps, set by the first frame.
    epoch: Option<Instant>,
    /// The last race-row seek that was let through: its target and the
    /// wall time (since the epoch) it was admitted at.
    row_seek: Option<(u64, Duration)>,
    stale: u64,
    throttled: u64,
    refused: u64,
    landed: u64,
    seeks: u64,
}

/// The stage's verdict on a race row's re-seek.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowSeek {
    Admitted,
    Throttled,
    Refused,
}

/// One generation's newest accepted clock frame.
#[derive(Debug, Clone, Copy)]
struct StagedClock {
    generation: u64,
    ticks: u64,
    /// Accepted but not yet applied.
    fresh: bool,
    /// Wall time (since the stage's epoch) it was accepted at.
    at: Duration,
}

impl WorldStage {
    /// A stage holding frames to `limits` rather than the defaults.
    pub fn with_limits(limits: WorldLimits) -> Self {
        Self {
            limits,
            ..Self::default()
        }
    }

    /// Hold later frames to `limits`; what is already staged stays.
    pub fn set_limits(&mut self, limits: WorldLimits) {
        self.limits = limits;
    }

    /// Queue one received clock frame, stamped with the wall clock.
    pub fn push(&mut self, generation: u64, ticks: u64) {
        let now = self.epoch.get_or_insert_with(Instant::now).elapsed();
        self.push_at(generation, ticks, now);
    }

    /// [`push`](Self::push) at an explicit wall time since the stage
    /// began — the clock a test can drive.
    pub fn push_at(&mut self, generation: u64, ticks: u64, now: Duration) {
        // Refused before it can move a watermark: one corrupt frame
        // must not turn every honest one after it into a stale drop.
        if ticks > MAX_SEEK_TICKS {
            self.refused += 1;
            return;
        }
        if let Some(entry) = self.frames.iter_mut().find(|f| f.generation == generation) {
            if ticks <= entry.ticks {
                self.stale += 1;
                return;
            }
            let elapsed = now.saturating_sub(entry.at);
            // Neither bound moves the watermark either: the next frame
            // is judged against the last one that was taken, and the
            // wall time since it only grows.
            if elapsed < self.limits.min_interval {
                self.throttled += 1;
            } else if !self.limits.allows(entry.ticks, ticks, elapsed) {
                self.refused += 1;
            } else {
                *entry = StagedClock {
                    generation,
                    ticks,
                    fresh: true,
                    at: now,
                };
            }
            return;
        }
        if self.frames.len() >= STAGED_GENERATIONS
            && let Some(oldest) = (0..self.frames.len()).min_by_key(|&i| self.frames[i].generation)
        {
            self.frames.swap_remove(oldest);
        }
        // A generation's first frame has nothing to be compared with:
        // a late joiner has to land wherever the host stands, bounded
        // by `MAX_SEEK_TICKS` alone.
        self.frames.push(StagedClock {
            generation,
            ticks,
            fresh: true,
            at: now,
        });
    }

    /// Drop everything staged and every watermark — the authority's
    /// stream ended. Counters are evidence, not stream state, and
    /// survive.
    pub fn reset(&mut self) {
        self.frames.clear();
        self.row_seek = None;
    }

    /// Judge a race row's seek to `target` by the same [`WorldLimits`]
    /// the clock frames are held to, stamped with the wall clock.
    fn admit_row_seek(&mut self, target: u64) -> RowSeek {
        let now = self.epoch.get_or_insert_with(Instant::now).elapsed();
        self.admit_row_seek_at(target, now)
    }

    /// [`admit_row_seek`](Self::admit_row_seek) at an explicit wall
    /// time since the stage began. The first seek is free (a late
    /// joiner lands wherever the host stands, bounded by
    /// [`MAX_SEEK_TICKS`]); a later one is judged against the last one
    /// let through, and — like a refused clock frame — neither a
    /// throttled nor a refused one moves that baseline.
    fn admit_row_seek_at(&mut self, target: u64, now: Duration) -> RowSeek {
        if let Some((prev, at)) = self.row_seek {
            let elapsed = now.saturating_sub(at);
            if elapsed < self.limits.min_interval {
                return RowSeek::Throttled;
            }
            if !self.limits.allows(prev, target, elapsed) {
                return RowSeek::Refused;
            }
        }
        self.row_seek = Some((target, now));
        RowSeek::Admitted
    }

    /// Frames dropped as no newer than one already seen.
    pub fn stale(&self) -> u64 {
        self.stale
    }

    /// Frames dropped for arriving sooner than
    /// [`WorldLimits::min_interval`] after the last one taken.
    pub fn throttled(&self) -> u64 {
        self.throttled
    }

    /// Frames refused (a tick past [`MAX_SEEK_TICKS`], a clock that ran
    /// faster than [`WorldLimits`] allow, or another generation's at
    /// apply time).
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
            if !entry.fresh {
                continue;
            }
            entry.fresh = false;
            if entry.generation == wire {
                found = Some(entry.ticks);
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
        if clock.sync(ticks) == SyncOutcome::Queued {
            stage.seeks += 1;
        }
    }
    if let Some(mut report) = report {
        report.world_landed = stage.landed;
        report.world_seeks = stage.seeks;
        report.world_refused = stage.refused;
        report.world_throttled = stage.throttled;
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

/// What [`WorldClock::sync`] did with a target tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncOutcome {
    /// Already within [`SYNC_TOLERANCE_TICKS`]; nothing queued.
    InTolerance,
    /// A re-seek to the target is queued.
    Queued,
    /// The target is past [`MAX_SEEK_TICKS`], or (a race row) a clock
    /// that outran [`WorldLimits`]: nothing queued, and the caller
    /// reports it as a counted refusal.
    Refused,
    /// A race row's seek arrived sooner than
    /// [`WorldLimits::min_interval`] after the last one let through:
    /// nothing queued, counted separately from a refusal.
    Throttled,
}

impl WorldClock {
    /// Ask for a re-seek to `target` unless the clock is already
    /// within [`SYNC_TOLERANCE_TICKS`] of it, or the target is past
    /// [`MAX_SEEK_TICKS`]. A seek replays every actor from its start
    /// inside one fixed step, so a corrupt or hostile target must never
    /// reach [`advance_world_clock`] — the `MAX_SEEK_TICKS` bound is
    /// the one gate every source of a target (clock frame, race row)
    /// goes through; a race row adds the rate bound in
    /// [`sync_row`](Self::sync_row).
    pub fn sync(&mut self, target: u64) -> SyncOutcome {
        if target > MAX_SEEK_TICKS {
            return SyncOutcome::Refused;
        }
        if self.ticks.abs_diff(target) <= SYNC_TOLERANCE_TICKS {
            return SyncOutcome::InTolerance;
        }
        self.seek = Some(target);
        SyncOutcome::Queued
    }

    /// [`sync`](Self::sync) for a target that came off a race row. The
    /// rows ride every snapshot, so unlike a clock frame nothing about
    /// their cadence bounds how often a host could ask for a replay;
    /// a seek that would really be queued is also held to the stage's
    /// [`WorldLimits`] (a couple a second, at a clock no faster than a
    /// bounded multiple of real time). A row inside tolerance costs
    /// nothing and is never judged.
    pub fn sync_row(&mut self, target: u64, stage: &mut WorldStage) -> SyncOutcome {
        if target > MAX_SEEK_TICKS {
            return SyncOutcome::Refused;
        }
        if self.ticks.abs_diff(target) <= SYNC_TOLERANCE_TICKS {
            return SyncOutcome::InTolerance;
        }
        match stage.admit_row_seek(target) {
            RowSeek::Admitted => {
                self.seek = Some(target);
                SyncOutcome::Queued
            }
            RowSeek::Throttled => SyncOutcome::Throttled,
            RowSeek::Refused => SyncOutcome::Refused,
        }
    }
}

/// The world tick a race row stands at: the countdown steps already
/// taken, then the race clock. `None` once complete — the race clock
/// freezes there while the scenery keeps running.
pub fn world_ticks(race: &RaceState) -> Option<u64> {
    let countdown = u64::from(race.definition.countdown_ticks);
    match race.phase {
        RacePhase::Countdown { remaining } => Some(countdown.saturating_sub(u64::from(remaining))),
        // Saturating: the clock is a wire value on a predicted client.
        RacePhase::Running => Some(countdown.saturating_add(race.clock)),
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
    let stepping = matches!(
        session.phase(),
        SessionPhase::Countdown | SessionPhase::Playing | SessionPhase::Results
    );
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
    // The drivers run after this system and take the step this frame
    // counted, so while the world is stepping the scenery is replayed
    // one step short of the target and ends the frame *at* it — the
    // host's `ticks` is the count of steps its actors have taken. (A
    // target of 0 cannot be undone: the frame's step stands.) A world
    // that is not stepping gets the full replay: nothing follows it.
    let (steps, replay) = match (stepping, target) {
        (true, 0) => (1, 0),
        (true, t) => (t, t - 1),
        (false, t) => (t, t),
    };
    let dt = time.delta_secs();
    for mut leaf in &mut leaves {
        if leaf.motion.mode == DrawbridgeMode::Timed {
            leaf.motion = replay_leaf(DrawbridgeMode::Timed, replay, dt);
        }
    }
    for (mut mover, start) in &mut movers {
        mover.follower = replay_follower(&start.0, replay, dt);
    }
    for (mut train, start) in &mut trains {
        train.motion = replay_train(&start.0, replay, dt);
    }
    clock.ticks = steps;
}

/// World ticks between two [`SceneryProbe`] samples.
pub const PROBE_EVERY_TICKS: u64 = mm2_game::RACE_TICK_HZ as u64 / 2;

/// Samples a [`SceneryProbe`] keeps, newest last: about 16 s of world
/// time, enough for two processes that end a little apart to still
/// share a tick.
pub const PROBE_KEEP: usize = 32;

/// Evidence for F26-A: a digest of where the clock-driven scenery
/// stands, taken at known world ticks, so two processes can be
/// compared at the *same* tick. Timed leaves, boats, ferries and train
/// cars only — a proximity leaf depends on where cars were, not on the
/// clock, and is not compared. Read by the headless smoke's record; no
/// game system consumes it.
#[derive(Resource, Debug, Default, Clone)]
pub struct SceneryProbe {
    /// `(world tick, digest)`, oldest first, bounded by [`PROBE_KEEP`].
    pub samples: VecDeque<(u64, u64)>,
    /// Actors the newest sample covered.
    pub actors: usize,
}

/// One body's pose, quantised to a millimetre and a ten-thousandth of a
/// quaternion component so a last-bit float difference is not a
/// difference.
fn pose_hash(kind: u8, pos: Vec3, rot: Quat) -> u64 {
    let q = |v: f32, scale: f32| ((v * scale).round() as i64).to_le_bytes();
    let mut hash = crate::worldprops::fnv(0xcbf2_9ce4_8422_2325, &[kind]);
    for v in pos.to_array() {
        hash = crate::worldprops::fnv(hash, &q(v, 1_000.0));
    }
    for v in rot.to_array() {
        hash = crate::worldprops::fnv(hash, &q(v, 10_000.0));
    }
    hash
}

/// Sample the scenery's poses on every [`PROBE_EVERY_TICKS`]th world
/// tick. Runs after the drivers, so the poses are those of the step the
/// clock just counted. Order-independent: the per-actor hashes are
/// sorted before they are folded, because two processes spawn the same
/// actors under different entity ids.
pub fn sample_scenery(
    clock: Res<WorldClock>,
    leaves: Query<(&DrawbridgeLeaf, &Position, &Rotation)>,
    movers: Query<(&Position, &Rotation), With<Mover>>,
    cars: Query<(&Position, &Rotation), With<crate::movers::TrainCar>>,
    mut probe: ResMut<SceneryProbe>,
) {
    let tick = clock.ticks;
    if tick == 0 || !tick.is_multiple_of(PROBE_EVERY_TICKS) {
        return;
    }
    if probe.samples.back().is_some_and(|&(t, _)| t >= tick) {
        return;
    }
    let mut poses: Vec<u64> = leaves
        .iter()
        .filter(|(leaf, ..)| leaf.motion.mode == DrawbridgeMode::Timed)
        .map(|(_, p, r)| pose_hash(0, p.0, r.0))
        .chain(movers.iter().map(|(p, r)| pose_hash(1, p.0, r.0)))
        .chain(cars.iter().map(|(p, r)| pose_hash(2, p.0, r.0)))
        .collect();
    if poses.is_empty() {
        return;
    }
    poses.sort_unstable();
    let digest = poses.iter().fold(0xcbf2_9ce4_8422_2325, |hash, pose| {
        crate::worldprops::fnv(hash, &pose.to_le_bytes())
    });
    probe.actors = poses.len();
    if probe.samples.len() == PROBE_KEEP {
        probe.samples.pop_front();
    }
    probe.samples.push_back((tick, digest));
}

#[cfg(test)]
mod tests {
    use super::*;
    use mm2_game::drawbridge::{CLOSED_WAIT, LeafPhase};

    const DT: f32 = 1.0 / 60.0;

    #[test]
    fn the_stage_keeps_the_newest_tick_per_generation() {
        let mut stage = WorldStage::with_limits(WorldLimits::UNBOUNDED);
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
        let mut stage = WorldStage::with_limits(WorldLimits::UNBOUNDED);
        stage.push(4, 9_000);
        stage.push(3, 100);
        assert_eq!(stage.stale(), 0);
        assert_eq!(stage.take_for(3), Some(100));
        assert_eq!(stage.refused(), 1, "generation 4's frame is not ours");
    }

    #[test]
    fn an_implausible_tick_is_refused_before_it_can_become_a_watermark() {
        let mut stage = WorldStage::with_limits(WorldLimits::UNBOUNDED);
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
            stage.frames.iter().all(|f| f.generation >= 60),
            "the oldest generations are the ones evicted"
        );
    }

    #[test]
    fn a_reset_forgets_the_stream_but_not_the_evidence() {
        let mut stage = WorldStage::with_limits(WorldLimits::UNBOUNDED);
        stage.push(1, 50);
        stage.push(1, 40);
        stage.reset();
        assert_eq!(stage.take_for(1), None);
        stage.push(1, 40);
        assert_eq!(stage.stale(), 1, "counters survive");
        assert_eq!(stage.take_for(1), Some(40), "the watermark does not");
    }

    const SEC: Duration = Duration::from_secs(1);

    #[test]
    fn frames_arriving_faster_than_the_interval_are_throttled_not_taken() {
        let mut stage = WorldStage::default();
        stage.push_at(1, 1_000, Duration::ZERO);
        stage.push_at(1, 1_120, Duration::from_millis(10));
        stage.push_at(1, 1_240, Duration::from_millis(499));
        assert_eq!(stage.throttled(), 2);
        assert_eq!(stage.stale(), 0, "a throttled frame is not a stale one");
        assert_eq!(stage.take_for(1), Some(1_000), "only the first counted");
        // The watermark stayed on the frame that was taken, so the next
        // one a half-second on is judged against it, not against a
        // frame that never counted.
        stage.push_at(1, 1_240, Duration::from_millis(500));
        assert_eq!(stage.take_for(1), Some(1_240));
        assert_eq!(stage.throttled(), 2);
    }

    #[test]
    fn a_clock_that_outruns_real_time_is_refused_without_becoming_the_watermark() {
        let limits = WorldLimits {
            min_interval: Duration::ZERO,
            max_ticks_per_second: 100.0,
            slack_ticks: 50,
        };
        let mut stage = WorldStage::with_limits(limits);
        stage.push_at(1, 10_000, Duration::ZERO);
        assert_eq!(stage.take_for(1), Some(10_000), "the first frame is free");
        // 2 s on: 200 ticks of reach plus 50 slack — 250 is the edge.
        stage.push_at(1, 10_251, 2 * SEC);
        assert_eq!(stage.refused(), 1);
        assert_eq!(stage.take_for(1), None);
        stage.push_at(1, 10_250, 2 * SEC);
        assert_eq!(stage.take_for(1), Some(10_250), "the bound is inclusive");
        assert_eq!(stage.refused(), 1, "the refused frame left no watermark");
        // The wall time since the last taken frame keeps growing, so an
        // honest host that was merely bunched up by a blackout catches up.
        stage.push_at(1, 10_250 + 10 * 100 + 50, 12 * SEC);
        assert_eq!(stage.take_for(1), Some(11_300));
    }

    #[test]
    fn race_row_seeks_are_throttled_and_held_to_the_growth_bound() {
        let limits = WorldLimits {
            min_interval: SEC / 2,
            max_ticks_per_second: 100.0,
            slack_ticks: 50,
        };
        let mut stage = WorldStage::with_limits(limits);
        assert_eq!(
            stage.admit_row_seek_at(900_000, Duration::ZERO),
            RowSeek::Admitted,
            "the first seek is free: a late joiner lands where the host stands"
        );
        // Back to back: throttled whatever the target, and it leaves no baseline.
        assert_eq!(
            stage.admit_row_seek_at(900_001, SEC / 4),
            RowSeek::Throttled
        );
        // 2 s on: 200 ticks of reach plus 50 slack past the last admitted.
        assert_eq!(stage.admit_row_seek_at(900_251, 2 * SEC), RowSeek::Refused);
        assert_eq!(
            stage.admit_row_seek_at(900_250, 2 * SEC),
            RowSeek::Admitted,
            "the bound is inclusive and the refused row moved nothing"
        );
        // A jump back costs less than the replay already paid.
        assert_eq!(
            stage.admit_row_seek_at(10, 3 * SEC),
            RowSeek::Admitted,
            "a regression is never a growth"
        );
        stage.reset();
        assert_eq!(
            stage.admit_row_seek_at(1_000_000, 3 * SEC),
            RowSeek::Admitted,
            "a new stream starts free"
        );
    }

    #[test]
    fn sync_row_only_judges_a_seek_that_would_be_queued() {
        let mut stage = WorldStage::with_limits(WorldLimits {
            min_interval: 10 * SEC,
            ..WorldLimits::default()
        });
        let mut clock = WorldClock {
            ticks: 1_000,
            seek: None,
        };
        // Inside tolerance neither queues nor spends the allowance.
        assert_eq!(
            clock.sync_row(1_000 + SYNC_TOLERANCE_TICKS, &mut stage),
            SyncOutcome::InTolerance
        );
        assert_eq!(clock.sync_row(5_000, &mut stage), SyncOutcome::Queued);
        assert_eq!(clock.seek, Some(5_000));
        clock.seek = None;
        assert_eq!(clock.sync_row(9_000, &mut stage), SyncOutcome::Throttled);
        assert_eq!(clock.seek, None, "nothing was queued");
        assert_eq!(
            clock.sync_row(MAX_SEEK_TICKS + 1, &mut stage),
            SyncOutcome::Refused
        );
    }

    #[test]
    fn a_regressing_clock_is_stale_whatever_the_limits() {
        let mut stage = WorldStage::with_limits(WorldLimits::UNBOUNDED);
        stage.push_at(1, 500, Duration::ZERO);
        stage.push_at(1, 499, SEC);
        assert_eq!(stage.stale(), 1);
        stage.push_at(1, u64::MAX / 2, 2 * SEC);
        assert_eq!(stage.refused(), 1, "the seek cap still holds unbounded");
    }

    #[test]
    fn the_limits_bound_the_seek_work_a_hostile_host_can_extract() {
        // A host sending a frame every millisecond with a clock running
        // ten times too fast: the default limits take at most one frame
        // per interval and none past the growth bound.
        let mut stage = WorldStage::default();
        let (mut taken, mut tick) = (0u64, 0u64);
        for ms in 0..10_000u64 {
            tick += 5; // 5 ticks per ms is 5,000 per second, ~10x the cap
            stage.push_at(1, tick, Duration::from_millis(ms));
            if stage.take_for(1).is_some() {
                taken += 1;
            }
        }
        // First frame is free; then at most one per 500 ms and only
        // while the claimed clock is plausible: the runaway is shut out.
        assert!(taken <= 1 + 10_000 / 500, "taken {taken}");
        assert!(stage.refused() > 0 && stage.throttled() > 0);
    }

    #[test]
    fn an_honest_hosts_cadence_is_never_limited() {
        let mut stage = WorldStage::default();
        // 60 Hz world, a frame every PUBLISH_EVERY_TICKS (1 s).
        for i in 1..=60u64 {
            stage.push_at(1, i * PUBLISH_EVERY_TICKS, Duration::from_secs(i));
            assert_eq!(
                stage.take_for(1),
                Some(i * PUBLISH_EVERY_TICKS),
                "frame {i}"
            );
        }
        // A 3 s link blackout, then the three queued frames arrive
        // together: the first lands (four seconds of wall time cover
        // its ticks), the two that bunch up behind it are throttled,
        // and the next frame on cadence lands again — the clock is at
        // most a second behind, never refused.
        let base = 60 * PUBLISH_EVERY_TICKS;
        let t = 64 * SEC;
        for k in 1..=3u64 {
            stage.push_at(1, base + k * PUBLISH_EVERY_TICKS, t);
        }
        assert_eq!(stage.take_for(1), Some(base + PUBLISH_EVERY_TICKS));
        stage.push_at(1, base + 4 * PUBLISH_EVERY_TICKS, t + SEC);
        assert_eq!(stage.take_for(1), Some(base + 4 * PUBLISH_EVERY_TICKS));
        assert_eq!(
            (stage.throttled(), stage.refused(), stage.stale()),
            (2, 0, 0)
        );
    }

    #[test]
    fn sync_ignores_jitter_and_queues_real_drift() {
        let mut clock = WorldClock {
            ticks: 100,
            seek: None,
        };
        assert_eq!(
            clock.sync(100 + SYNC_TOLERANCE_TICKS),
            SyncOutcome::InTolerance
        );
        assert_eq!(
            clock.sync(100 - SYNC_TOLERANCE_TICKS),
            SyncOutcome::InTolerance
        );
        assert_eq!(clock.seek, None);
        assert_eq!(
            clock.sync(100 + SYNC_TOLERANCE_TICKS + 1),
            SyncOutcome::Queued
        );
        assert_eq!(clock.seek, Some(100 + SYNC_TOLERANCE_TICKS + 1));
    }

    #[test]
    fn sync_refuses_a_target_past_the_seek_bound_and_queues_nothing() {
        let mut clock = WorldClock {
            ticks: 100,
            seek: None,
        };
        assert_eq!(clock.sync(MAX_SEEK_TICKS + 1), SyncOutcome::Refused);
        assert_eq!(clock.sync(u64::MAX), SyncOutcome::Refused);
        assert_eq!(clock.seek, None, "a refusal leaves no seek behind");
        assert_eq!(clock.sync(MAX_SEEK_TICKS), SyncOutcome::Queued);
        assert_eq!(clock.seek, Some(MAX_SEEK_TICKS), "the bound is inclusive");
        // A refusal does not cancel a seek already queued.
        let mut clock = WorldClock {
            ticks: 100,
            seek: Some(500),
        };
        assert_eq!(clock.sync(u64::MAX), SyncOutcome::Refused);
        assert_eq!(clock.seek, Some(500));
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
        assert_eq!(clock.sync(2_000), SyncOutcome::Queued);
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
        // The frame's driver step follows, and lands on the target.
        let mut seeked = world.get::<DrawbridgeLeaf>(timed).unwrap().motion;
        assert_eq!(
            seeked,
            replay_leaf(DrawbridgeMode::Timed, 1_999, fixed_dt()),
            "a stepping world is replayed one step short of the target"
        );
        seeked.step(fixed_dt());
        assert_eq!(
            seeked,
            replay_leaf(DrawbridgeMode::Timed, 2_000, fixed_dt())
        );
        assert_eq!(
            world.get::<DrawbridgeLeaf>(prox).unwrap().motion,
            LeafMotion::new(DrawbridgeMode::Proximity)
        );
        let clock = world.resource::<WorldClock>();
        assert_eq!((clock.ticks, clock.seek), (2_000, None));
    }

    /// One `advance_world_clock` frame in `phase` with a timed leaf and
    /// a seek to `target` queued, then the leaf driver's step if the
    /// world is stepping. Returns the leaf's motion and the clock.
    fn seek_frame(phase: &SessionPhase, target: u64) -> (LeafMotion, WorldClock) {
        let mut app = App::new();
        let mut fixed = Time::<Fixed>::from_hz(60.0);
        fixed.advance_by(std::time::Duration::from_secs_f32(DT));
        let mut session = Session::default();
        session.transition(SessionPhase::Loading).unwrap();
        session.transition(SessionPhase::Ready).unwrap();
        if *phase != SessionPhase::Ready {
            session.transition(SessionPhase::Countdown).unwrap();
            if *phase != SessionPhase::Countdown {
                session.transition(SessionPhase::Playing).unwrap();
            }
            if *phase == SessionPhase::Results {
                session.transition(SessionPhase::Results).unwrap();
            }
        }
        app.insert_resource(fixed)
            .insert_resource(session)
            .insert_resource(WorldClock {
                ticks: 0,
                seek: Some(target),
            })
            .add_systems(Update, advance_world_clock);
        let leaf = app
            .world_mut()
            .spawn(DrawbridgeLeaf {
                motion: LeafMotion::new(DrawbridgeMode::Timed),
                partner: None,
                hinge: Vec3::ZERO,
                base: Quat::IDENTITY,
            })
            .id();
        app.update();
        let mut motion = app.world().get::<DrawbridgeLeaf>(leaf).unwrap().motion;
        if matches!(
            app.world().resource::<Session>().phase(),
            SessionPhase::Countdown | SessionPhase::Playing | SessionPhase::Results
        ) {
            motion.step(fixed_dt());
        }
        (motion, *app.world().resource::<WorldClock>())
    }

    #[test]
    fn a_seeked_peer_stands_where_a_live_peer_stands_at_the_same_clock() {
        // A live peer's clock is the count of steps its actors took. A
        // seek in a stepping world must end the frame at the target
        // too, or every re-seeked client trails its clock by a step.
        for phase in &[
            SessionPhase::Countdown,
            SessionPhase::Playing,
            SessionPhase::Results,
        ] {
            for target in [1, 2, 600, 2_000] {
                let (motion, clock) = seek_frame(phase, target);
                assert_eq!(clock.ticks, target, "{phase:?} {target}");
                assert_eq!(
                    motion,
                    replay_leaf(DrawbridgeMode::Timed, target, fixed_dt()),
                    "{phase:?} {target}"
                );
            }
        }
        // A target of 0 cannot undo the frame's own step.
        let (motion, clock) = seek_frame(&SessionPhase::Countdown, 0);
        assert_eq!(clock.ticks, 1);
        assert_eq!(motion, replay_leaf(DrawbridgeMode::Timed, 1, fixed_dt()));
    }

    #[test]
    fn a_seek_before_the_world_steps_replays_in_full() {
        // A joiner's first race row seeks in `Ready`; no driver step
        // follows, and the first `Countdown` frame adds its own.
        let (motion, clock) = seek_frame(&SessionPhase::Ready, 180);
        assert_eq!(clock.ticks, 180);
        assert_eq!(motion, replay_leaf(DrawbridgeMode::Timed, 180, fixed_dt()));
    }

    fn probe_app(ticks: u64, bodies: &[Vec3]) -> App {
        let mut app = App::new();
        app.insert_resource(WorldClock { ticks, seek: None })
            .init_resource::<SceneryProbe>()
            .add_systems(Update, sample_scenery);
        let path = vec![Vec3::ZERO, Vec3::X * 50.0];
        for &at in bodies {
            app.world_mut().spawn((
                Mover {
                    family: crate::movers::MoverFamily::Sailboat,
                    follower: PathFollower::new(path.clone(), 5.0).unwrap(),
                    lift: 0.0,
                },
                Position(at),
                Rotation(Quat::IDENTITY),
            ));
        }
        app
    }

    #[test]
    fn the_probe_samples_on_its_stride_and_ignores_spawn_order() {
        let (a, b) = (Vec3::new(1.0, 2.0, 3.0), Vec3::new(-4.0, 0.0, 9.0));
        let sample = |ticks, bodies: &[Vec3]| {
            let mut app = probe_app(ticks, bodies);
            app.update();
            app.update(); // a repeat frame at the same tick adds nothing
            app.world().resource::<SceneryProbe>().clone()
        };
        let forward = sample(PROBE_EVERY_TICKS, &[a, b]);
        let backward = sample(PROBE_EVERY_TICKS, &[b, a]);
        assert_eq!(forward.samples.len(), 1);
        assert_eq!(forward.actors, 2);
        assert_eq!(
            forward.samples, backward.samples,
            "entity order is not state"
        );
        assert_ne!(
            forward.samples,
            sample(PROBE_EVERY_TICKS, &[a, a]).samples,
            "a different pose is a different digest"
        );
        assert_ne!(
            forward.samples,
            sample(PROBE_EVERY_TICKS, &[a]).samples,
            "a missing actor is too"
        );
        assert!(sample(PROBE_EVERY_TICKS + 1, &[a, b]).samples.is_empty());
        assert!(sample(0, &[a, b]).samples.is_empty());
        assert!(sample(PROBE_EVERY_TICKS, &[]).samples.is_empty());
        // Sub-millimetre float noise is not a difference.
        assert_eq!(
            forward.samples,
            sample(PROBE_EVERY_TICKS, &[a + Vec3::splat(1e-5), b]).samples
        );
    }

    #[test]
    fn the_probe_keeps_only_the_newest_samples() {
        let mut app = probe_app(0, &[Vec3::ONE]);
        for i in 1..=(PROBE_KEEP as u64 + 5) {
            app.world_mut().resource_mut::<WorldClock>().ticks = i * PROBE_EVERY_TICKS;
            app.update();
        }
        let probe = app.world().resource::<SceneryProbe>();
        assert_eq!(probe.samples.len(), PROBE_KEEP);
        assert_eq!(probe.samples.front().unwrap().0, 6 * PROBE_EVERY_TICKS);
        assert_eq!(
            probe.samples.back().unwrap().0,
            (PROBE_KEEP as u64 + 5) * PROBE_EVERY_TICKS
        );
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
