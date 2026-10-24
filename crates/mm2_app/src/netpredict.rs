//! Own-car prediction history and replay reconciliation (NETPLAY-04,
//! `docs/netcode-redesign.md`, `docs/netplay-timeline.md`).
//!
//! Replaces the [`DriftWatch`](crate::netdrive::DriftWatch) /
//! [`SettleWatch`](crate::netdrive::SettleWatch) pulls and snaps as the
//! routine way a client's own car follows the host, behind the one
//! [`NetplayTimeline`] selector.
//!
//! **Timeline.** The client predicts its car on the production step and
//! records the whole post-step physics world after every command, keyed by
//! the command's `seq` ([`record_prediction`], `FixedLast`). A snapshot
//! entry for the own seat carries `input_ack`: the highest `seq` already
//! inside that completed host state. So the host state `S[server_tick]`
//! and the client state recorded for `seq == input_ack` are the *same
//! simulation instant* — no tick arithmetic, no "current car versus past
//! host" comparison. [`reconcile_own`] (`FixedFirst`, between steps) pulls
//! the newest staged assertion and:
//!
//! * compares position, orientation and linear/angular velocity with that
//!   recorded state; inside the tolerances nothing happens;
//! * otherwise restores the recorded world with the host's body state put
//!   in, replays the stored commands `ack+1..=newest` one fixed step at a
//!   time through [`mm2_vehicle::replay::replay_from_observed`], and writes each
//!   re-simulated post-step state back into the history so the next
//!   snapshot compares against the corrected past.
//!
//! The vehicle's wheels, drivetrain and force history come from the
//! client's own recorded state (the wire carries only the body); a
//! divergence there shows up in the body and is corrected on the next
//! comparison.
//!
//! **Two cars on one tick (NETPLAY-05A-4).** Under the selector a remote
//! human car is a dynamic contact body ([`PredictedRemoteBody`]) that the
//! recorded world already holds. The snapshot that carries the own
//! assertion carries that seat's host body state at the *same* host tick,
//! staged as [`RemoteAssertion`]s. So the comparison covers every such
//! body, a divergence in any of them triggers the replay, the restored
//! world takes the host state for the own car *and* each remote body, and
//! each replayed step drives them all: the own car with its stored command
//! and each remote body with the command [`RemoteFeed::command_at`] gives
//! for the client tick of that step (the store's authoritative command if
//! it is still there, else what the live step applied). Contact between
//! the cars therefore replays like any other dynamics. The delayed visual
//! of a remote car (`RemoteReplica`, driven by the snapshot lerp) is not
//! part of the replay and is put back to where it was; the contact bodies
//! are not put back — their replayed state is the corrected one.
//!
//! **Bounded recovery.** A routine correction is never reported as a
//! reset. The cases that cannot be replayed each take a distinct, counted
//! path ([`RecoveryCause`]) that adopts the host body state outright, and
//! snapshots that cannot be compared are skipped with a [`Skip`] reason.
//! A contact body whose pose keeps disagreeing with the host is replayed
//! against the recorded past only for [`RECOVERY_DEADLINE_TICKS`] host
//! ticks, counted from the first usable complete state; then the past is
//! dropped and the host's state is stepped forward through the commands in
//! flight, so the own car keeps its lead
//! ([`RecoveryCause::DeadlineExceeded`]).
//! An authority reset (new reset epoch) clears the history instead; the
//! commands issued before it are never replayed across it.

use std::collections::VecDeque;

use avian3d::prelude::{AngularVelocity, CollisionStart, LinearVelocity, Position, Rotation};
use bevy::ecs::message::MessageCursor;
use bevy::prelude::*;
use mm2_game::{PlayerVehicle, Session};
use mm2_vehicle::capture::{EpochRef, VehicleCapture};
use mm2_vehicle::physics_world::{WorldCapture, capture_world, restore_world};
use mm2_vehicle::replay::{HistoryRing, MAX_REPLAY_STEPS, ReplayStep, replay_from_observed};
use mm2_vehicle::{
    EngineImpairment, PreStepVelocity, RemoteReplica, Teleported, Vehicle, VehicleInput,
    VehicleState,
};

use crate::car_visual::Trailer;
use crate::net::LobbyLink;
use crate::netcmd::CommandStream;
use crate::netdrive::{
    PredictedRemoteBody, RemoteSnaps, RemoteTrailer, ResetEpoch, decode_tick_command,
};
use crate::netplay_timeline::NetplayTimeline;
use crate::netremote::RemoteFeed;

/// Host ticks a contact divergence may keep being replayed before the host
/// state is adopted outright (NETPLAY-05B-4). Designed, not original: the
/// clock starts at the first *usable complete* authoritative state (see
/// [`OwnPrediction::deadline_start`]), so a late or dropped snapshot delays
/// the start instead of eating the budget. One second covers the
/// 30-tick-delay contact tests of `docs/netcode-redesign.md`, which
/// converge in 30-41 ticks, with margin; a body still disagreeing after it
/// is not going to be corrected by yet another replay.
pub const RECOVERY_DEADLINE_TICKS: u64 = 60;

/// Position difference (m) below which a replayed snapshot is "the same
/// state". Designed: the two simulations run the same step on the same
/// quantized commands, so any real agreement is far inside this, and a
/// millimetre is invisible; larger is a real divergence worth a replay.
pub const POS_TOLERANCE: f32 = 0.01;
/// Orientation difference (radians) below which states agree (~0.06°).
pub const ROT_TOLERANCE: f32 = 0.001;
/// Linear velocity difference (m/s) below which states agree.
pub const VEL_TOLERANCE: f32 = 0.02;
/// Angular velocity difference (rad/s) below which states agree.
pub const ANG_TOLERANCE: f32 = 0.02;
/// Position difference (m) at which a contact body's disagreement counts
/// towards [`RECOVERY_DEADLINE_TICKS`]. A body that disagrees by less is
/// replayed (the tolerances above) but is not "still disagreeing": the
/// host's suspension and tyre state is not on the wire, so a parked or
/// resting body's replay keeps a residue of its own. Measured over the
/// `net_contact` bob-rams/alice-rams runs (5900 partner comparisons): the
/// 99th percentile of the residue was 0.011 m and 0.004 rad in pose, 0.22 m/s
/// and 0.14 rad/s in velocity, so with the replay tolerances alone the clock
/// ran out every [`RECOVERY_DEADLINE_TICKS`] on bodies that agreed to a
/// centimetre. Designed, not original.
pub const DEADLINE_POS_TOLERANCE: f32 = 0.05;
/// Orientation difference (radians) at which a contact body's disagreement
/// counts towards the deadline; see [`DEADLINE_POS_TOLERANCE`]. Velocities
/// never do: with the pose agreeing they are that residue.
pub const DEADLINE_ROT_TOLERANCE: f32 = 0.02;

/// The host's body state for the own seat at its completed tick, staged
/// by `apply_snapshots` for [`reconcile_own`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OwnAssertion {
    /// Wire session generation the snapshot belongs to.
    pub generation: u64,
    /// The snapshot's host tick (diagnostics; the ack is the alignment).
    pub server_tick: u64,
    /// Highest own command `seq` inside this state.
    pub ack: u64,
    pub pos: Vec3,
    pub rot: Quat,
    pub vel: Vec3,
    pub ang: Vec3,
}

/// The host's body state for a remote human seat, from the same snapshot
/// (so the same host tick) as the [`OwnAssertion`] it travels with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RemoteAssertion {
    pub wire: u16,
    /// The seat's reset epoch in that snapshot (a trailer row carries its
    /// towing seat's).
    pub epoch: u8,
    pub pos: Vec3,
    pub rot: Quat,
    pub vel: Vec3,
    pub ang: Vec3,
}

/// What `apply_snapshots` hands the fixed schedule: the newest assertion
/// (latest wins; acks only move forward, so a superseded one carries
/// nothing the newer lacks) and whether an authority reset happened.
#[derive(Debug, Default)]
pub struct OwnStage {
    /// Set by `apply_snapshots` from the selector each run.
    pub enabled: bool,
    pub assertion: Option<OwnAssertion>,
    /// The remote seats' host state from the snapshot tagged
    /// `remotes_at` (`generation`, `server_tick`); used only with the
    /// assertion of that same snapshot.
    pub remotes: Vec<RemoteAssertion>,
    pub remotes_at: Option<(u64, u64)>,
    /// The snapshot's trailer rows (`wire` is the towing seat's id), staged
    /// and gated exactly like `remotes`.
    pub trailers: Vec<RemoteAssertion>,
    /// The own seat's wire id in that snapshot, so the own rig's trailer
    /// row can be found among `trailers`.
    pub own_wire: Option<u16>,
    /// The host reset the own car: forget the history.
    pub reset: bool,
}

impl OwnStage {
    /// Drop everything staged (a new authority stream).
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

/// Why a snapshot was not compared. None of these touches the car.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skip {
    /// `input_ack == 0`: none of our commands is inside the host state.
    NoAck,
    /// Same or older ack than already compared: a duplicate or a starved
    /// host holding its ack. Nothing new to learn.
    Seen,
    /// The ack names a command we have not simulated: a corrupt or foreign
    /// snapshot.
    Future,
    /// The ack predates the last authority reset.
    PreReset,
    /// A non-finite or degenerate host state is never applied.
    Corrupt,
    /// A snapshot of another session generation.
    OtherGeneration,
}

/// Why the host state was adopted outright instead of replayed. Each is a
/// bounded, rare path with its own counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryCause {
    /// The compared state is older than the history holds.
    HistoryExhausted,
    /// More steps to replay than [`MAX_REPLAY_STEPS`].
    TooDeep,
    /// A stored command in `ack+1..=newest` is missing.
    CommandGap,
    /// `replay_from` refused (stale lifetimes, missing history, restore).
    ReplayRefused,
    /// A contact body still disagreed with the host
    /// [`RECOVERY_DEADLINE_TICKS`] host ticks after the first usable
    /// complete state that showed it.
    DeadlineExceeded,
}

/// One reconciliation's outcome, for rendering and the statistics.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Outcome {
    /// The recorded state agreed with the host's.
    Matched,
    /// Restored and replayed.
    Replayed {
        steps: usize,
        /// Host-versus-recorded difference at the compared boundary.
        pos_error: f32,
        rot_error: f32,
        /// How far the *present* car moved because of the replay.
        shift: f32,
        turn: f32,
        /// Largest host-versus-recorded position difference of a remote
        /// body at the compared boundary (0 with none compared).
        remote_error: f32,
        /// Whether any remote body exceeded a tolerance (not just position).
        remote_diverged: bool,
    },
    Recovered(RecoveryCause),
    Skipped(Skip),
    /// The authority reset the car: history cleared.
    Reset,
}

/// Counters and the last result; `Resource` so the HUD, probe and tests
/// read them. Correction *count and size* are exposed so a stream of tiny
/// corrections cannot hide behind the absence of snaps.
#[derive(Resource, Debug)]
pub struct OwnPrediction {
    generation: u64,
    ring: HistoryRing,
    /// Highest ack compared so far.
    last_ack: u64,
    /// First `seq` the history may be compared at (set by a reset).
    floor: u64,
    /// Host tick of the first usable complete authoritative state in which
    /// a contact body disagreed with the prediction. Usable means compared
    /// (not skipped); complete means every contact body and trailer in the
    /// recorded world has its host row. `None` until such a state arrives,
    /// and again after a complete state that agrees, a recovery, a reset
    /// or a new generation.
    deadline_start: Option<u64>,
    /// The most recent outcome.
    pub last: Option<Outcome>,
    pub compared: u64,
    pub matched: u64,
    pub replays: u64,
    /// Replays in which a remote body diverged beyond the tolerances.
    pub remote_replays: u64,
    pub replayed_steps: u64,
    /// Sum of present-car shifts caused by replays (m).
    pub shift_total: f32,
    /// Sum of present-car turns caused by replays (rad).
    pub turn_total: f32,
    pub resets: u64,
    pub skipped: u64,
    pub recoveries: u64,
    pub recovered_exhausted: u64,
    pub recovered_too_deep: u64,
    pub recovered_gap: u64,
    pub recovered_refused: u64,
    pub recovered_deadline: u64,
    /// Remote assertions dropped because their reset epoch is older than
    /// the contact body's.
    pub stale_remote: u64,
    /// Remote seat resets (a newer epoch than the body's history) that a
    /// consumed assertion folded into the history.
    pub remote_resets: u64,
    /// Contact starts the live world raised, as `(seq of the step, body
    /// pair)`, oldest first: what a replay's contacts are compared with.
    contacts: VecDeque<(u64, ContactPair)>,
    contact_cursor: MessageCursor<CollisionStart>,
    /// Contact starts a replay was the first to discover, raised in the
    /// live world by [`reconcile`] (their damage, sparks and audio).
    pub contacts_surfaced: u64,
}

/// Ticks a replayed contact start may differ from the raised one and still
/// be the same contact. Designed, not original: in the missed-contact runs
/// of `docs/netcode-redesign.md` (delay 30) the replayed start moves 2-4
/// ticks earlier with each newer snapshot while the car converges, and 1
/// tick later from a replay that starts just before the contact.
const CONTACT_JITTER_TICKS: u64 = 4;

/// The two bodies of a contact, in a fixed order.
type ContactPair = (Entity, Entity);

fn contact_pair(m: &CollisionStart) -> Option<ContactPair> {
    let (a, b) = (m.body1?, m.body2?);
    Some(if a < b { (a, b) } else { (b, a) })
}

impl Default for OwnPrediction {
    fn default() -> Self {
        Self {
            generation: 0,
            ring: HistoryRing::default(),
            last_ack: 0,
            floor: 0,
            deadline_start: None,
            last: None,
            compared: 0,
            matched: 0,
            replays: 0,
            remote_replays: 0,
            replayed_steps: 0,
            shift_total: 0.0,
            turn_total: 0.0,
            resets: 0,
            skipped: 0,
            recoveries: 0,
            recovered_exhausted: 0,
            recovered_too_deep: 0,
            recovered_gap: 0,
            recovered_refused: 0,
            recovered_deadline: 0,
            stale_remote: 0,
            remote_resets: 0,
            contacts: VecDeque::new(),
            contact_cursor: MessageCursor::default(),
            contacts_surfaced: 0,
        }
    }
}

impl OwnPrediction {
    /// Number of recorded post-step states.
    pub fn depth(&self) -> usize {
        self.ring.len()
    }

    /// The recorded command range `(oldest, newest)`.
    pub fn span(&self) -> Option<(u64, u64)> {
        Some((self.ring.oldest()?, self.ring.newest()?))
    }

    /// Record the world after the step driven by command `seq`, as the
    /// `record_prediction` system does each fixed tick. Public so a test
    /// that drives the app's own systems by hand can fill the history.
    pub fn record(&mut self, seq: u64, capture: WorldCapture) {
        self.ring.record(seq, capture);
    }

    /// Note the contact starts the live world raised since the last call,
    /// under the `seq` of the step that raised them. The `record_prediction`
    /// system calls it each fixed tick, after the physics step.
    pub fn log_contacts(&mut self, world: &World, seq: u64) {
        let Some(messages) = world.get_resource::<Messages<CollisionStart>>() else {
            return;
        };
        for pair in self.contact_cursor.read(messages).filter_map(contact_pair) {
            self.contacts.push_back((seq, pair));
        }
        if let Some(oldest) = self.ring.oldest() {
            while self.contacts.front().is_some_and(|(s, _)| *s < oldest) {
                self.contacts.pop_front();
            }
        }
    }

    /// Match the contact starts a replay of `seqs` (the commands
    /// `ack+1..=newest`) found against the ones already raised, and return
    /// the ones the live world had not raised. `born` is the replay's
    /// `(step, start)` list.
    ///
    /// A replayed start cancels one raised start of the same body pair that
    /// is not yet behind the ack and lies within [`CONTACT_JITTER_TICKS`];
    /// the raised start then takes the earlier of the two ticks, because a
    /// contact the client found late is found earlier by each newer
    /// snapshot. A start at or before the ack is host-verified history: a
    /// later start of the same pair is a new contact (a bounce), not that
    /// one again. A raised start the replay no longer finds stays raised,
    /// it already played.
    fn new_contacts(
        &mut self,
        ack: u64,
        seqs: &[u64],
        born: &[(usize, CollisionStart)],
    ) -> Vec<CollisionStart> {
        let mut out = Vec::new();
        let mut matched: Vec<usize> = Vec::new();
        for (step, m) in born {
            let (Some(pair), Some(&tick)) = (contact_pair(m), seqs.get(*step)) else {
                continue;
            };
            let near = self
                .contacts
                .iter()
                .enumerate()
                .filter(|(i, (t, p))| {
                    *p == pair
                        && *t > ack
                        && t.abs_diff(tick) <= CONTACT_JITTER_TICKS
                        && !matched.contains(i)
                })
                .min_by_key(|(_, (t, _))| t.abs_diff(tick))
                .map(|(i, _)| i);
            if let Some(i) = near {
                matched.push(i);
                self.contacts[i].0 = self.contacts[i].0.min(tick);
            } else {
                out.push(*m);
                self.contacts.push_back((tick, pair));
                matched.push(self.contacts.len() - 1);
            }
        }
        out
    }

    fn restart(&mut self, generation: u64) {
        self.generation = generation;
        self.contacts.clear();
        self.ring.clear();
        self.last_ack = 0;
        self.floor = 0;
        self.deadline_start = None;
    }

    /// Host tick the recovery deadline started at, if it is running.
    pub fn deadline_start(&self) -> Option<u64> {
        self.deadline_start
    }
}

/// A remote contact body to compare, correct and replay with the own car.
#[derive(Debug, Clone)]
pub struct RemoteDrive {
    pub body: Entity,
    /// The host's state for it at the assertion's tick, if the snapshot had
    /// the seat.
    pub assertion: Option<RemoteAssertion>,
    /// Its command for each step `ack+1..=newest`, oldest first; `None`
    /// when any step's command is unknown (a [`RecoveryCause::CommandGap`]).
    pub commands: Option<Vec<VehicleInput>>,
}

/// Where a host assertion's reset epoch stands against the epoch the
/// contact body's history reflects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EpochState {
    /// The same epoch, or the body does not track one.
    Current,
    /// The host reset the seat since the body's history was recorded: the
    /// recorded past of this body is from before a teleport.
    Reset,
    /// The assertion predates the body's epoch: a stale state that must
    /// not be written back over a newer one.
    Behind,
}

/// `new` is ahead of `cur` on the wrapping `u8` epoch circle.
fn epoch_ahead(new: u8, cur: u8) -> bool {
    new != cur && new.wrapping_sub(cur) < 128
}

impl RemoteDrive {
    fn epoch_state(&self, world: &World) -> EpochState {
        let (Some(a), Some(body)) = (self.assertion, world.get::<PredictedRemoteBody>(self.body))
        else {
            return EpochState::Current;
        };
        if a.epoch == body.epoch {
            EpochState::Current
        } else if epoch_ahead(a.epoch, body.epoch) {
            EpochState::Reset
        } else {
            EpochState::Behind
        }
    }
}

/// A real (jointed) trailer body towed by the own car or a remote contact
/// body. It has no command of its own: the replay copies the tractor's
/// brake and handbrake onto it for each step, as
/// [`trailer_input`](crate::car_visual::trailer_input) does live.
pub struct TrailerDrive {
    pub body: Entity,
    /// The own car or the [`RemoteDrive::body`] pulling it.
    pub towing: Entity,
    /// The host's state for it at the assertion's tick, if the snapshot
    /// had its row (`wire` is the towing seat).
    pub assertion: Option<RemoteAssertion>,
}

/// Everything a replay needs besides the world.
pub struct Request<'a> {
    pub own: Entity,
    pub assertion: &'a OwnAssertion,
    /// Newest simulated command `seq`.
    pub newest: u64,
    /// The stored commands as `(seq, input)`, oldest first, starting at
    /// `assertion.ack + 1` when complete.
    pub commands: &'a [(u64, VehicleInput)],
    /// The remote contact bodies replayed together with the own car.
    pub remotes: &'a [RemoteDrive],
    /// The jointed trailers of the own car and of the remote bodies; they
    /// are restored, corrected and replayed with their tractors so the
    /// hitch never sees a car and a trailer from different ticks.
    pub trailers: &'a [TrailerDrive],
    pub epoch: EpochRef,
}

impl Request<'_> {
    /// Every host-asserted state other than the own car's: remote bodies
    /// first, then trailers.
    fn partners(&self) -> impl Iterator<Item = (Entity, RemoteAssertion)> + '_ {
        let remotes = self
            .remotes
            .iter()
            .filter_map(|r| Some((r.body, r.assertion?)));
        let trailers = self
            .trailers
            .iter()
            .filter_map(|t| Some((t.body, t.assertion?)));
        remotes.chain(trailers)
    }
}

/// The angle (rad) between two rotations. Not `Quat::angle_between`, which
/// is `2·acos(dot)` in f32: a unit quaternion whose squared length rounds
/// to 1 - 6e-8 has a dot with itself that reads as ~7e-4 rad, so identical
/// rotations could pass [`ROT_TOLERANCE`] on one machine and fail it on
/// another and cause a replay of a world that agreed. The relative
/// rotation's vector part, sin(θ/2), stays precise for tiny angles.
fn rotation_error(a: Quat, b: Quat) -> f32 {
    let d = a.inverse() * b;
    2.0 * d.xyz().length().atan2(d.w.abs())
}

fn finite3(v: Vec3) -> bool {
    v.is_finite()
}

fn valid(a: &OwnAssertion) -> bool {
    finite3(a.pos)
        && finite3(a.vel)
        && finite3(a.ang)
        && a.rot.is_finite()
        && (a.rot.length_squared() - 1.0).abs() < 0.05
}

fn own_body(capture: &WorldCapture, own: Entity) -> Option<(Vec3, Quat, Vec3, Vec3)> {
    let (_, vc) = capture.vehicles.iter().find(|(e, _)| *e == own)?;
    let b = &vc.body;
    Some((
        Vec3::from(b.position),
        Quat::from_array(b.rotation),
        Vec3::from(b.linear_velocity),
        Vec3::from(b.angular_velocity),
    ))
}

/// Compare the host's `assertion` with the recorded state at its ack and,
/// when they differ, correct by restoring and replaying. The one place the
/// own car's history is rewritten.
pub fn reconcile(world: &mut World, pred: &mut OwnPrediction, req: &Request<'_>) -> Outcome {
    let a = req.assertion;
    // A remote assertion from before the body's epoch is dropped from the
    // request, so it can neither be compared nor written back; one from a
    // later epoch is a teleport the recorded past does not contain.
    let mut resets = Vec::new();
    let mut stale = 0;
    let remotes: Vec<RemoteDrive> = req
        .remotes
        .iter()
        .map(|r| {
            let mut r = r.clone();
            match r.epoch_state(world) {
                EpochState::Current => {}
                EpochState::Reset => resets.push(r.body),
                EpochState::Behind => {
                    r.assertion = None;
                    stale += 1;
                }
            }
            r
        })
        .collect();
    pred.stale_remote += stale;
    let req = &Request {
        remotes: &remotes,
        ..*req
    };
    let outcome = classify_and_run(world, pred, req, &resets);
    if matches!(
        outcome,
        Outcome::Matched | Outcome::Replayed { .. } | Outcome::Recovered(_)
    ) {
        // The reset is part of the corrected history now (or the body was
        // adopted outright): later assertions of that epoch compare as usual.
        for r in req.remotes.iter().filter(|r| resets.contains(&r.body)) {
            if let (Some(a), Some(mut body)) =
                (r.assertion, world.get_mut::<PredictedRemoteBody>(r.body))
            {
                body.epoch = a.epoch;
                pred.remote_resets += 1;
            }
        }
    }
    match outcome {
        Outcome::Skipped(_) => pred.skipped += 1,
        Outcome::Matched => {
            pred.compared += 1;
            pred.matched += 1;
            pred.last_ack = a.ack;
        }
        Outcome::Replayed {
            steps,
            shift,
            turn,
            remote_diverged,
            ..
        } => {
            pred.compared += 1;
            pred.replays += 1;
            pred.remote_replays += u64::from(remote_diverged);
            pred.replayed_steps += steps as u64;
            pred.shift_total += shift;
            pred.turn_total += turn;
            pred.last_ack = a.ack;
        }
        Outcome::Recovered(cause) => {
            pred.compared += 1;
            pred.recoveries += 1;
            match cause {
                RecoveryCause::HistoryExhausted => pred.recovered_exhausted += 1,
                RecoveryCause::TooDeep => pred.recovered_too_deep += 1,
                RecoveryCause::CommandGap => pred.recovered_gap += 1,
                RecoveryCause::ReplayRefused => pred.recovered_refused += 1,
                RecoveryCause::DeadlineExceeded => pred.recovered_deadline += 1,
            }
            pred.deadline_start = None;
            pred.last_ack = a.ack;
        }
        Outcome::Reset => {}
    }
    pred.last = Some(outcome);
    outcome
}

fn classify_and_run(
    world: &mut World,
    pred: &mut OwnPrediction,
    req: &Request<'_>,
    resets: &[Entity],
) -> Outcome {
    let a = req.assertion;
    if a.generation != pred.generation {
        return Outcome::Skipped(Skip::OtherGeneration);
    }
    if !valid(a) {
        return Outcome::Skipped(Skip::Corrupt);
    }
    if a.ack == 0 {
        return Outcome::Skipped(Skip::NoAck);
    }
    if a.ack <= pred.last_ack {
        return Outcome::Skipped(Skip::Seen);
    }
    if a.ack > req.newest {
        return Outcome::Skipped(Skip::Future);
    }
    if a.ack < pred.floor {
        return Outcome::Skipped(Skip::PreReset);
    }
    let Some(past) = pred.ring.get(a.ack) else {
        adopt(world, req);
        pred.ring.clear();
        pred.floor = req.newest + 1;
        return Outcome::Recovered(RecoveryCause::HistoryExhausted);
    };
    let Some((pos, rot, vel, ang)) = own_body(past, req.own) else {
        adopt(world, req);
        pred.ring.clear();
        pred.floor = req.newest + 1;
        return Outcome::Recovered(RecoveryCause::ReplayRefused);
    };
    let pos_error = pos.distance(a.pos);
    let rot_error = rotation_error(rot, a.rot);
    let own_agrees = pos_error <= POS_TOLERANCE
        && rot_error <= ROT_TOLERANCE
        && vel.distance(a.vel) <= VEL_TOLERANCE
        && ang.distance(a.ang) <= ANG_TOLERANCE;
    let (remote_error, mut remote_diverged, mut lasting) = compare_partners(past, req);
    // A seat the host reset since the body's history was recorded is a
    // teleport inside it: replay even when the poses happen to be close.
    let reset_seen = resets.iter().any(|b| in_capture(past, *b));
    remote_diverged |= reset_seen;
    lasting |= reset_seen;
    // A seat that spawned after the compared tick has no recorded past to
    // compare: it sits at its spawn pose. Seed it from the host's state so
    // the replay steps it, and force the replay even if the own car agrees.
    let entrants = entrants(past, req);
    remote_diverged |= !entrants.is_empty();
    lasting |= !entrants.is_empty();
    let complete_state = partners_complete(past, req);
    let mut deadline = false;
    if lasting {
        // Only a usable complete state starts the clock; the contact tick
        // and a partial snapshot never do.
        if complete_state && pred.deadline_start.is_none() {
            pred.deadline_start = Some(a.server_tick);
        }
        deadline = pred
            .deadline_start
            .is_some_and(|t| a.server_tick.saturating_sub(t) >= RECOVERY_DEADLINE_TICKS);
    } else if complete_state {
        // Nothing disagrees beyond the replay residue: the clock stops.
        pred.deadline_start = None;
    }
    if own_agrees && !remote_diverged {
        return Outcome::Matched;
    }
    let depth = (req.newest - a.ack) as usize;
    let complete = req.commands.len() == depth
        && req
            .commands
            .iter()
            .zip(a.ack + 1..)
            .all(|((seq, _), want)| *seq == want)
        && req
            .remotes
            .iter()
            .filter(|r| in_capture(past, r.body) || entrants.contains(&r.body))
            .all(|r| r.commands.as_ref().is_some_and(|c| c.len() == depth));
    if !complete {
        adopt(world, req);
        pred.ring.clear();
        pred.floor = req.newest + 1;
        return Outcome::Recovered(RecoveryCause::CommandGap);
    }
    if depth > MAX_REPLAY_STEPS {
        adopt(world, req);
        pred.ring.clear();
        pred.floor = req.newest + 1;
        return Outcome::Recovered(RecoveryCause::TooDeep);
    }
    let outcome = replay(
        world,
        pred,
        req,
        (pos_error, rot_error),
        (remote_error, remote_diverged),
        &entrants,
        resets,
    );
    // The deadline gives up on the recorded past, not on the car's lead: the
    // host's state at the ack is stepped forward through the commands still
    // in flight, so the own car is not thrown back by lead x speed.
    if deadline && matches!(outcome, Outcome::Replayed { .. }) {
        return Outcome::Recovered(RecoveryCause::DeadlineExceeded);
    }
    outcome
}

/// Remote contact bodies that spawned after the compared tick (absent from
/// `past`) and have the host's state for it plus a command for every
/// replayed step. One without commands stays out, as before: the replay
/// cannot step it, and forcing a recovery for it would be worse.
fn entrants(past: &WorldCapture, req: &Request<'_>) -> Vec<Entity> {
    let depth = req.commands.len();
    req.remotes
        .iter()
        .filter(|r| {
            r.assertion.is_some()
                && !in_capture(past, r.body)
                && r.commands.as_ref().is_some_and(|c| c.len() == depth)
        })
        .map(|r| r.body)
        .collect()
}

/// Every contact body and trailer in the recorded world has its host row
/// in this snapshot (a stale-epoch row was already dropped from the
/// request, so it counts as missing).
fn partners_complete(past: &WorldCapture, req: &Request<'_>) -> bool {
    req.remotes
        .iter()
        .map(|r| (r.body, r.assertion))
        .chain(req.trailers.iter().map(|t| (t.body, t.assertion)))
        .all(|(body, assertion)| assertion.is_some() || !in_capture(past, body))
}

/// Put each captured vehicle's mass and inertia back to what the world
/// holds now. The load a Cops & Robbers carrier bears is a function of
/// the host's gold state at *this* moment, written by
/// `cnr::reconcile_gold_load` once per match change; a capture from
/// before the pickup (or before the drop) holds the other mass, and
/// restoring it would leave the predicted car lighter (or heavier)
/// than the host's copy for good, because the reconcile has nothing
/// left to change. The pose and velocity are the history's to rewrite;
/// the load is not part of that timeline.
fn keep_live_mass(world: &World, capture: &mut WorldCapture) {
    use avian3d::prelude::{AngularInertia, Mass};
    for (entity, vehicle) in &mut capture.vehicles {
        let Ok(live) = world.get_entity(*entity) else {
            continue;
        };
        if let (Some(mass), Some(inertia)) = (live.get::<Mass>(), live.get::<AngularInertia>()) {
            vehicle.body.mass = mass.0;
            vehicle.body.inertia_principal = inertia.principal.to_array();
        }
    }
}

fn in_capture(capture: &WorldCapture, e: Entity) -> bool {
    capture.vehicles.iter().any(|(v, _)| *v == e)
}

/// Compare each remote body and trailer the host reported with its
/// recorded state: the largest position difference, whether any tolerance
/// was passed (replay it) and whether the pose disagrees by a deadline
/// tolerance (the deadline clock runs). A body that is not in the recorded world (it
/// spawned after that tick) has nothing to compare.
fn compare_partners(past: &WorldCapture, req: &Request<'_>) -> (f32, bool, bool) {
    let mut worst = 0.0_f32;
    let mut diverged = false;
    let mut lasting = false;
    for (body, a) in req.partners() {
        let Some((pos, rot, vel, ang)) = own_body(past, body) else {
            continue;
        };
        let err = pos.distance(a.pos);
        worst = worst.max(err);
        lasting |=
            err > DEADLINE_POS_TOLERANCE || rotation_error(rot, a.rot) > DEADLINE_ROT_TOLERANCE;
        diverged |= err > POS_TOLERANCE
            || rotation_error(rot, a.rot) > ROT_TOLERANCE
            || vel.distance(a.vel) > VEL_TOLERANCE
            || ang.distance(a.ang) > ANG_TOLERANCE;
    }
    (worst, diverged, lasting)
}

/// The host's body state, written onto the own car.
fn adopt(world: &mut World, req: &Request<'_>) {
    let a = req.assertion;
    if let Ok(mut car) = world.get_entity_mut(req.own) {
        car.insert((
            Position(a.pos),
            Rotation(a.rot),
            LinearVelocity(a.vel),
            AngularVelocity(a.ang),
            Teleported,
        ));
    }
    // The remote contact bodies and the trailers take the host's state of
    // the same tick.
    for (e, a) in req.partners() {
        let Ok(mut body) = world.get_entity_mut(e) else {
            continue;
        };
        body.insert((
            Position(a.pos),
            Rotation(a.rot),
            LinearVelocity(a.vel),
            AngularVelocity(a.ang),
            Teleported,
        ));
    }
}

type Pose = (Entity, Vec3, Quat, Vec3, Vec3);

fn remote_poses(world: &mut World) -> Vec<Pose> {
    let mut q = world.query_filtered::<(
        Entity,
        &Position,
        &Rotation,
        &LinearVelocity,
        &AngularVelocity,
    ), With<RemoteReplica>>();
    q.iter(world)
        .map(|(e, p, r, v, w)| (e, p.0, r.0, v.0, w.0))
        .collect()
}

fn put_back(world: &mut World, poses: &[Pose]) {
    for (e, p, r, v, w) in poses {
        if let Ok(mut ent) = world.get_entity_mut(*e) {
            ent.insert((
                Position(*p),
                Rotation(*r),
                LinearVelocity(*v),
                AngularVelocity(*w),
            ));
        }
    }
}

fn present(world: &World, own: Entity) -> Option<(Vec3, Quat)> {
    Some((world.get::<Position>(own)?.0, world.get::<Rotation>(own)?.0))
}

fn replay(
    world: &mut World,
    pred: &mut OwnPrediction,
    req: &Request<'_>,
    (pos_error, rot_error): (f32, f32),
    (remote_error, remote_diverged): (f32, bool),
    entrants: &[Entity],
    resets: &[Entity],
) -> Outcome {
    let a = req.assertion;
    let mut corrected = pred.ring.get(a.ack).expect("checked by the caller").clone();
    keep_live_mass(world, &mut corrected);
    seed_entrants(world, req, entrants, &mut corrected);
    renew_reset_bodies(world, resets, &mut corrected);
    if let Some((_, vc)) = corrected.vehicles.iter_mut().find(|(e, _)| *e == req.own) {
        vc.body.position = a.pos.to_array();
        vc.body.rotation = a.rot.to_array();
        vc.body.linear_velocity = a.vel.to_array();
        vc.body.angular_velocity = a.ang.to_array();
    }
    for (body, a) in req.partners() {
        if let Some((_, vc)) = corrected.vehicles.iter_mut().find(|(e, _)| *e == body) {
            vc.body.position = a.pos.to_array();
            vc.body.rotation = a.rot.to_array();
            vc.body.linear_velocity = a.vel.to_array();
            vc.body.angular_velocity = a.ang.to_array();
        }
    }
    let before = present(world, req.own);
    let remotes = remote_poses(world);

    let mut staged: Vec<(u64, WorldCapture)> = Vec::with_capacity(req.commands.len());
    if req.commands.is_empty() {
        // The host's state is for the newest tick: nothing to step, but
        // the corrected capture still has to reach the world (the own car
        // and the reset or diverged bodies), or the history would record
        // a correction the world never took.
        let restore = restore_world(world, &corrected);
        put_back(world, &remotes);
        if !restore.refused.is_empty() || !restore.issues.is_empty() {
            adopt(world, req);
            pred.ring.clear();
            pred.floor = req.newest + 1;
            return Outcome::Recovered(RecoveryCause::ReplayRefused);
        }
        pred.ring.record(a.ack, corrected);
        return Outcome::Replayed {
            steps: 0,
            pos_error,
            rot_error,
            shift: 0.0,
            turn: 0.0,
            remote_error,
            remote_diverged,
        };
    }
    let steps: Vec<ReplayStep> = req
        .commands
        .iter()
        .enumerate()
        .map(|(i, (_, input))| {
            let mut inputs = vec![(req.own, *input)];
            inputs.extend(req.remotes.iter().filter_map(|r| {
                let command = r.commands.as_ref()?.get(i)?;
                in_capture(&corrected, r.body).then_some((r.body, *command))
            }));
            // A trailer brakes with its tractor, the way the live
            // `trailer_input` system copies it.
            for t in req.trailers {
                if !in_capture(&corrected, t.body) {
                    continue;
                }
                let tractor = if t.towing == req.own {
                    Some(*input)
                } else {
                    inputs.iter().find(|(e, _)| *e == t.towing).map(|(_, c)| *c)
                };
                if let Some(c) = tractor {
                    inputs.push((
                        t.body,
                        VehicleInput {
                            brake: c.brake,
                            handbrake: c.handbrake,
                            ..VehicleInput::default()
                        },
                    ));
                }
            }
            ReplayStep { inputs }
        })
        .collect();
    // One replay over the whole window, against the recorded history with
    // only the compared boundary corrected: the world actors (traffic
    // copies, props that spawned or moved) follow what was recorded for
    // every tick, not what they are doing now. Stepping one tick at a time
    // against a two-entry ring left them no history past the first step.
    // The permanent history is rewritten only when the whole replay
    // succeeded; every refusal clears it.
    let mut history = std::mem::take(&mut pred.ring);
    history.replace(a.ack, corrected.clone());
    let seqs: Vec<u64> = req.commands.iter().map(|(seq, _)| *seq).collect();
    let epoch = req.epoch;
    let result = replay_from_observed(world, &history, a.ack, &steps, &mut |world, i| {
        staged.push((seqs[i], capture_world(world, epoch)));
    });
    if result.is_err() {
        put_back(world, &remotes);
        adopt(world, req);
        pred.ring.clear();
        pred.floor = req.newest + 1;
        return Outcome::Recovered(RecoveryCause::ReplayRefused);
    }
    pred.ring = history;
    put_back(world, &remotes);
    // Contacts only the replay found: raise them in the live world now,
    // once. The quarantine swallowed them where they were born.
    if let Ok(report) = &result {
        let found = pred.new_contacts(a.ack, &seqs, &report.collision_starts);
        pred.contacts_surfaced += found.len() as u64;
        if let Some(mut live) = world.get_resource_mut::<Messages<CollisionStart>>() {
            for m in found {
                live.write(m);
            }
            // Already in the ledger: `log_contacts` must not note them again.
            pred.contact_cursor.read(&live).for_each(drop);
        }
    }
    pred.ring.record(a.ack, corrected);
    for (seq, capture) in staged {
        pred.ring.record(seq, capture);
    }
    let (shift, turn) = match (before, present(world, req.own)) {
        (Some((p0, r0)), Some((p1, r1))) => (p0.distance(p1), rotation_error(r0, r1)),
        _ => (0.0, 0.0),
    };
    Outcome::Replayed {
        steps: req.commands.len(),
        pos_error,
        rot_error,
        shift,
        turn,
        remote_error,
        remote_diverged,
    }
}

/// A contact body whose seat the host reset: its recorded drivetrain, tyre
/// and suspension state belongs to the life before the teleport, so the
/// corrected capture carries a fresh state instead (what the host's
/// `vehicle_reset` gave the seat). The body pose is overwritten with the
/// host's by the caller. How many ticks before the compared tick the
/// reset happened is not on the wire, so the fresh state is taken at the
/// compared tick: an inference, not a wire fact.
fn renew_reset_bodies(world: &World, resets: &[Entity], corrected: &mut WorldCapture) {
    for body in resets {
        let (Some(vehicle), Some((_, vc))) = (
            world.get::<Vehicle>(*body),
            corrected.vehicles.iter_mut().find(|(e, _)| e == body),
        ) else {
            continue;
        };
        let mut state = VehicleState::new(&vehicle.config);
        let (spins, completed) = (vc.gyro_spins, vc.gyro_completed);
        state.gyro_spins = spins;
        state.gyro_completed = completed;
        let input = vc.input;
        let fresh = VehicleCapture::from_parts(
            vc.epoch,
            vc.body,
            &VehicleInput::default(),
            PreStepVelocity::default(),
            vc.impairment.map(EngineImpairment),
            &state,
        );
        *vc = VehicleCapture { input, ..fresh };
    }
}

/// Put each entrant into the corrected capture, with its vehicle state as
/// it is now (a freshly spawned car) and the host's body state of the
/// compared tick, so the restore places it there and the replay steps it.
fn seed_entrants(
    world: &mut World,
    req: &Request<'_>,
    entrants: &[Entity],
    corrected: &mut WorldCapture,
) {
    if entrants.is_empty() {
        return;
    }
    let now = capture_world(world, req.epoch);
    for r in req.remotes.iter().filter(|r| entrants.contains(&r.body)) {
        let (Some(a), Some((_, vc))) =
            (r.assertion, now.vehicles.iter().find(|(e, _)| *e == r.body))
        else {
            continue;
        };
        let mut vc = vc.clone();
        vc.body.position = a.pos.to_array();
        vc.body.rotation = a.rot.to_array();
        vc.body.linear_velocity = a.vel.to_array();
        vc.body.angular_velocity = a.ang.to_array();
        corrected.vehicles.push((r.body, vc));
        if let Some(inc) = now.incarnations.iter().find(|(e, _)| *e == r.body) {
            corrected.incarnations.push(*inc);
        }
    }
    corrected.vehicles.sort_by_key(|(e, _)| *e);
    corrected.incarnations.sort();
}

fn own_entity(world: &mut World) -> Option<(Entity, u8)> {
    let mut q = world.query_filtered::<(Entity, Option<&ResetEpoch>), With<PlayerVehicle>>();
    let mut it = q.iter(world);
    let (e, epoch) = it.next()?;
    it.next().is_none().then_some((e, epoch.map_or(0, |e| e.0)))
}

fn gate(world: &World) -> bool {
    world
        .get_resource::<NetplayTimeline>()
        .is_some_and(|t| t.enabled)
        && world.contains_resource::<LobbyLink>()
        && world.resource::<Session>().is_playing()
}

fn epoch_ref(session: &Session, epoch: u8) -> EpochRef {
    EpochRef {
        generation: session.wire_generation() as u32,
        reset_epoch: u32::from(epoch),
    }
}

/// `FixedLast`: record the whole physics world after this tick's step,
/// under the `seq` of the command that drove it.
pub fn record_prediction(world: &mut World) {
    if !gate(world) {
        return;
    }
    let generation = world.resource::<Session>().wire_generation();
    let Some((_, epoch)) = own_entity(world) else {
        return;
    };
    let seq = world.resource::<CommandStream>().history().last_seq();
    let epoch = epoch_ref(world.resource::<Session>(), epoch);
    let mut pred = world.remove_resource::<OwnPrediction>().unwrap_or_default();
    if pred.generation != generation {
        pred.restart(generation);
    }
    if seq != 0 && pred.ring.newest().is_none_or(|n| seq > n) {
        let capture = capture_world(world, epoch);
        pred.record(seq, capture);
        pred.log_contacts(world, seq);
    }
    world.insert_resource(pred);
}

/// `FixedFirst`: apply the staged host assertion, between fixed steps.
pub fn reconcile_own(world: &mut World) {
    if !gate(world) {
        return;
    }
    let (assertion, remote_states, trailer_states, own_wire, reset) = {
        let Some(mut snaps) = world.get_resource_mut::<RemoteSnaps>() else {
            return;
        };
        let stage = snaps.own_stage_mut();
        let assertion = stage.assertion.take();
        let at = stage.remotes_at.take();
        let remotes = std::mem::take(&mut stage.remotes);
        let trailers = std::mem::take(&mut stage.trailers);
        let own_wire = stage.own_wire.take();
        // The seats and trailers only travel with the assertion of their
        // own snapshot.
        let (remotes, trailers) = match (assertion, at) {
            (Some(a), Some((g, t))) if a.generation == g && a.server_tick == t => {
                (remotes, trailers)
            }
            _ => (Vec::new(), Vec::new()),
        };
        (
            assertion,
            remotes,
            trailers,
            own_wire,
            std::mem::take(&mut stage.reset),
        )
    };
    let generation = world.resource::<Session>().wire_generation();
    let Some((own, epoch)) = own_entity(world) else {
        return;
    };
    let newest = world.resource::<CommandStream>().history().last_seq();
    let mut pred = world.remove_resource::<OwnPrediction>().unwrap_or_default();
    if pred.generation != generation {
        pred.restart(generation);
        crate::netvisual::clear_correction(world, own);
    }
    if reset {
        // A new life: the drawn pose must not slide from the old one.
        crate::netvisual::clear_correction(world, own);
        pred.ring.clear();
        pred.floor = newest + 1;
        pred.deadline_start = None;
        pred.resets += 1;
        pred.last = Some(Outcome::Reset);
        println!("event=own_reconcile kind=reset newest={newest}");
    }
    if let Some(assertion) = assertion {
        let commands: Vec<(u64, VehicleInput)> = world
            .resource::<CommandStream>()
            .history()
            .unacked()
            .filter(|r| r.seq > assertion.ack)
            .map(|r| (r.seq, decode_tick_command(&r.command)))
            .collect();
        let remotes = remote_drives(world, &assertion, &remote_states);
        let trailers = trailer_drives(world, own, own_wire, &trailer_states);
        let epoch = epoch_ref(world.resource::<Session>(), epoch);
        let before = present(world, own);
        let outcome = reconcile(
            world,
            &mut pred,
            &Request {
                own,
                assertion: &assertion,
                newest,
                commands: &commands,
                remotes: &remotes,
                trailers: &trailers,
                epoch,
            },
        );
        report(&assertion, outcome, before, present(world, own));
        // The body moved; what is drawn does not (it eases across). Only
        // the render path reads this, never `Position`/`Rotation`.
        if let (Some(b), Some(a)) = (before, present(world, own))
            && matches!(outcome, Outcome::Replayed { .. } | Outcome::Recovered(_))
        {
            crate::netvisual::absorb_correction(world, own, b, a);
        }
        // Everything up to the ack is inside the host's state: it can
        // never be replayed again.
        if matches!(
            outcome,
            Outcome::Matched | Outcome::Replayed { .. } | Outcome::Recovered(_)
        ) {
            world
                .resource_mut::<CommandStream>()
                .history_mut()
                .ack(assertion.ack);
        }
    }
    world.insert_resource(pred);
}

/// Every remote contact body with its host state (when the snapshot had
/// the seat) and the command it needs for each step after `assertion.ack`.
/// The client tick of a step is the `predicted_tick` of its own command.
fn remote_drives(
    world: &mut World,
    assertion: &OwnAssertion,
    states: &[RemoteAssertion],
) -> Vec<RemoteDrive> {
    let ticks: Vec<u64> = world
        .resource::<CommandStream>()
        .history()
        .unacked()
        .filter(|r| r.seq > assertion.ack)
        .map(|r| r.predicted_tick)
        .collect();
    // The snapshot's remote states are at host tick `server_tick`, the
    // state of the own seat after command `ack`; replay step `k` (0-based)
    // is therefore host tick `server_tick + 1 + k`, and the store is keyed
    // by host tick.
    if let Some(first) = ticks.first().copied()
        && let Some(mut feed) = world.get_resource_mut::<RemoteFeed>()
    {
        feed.set_offset(first, assertion.server_tick + 1);
    }
    let bodies: Vec<(Entity, u16)> = world
        .query::<(Entity, &PredictedRemoteBody)>()
        .iter(world)
        .map(|(e, b)| (e, b.wire))
        .collect();
    let Some(feed) = world.get_resource::<RemoteFeed>() else {
        return Vec::new();
    };
    bodies
        .into_iter()
        .map(|(body, wire)| RemoteDrive {
            body,
            assertion: states.iter().find(|s| s.wire == wire).copied(),
            commands: ticks
                .iter()
                .enumerate()
                .map(|(k, t)| {
                    feed.command_at(wire, *t, assertion.server_tick + 1 + k as u64)
                        .map(|c| decode_tick_command(&c))
                })
                .collect(),
        })
        .collect()
}

/// The real trailer bodies hitched to the own car or to a remote contact
/// body, each with its row of the snapshot (rows are keyed by the towing
/// seat's wire id). The predicted client's kinematic trailer copies
/// ([`RemoteTrailer`]) are wire-driven presentation and stay out.
fn trailer_drives(
    world: &mut World,
    own: Entity,
    own_wire: Option<u16>,
    states: &[RemoteAssertion],
) -> Vec<TrailerDrive> {
    let remote_wires: Vec<(Entity, u16)> = world
        .query::<(Entity, &PredictedRemoteBody)>()
        .iter(world)
        .map(|(e, b)| (e, b.wire))
        .collect();
    let mut q = world.query_filtered::<(Entity, &Trailer), Without<RemoteTrailer>>();
    q.iter(world)
        .filter_map(|(body, t)| {
            let wire = if t.towing == own {
                own_wire
            } else {
                remote_wires
                    .iter()
                    .find_map(|(e, w)| (*e == t.towing).then_some(*w))
            };
            let known = t.towing == own || wire.is_some();
            known.then(|| TrailerDrive {
                body,
                towing: t.towing,
                assertion: wire.and_then(|w| states.iter().find(|s| s.wire == w).copied()),
            })
        })
        .collect()
}

fn report(
    a: &OwnAssertion,
    outcome: Outcome,
    before: Option<(Vec3, Quat)>,
    after: Option<(Vec3, Quat)>,
) {
    let kind = match outcome {
        Outcome::Replayed { .. } => "replay",
        Outcome::Recovered(cause) => match cause {
            RecoveryCause::HistoryExhausted => "recover_history",
            RecoveryCause::TooDeep => "recover_depth",
            RecoveryCause::CommandGap => "recover_gap",
            RecoveryCause::ReplayRefused => "recover_refused",
            RecoveryCause::DeadlineExceeded => "recover_deadline",
        },
        _ => return,
    };
    let (Some((p0, _)), Some((p1, _))) = (before, after) else {
        return;
    };
    crate::netprobe::note_correction(kind, a.server_tick, p0, p1);
    println!(
        "event=own_reconcile kind={kind} tick={} ack={} shift={:.3}",
        a.server_tick,
        a.ack,
        p0.distance(p1)
    );
}

/// Register the history and the two fixed-schedule systems.
pub fn add_systems(app: &mut App) {
    app.init_resource::<OwnPrediction>()
        .add_systems(
            FixedFirst,
            // After the interpolation has put `Transform` back to last
            // tick's end (it would undo the reconcile's pose in `Transform`,
            // which avian then copies over `Position`, because the replay's
            // own physics steps are newer than the reconcile's write), and
            // before it records this tick's start.
            reconcile_own
                .after(avian3d::interpolation::TransformEasingSystems::Reset)
                .before(avian3d::interpolation::TransformEasingSystems::UpdateStart),
        )
        .add_systems(FixedLast, record_prediction);
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use avian3d::prelude::*;
    use bevy::time::TimeUpdateStrategy;
    use mm2_vehicle::capture::{capture_vehicle, restore_vehicle};
    use mm2_vehicle::{VehicleConfig, VehiclePlugin, VehicleProbeLayers, vehicle_bundle};

    use super::*;

    const EPOCH: EpochRef = EpochRef {
        generation: 1,
        reset_epoch: 0,
    };
    const GEN: u64 = 1;

    fn app() -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(AssetPlugin::default())
            .add_plugins(bevy::mesh::MeshPlugin)
            .add_plugins(bevy::gizmos::GizmoPlugin)
            .add_plugins(PhysicsPlugins::default())
            .insert_resource(Time::<Fixed>::from_hz(60.0))
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
                1.0 / 60.0,
            )))
            .insert_resource(Gravity(Vec3::NEG_Y * 9.81))
            .add_plugins(TransformPlugin)
            .add_plugins(VehiclePlugin)
            .insert_resource(VehicleProbeLayers(LayerMask(0b01)));
        app.finish();
        app.cleanup();
        let w = app.world_mut();
        w.spawn((
            RigidBody::Static,
            Collider::cuboid(600.0, 1.0, 600.0),
            Position(Vec3::new(0.0, -0.5, 0.0)),
            Transform::from_xyz(0.0, -0.5, 0.0),
        ));
        let car = w
            .spawn((
                vehicle_bundle(&VehicleConfig::default()),
                Position(Vec3::new(0.0, 1.2, 0.0)),
                Transform::from_xyz(0.0, 1.2, 0.0),
            ))
            .id();
        app.update();
        (app, car)
    }

    /// The scripted command of tick `seq`: a pedal-and-steering mix that
    /// changes several times, so a replay has real turns and brakes.
    fn command(seq: u64) -> VehicleInput {
        match seq {
            0..=8 => VehicleInput {
                throttle: 1.0,
                ..VehicleInput::default()
            },
            9..=14 => VehicleInput {
                throttle: 0.8,
                steering: 0.5,
                ..VehicleInput::default()
            },
            15..=19 => VehicleInput {
                brake: 0.7,
                steering: -0.4,
                ..VehicleInput::default()
            },
            _ => VehicleInput {
                throttle: 1.0,
                steering: 0.1,
                ..VehicleInput::default()
            },
        }
    }

    fn live(app: &mut App, car: Entity, seq: u64) {
        *app.world_mut().get_mut::<VehicleInput>(car).unwrap() = command(seq);
        app.update();
    }

    /// Run commands `1..=n`, recording the world after each.
    fn run(app: &mut App, car: Entity, pred: &mut OwnPrediction, n: u64) {
        for seq in 1..=n {
            live(app, car, seq);
            let cap = capture_world(app.world_mut(), EPOCH);
            pred.ring.record(seq, cap);
        }
    }

    fn fresh() -> OwnPrediction {
        let mut p = OwnPrediction::default();
        p.restart(GEN);
        p
    }

    fn assertion_from(pred: &OwnPrediction, car: Entity, ack: u64) -> OwnAssertion {
        let (pos, rot, vel, ang) = own_body(pred.ring.get(ack).unwrap(), car).unwrap();
        OwnAssertion {
            generation: GEN,
            server_tick: ack + 100,
            ack,
            pos,
            rot,
            vel,
            ang,
        }
    }

    fn commands(from: u64, to: u64) -> Vec<(u64, VehicleInput)> {
        (from..=to).map(|s| (s, command(s))).collect()
    }

    fn go(
        app: &mut App,
        pred: &mut OwnPrediction,
        car: Entity,
        a: &OwnAssertion,
        newest: u64,
    ) -> Outcome {
        let cmds = commands(a.ack + 1, newest);
        reconcile(
            app.world_mut(),
            pred,
            &Request {
                own: car,
                assertion: a,
                newest,
                commands: &cmds,
                remotes: &[],
                trailers: &[],
                epoch: EPOCH,
            },
        )
    }

    #[test]
    fn an_agreeing_snapshot_changes_nothing() {
        let (mut app, car) = app();
        let mut pred = fresh();
        run(&mut app, car, &mut pred, 30);
        let before = *app.world().get::<Position>(car).unwrap();
        let a = assertion_from(&pred, car, 20);
        assert_eq!(go(&mut app, &mut pred, car, &a, 30), Outcome::Matched);
        assert_eq!(*app.world().get::<Position>(car).unwrap(), before);
        assert_eq!((pred.matched, pred.replays), (1, 0));
    }

    /// AC2: a divergent historical state, corrected after several pending
    /// turns and brakes, equals the same authoritative state run through
    /// those inputs — not the un-replayed host pose — and the history is
    /// rewritten so the next snapshot compares against the corrected past.
    #[test]
    fn a_divergent_past_is_replayed_through_the_pending_commands() {
        let (mut app, car) = app();
        let mut pred = fresh();
        run(&mut app, car, &mut pred, 30);
        let mut a = assertion_from(&pred, car, 8);
        a.pos += Vec3::new(0.4, 0.0, -0.3);
        a.vel += Vec3::new(1.5, 0.0, 0.0);
        let wrong_present = app.world().get::<Position>(car).unwrap().0;
        let Outcome::Replayed { steps, .. } = go(&mut app, &mut pred, car, &a, 30) else {
            panic!("expected a replay, got {:?}", pred.last);
        };
        assert_eq!(steps, 22);
        let corrected = app.world().get::<Position>(car).unwrap().0;
        assert_ne!(corrected, wrong_present);

        // Reference: the same host state, run live through the same
        // commands in an independent world.
        let (mut twin, tcar) = self::app();
        let mut scratch = fresh();
        run(&mut twin, tcar, &mut scratch, 8);
        let mut cap = capture_vehicle(twin.world(), tcar, EPOCH).unwrap();
        cap.body.position = a.pos.to_array();
        cap.body.linear_velocity = a.vel.to_array();
        restore_vehicle(twin.world_mut(), tcar, &cap).unwrap();
        for seq in 9..=30 {
            live(&mut twin, tcar, seq);
        }
        let expected = twin.world().get::<Position>(tcar).unwrap().0;
        assert!(
            corrected.distance(expected) < 1e-3,
            "replayed {corrected} vs reference {expected}"
        );

        // The history was rewritten: newest entry is the corrected present.
        let (p, ..) = own_body(pred.ring.get(30).unwrap(), car).unwrap();
        assert!(p.distance(corrected) < 1e-3);
        // And a later snapshot of the corrected past now matches.
        let again = assertion_from(&pred, car, 25);
        assert_eq!(go(&mut app, &mut pred, car, &again, 30), Outcome::Matched);
    }

    /// NETPLAY-08B: the gold load is the host's gold state *now*, not part
    /// of the replayed past. A replay from before the pickup must leave
    /// the carrier as heavy as the live world holds it, and a replay from
    /// before the drop must leave it unloaded.
    #[test]
    fn a_replay_keeps_the_carriers_current_load() {
        const LOAD: f32 = 250.0;
        let (mut app, car) = app();
        let mut pred = fresh();
        run(&mut app, car, &mut pred, 12);
        let base = app.world().get::<Mass>(car).unwrap().0;
        let base_inertia = app.world().get::<AngularInertia>(car).unwrap().principal;
        // The pickup lands live at tick 12; the history before it is light.
        let loaded = |app: &mut App| {
            app.world_mut().get_mut::<Mass>(car).unwrap().0 = base + LOAD;
            app.world_mut()
                .get_mut::<AngularInertia>(car)
                .unwrap()
                .principal = base_inertia * ((base + LOAD) / base);
        };
        loaded(&mut app);
        for seq in 13..=30 {
            live(&mut app, car, seq);
            let cap = capture_world(app.world_mut(), EPOCH);
            pred.ring.record(seq, cap);
        }
        let mut a = assertion_from(&pred, car, 8);
        a.pos += Vec3::new(0.4, 0.0, -0.3);
        assert!(matches!(
            go(&mut app, &mut pred, car, &a, 30),
            Outcome::Replayed { .. }
        ));
        assert_eq!(app.world().get::<Mass>(car).unwrap().0, base + LOAD);
        let want = base_inertia * ((base + LOAD) / base);
        assert!(
            app.world()
                .get::<AngularInertia>(car)
                .unwrap()
                .principal
                .distance(want)
                < 1e-3
        );

        // The drop lands live: the load is gone; a replay across it from
        // a heavy capture must not bring the load back.
        app.world_mut().get_mut::<Mass>(car).unwrap().0 = base;
        app.world_mut()
            .get_mut::<AngularInertia>(car)
            .unwrap()
            .principal = base_inertia;
        let mut b = assertion_from(&pred, car, 20);
        b.pos += Vec3::new(0.0, 0.0, 0.5);
        assert!(matches!(
            go(&mut app, &mut pred, car, &b, 30),
            Outcome::Replayed { .. }
        ));
        assert_eq!(app.world().get::<Mass>(car).unwrap().0, base);
    }

    /// AC3: a duplicated, older or starved ack never applies twice.
    #[test]
    fn duplicate_and_older_acks_are_skipped() {
        let (mut app, car) = app();
        let mut pred = fresh();
        run(&mut app, car, &mut pred, 30);
        let mut a = assertion_from(&pred, car, 12);
        a.pos.x += 1.0;
        assert!(matches!(
            go(&mut app, &mut pred, car, &a, 30),
            Outcome::Replayed { .. }
        ));
        let at = app.world().get::<Position>(car).unwrap().0;
        assert_eq!(
            go(&mut app, &mut pred, car, &a, 30),
            Outcome::Skipped(Skip::Seen)
        );
        let mut older = assertion_from(&pred, car, 10);
        older.pos.x += 5.0;
        assert_eq!(
            go(&mut app, &mut pred, car, &older, 30),
            Outcome::Skipped(Skip::Seen)
        );
        assert_eq!(app.world().get::<Position>(car).unwrap().0, at);
        assert_eq!((pred.replays, pred.skipped), (1, 2));
    }

    #[test]
    fn unusable_snapshots_are_skipped_without_touching_the_car() {
        let (mut app, car) = app();
        let mut pred = fresh();
        run(&mut app, car, &mut pred, 30);
        let at = app.world().get::<Position>(car).unwrap().0;
        let good = assertion_from(&pred, car, 20);
        let cases = [
            (OwnAssertion { ack: 0, ..good }, Skip::NoAck),
            (OwnAssertion { ack: 31, ..good }, Skip::Future),
            (
                OwnAssertion {
                    pos: Vec3::new(f32::NAN, 0.0, 0.0),
                    ..good
                },
                Skip::Corrupt,
            ),
            (
                OwnAssertion {
                    generation: GEN + 1,
                    ..good
                },
                Skip::OtherGeneration,
            ),
        ];
        for (a, why) in cases {
            assert_eq!(go(&mut app, &mut pred, car, &a, 30), Outcome::Skipped(why));
        }
        assert_eq!(app.world().get::<Position>(car).unwrap().0, at);
    }

    /// An authority reset forgets the history; the ack of a command from
    /// before it can no longer be compared or replayed across the map.
    #[test]
    fn nothing_replays_across_a_reset() {
        let (mut app, car) = app();
        let mut pred = fresh();
        run(&mut app, car, &mut pred, 30);
        pred.ring.clear();
        pred.floor = 31;
        let mut a = assertion_from(
            &{
                let mut p = fresh();
                run(&mut app, car, &mut p, 1);
                p
            },
            car,
            1,
        );
        a.ack = 12;
        a.pos.x += 50.0;
        assert_eq!(
            go(&mut app, &mut pred, car, &a, 31),
            Outcome::Skipped(Skip::PreReset)
        );
    }

    /// Exhausted history takes its own counted recovery and says so.
    #[test]
    fn a_state_older_than_the_history_is_a_named_recovery() {
        let (mut app, car) = app();
        let mut pred = fresh();
        run(&mut app, car, &mut pred, 30);
        let mut a = assertion_from(&pred, car, 20);
        a.ack = 10;
        pred.ring = HistoryRing::new(10);
        run(&mut app, car, &mut pred, 0);
        let cap = capture_world(app.world_mut(), EPOCH);
        pred.ring.record(30, cap);
        let out = go(&mut app, &mut pred, car, &a, 30);
        assert_eq!(out, Outcome::Recovered(RecoveryCause::HistoryExhausted));
        assert_eq!(app.world().get::<Position>(car).unwrap().0, a.pos);
        assert!(app.world().get::<Teleported>(car).is_some());
        assert_eq!(
            (pred.recoveries, pred.recovered_exhausted, pred.replays),
            (1, 1, 0)
        );
    }

    #[test]
    fn a_missing_stored_command_is_a_named_recovery() {
        let (mut app, car) = app();
        let mut pred = fresh();
        run(&mut app, car, &mut pred, 30);
        let mut a = assertion_from(&pred, car, 10);
        a.pos.z += 1.0;
        let mut cmds = commands(11, 30);
        cmds.remove(4);
        let out = reconcile(
            app.world_mut(),
            &mut pred,
            &Request {
                own: car,
                assertion: &a,
                newest: 30,
                commands: &cmds,
                remotes: &[],
                trailers: &[],
                epoch: EPOCH,
            },
        );
        assert_eq!(out, Outcome::Recovered(RecoveryCause::CommandGap));
    }

    // ---- Two cars on one tick (NETPLAY-05A-4) ----

    /// The remote car's scripted command of client tick `seq`: its own mix
    /// of turns and brakes, different from the own car's.
    fn remote_command(seq: u64) -> VehicleInput {
        match seq {
            0..=5 => VehicleInput {
                throttle: 0.6,
                ..VehicleInput::default()
            },
            6..=16 => VehicleInput {
                throttle: 1.0,
                steering: -0.5,
                ..VehicleInput::default()
            },
            _ => VehicleInput {
                brake: 0.5,
                steering: 0.2,
                ..VehicleInput::default()
            },
        }
    }

    /// A world with the own car and a second vehicle (the remote contact
    /// body) close enough to the first that the pair can touch.
    fn two_cars() -> (App, Entity, Entity) {
        let (mut app, own) = app();
        let remote = app
            .world_mut()
            .spawn((
                vehicle_bundle(&VehicleConfig::default()),
                Position(Vec3::new(2.6, 1.2, 4.0)),
                Transform::from_xyz(2.6, 1.2, 4.0),
            ))
            .id();
        app.update();
        (app, own, remote)
    }

    fn live2(app: &mut App, own: Entity, remote: Entity, seq: u64) {
        *app.world_mut().get_mut::<VehicleInput>(own).unwrap() = command(seq);
        *app.world_mut().get_mut::<VehicleInput>(remote).unwrap() = remote_command(seq);
        app.update();
    }

    fn run2(app: &mut App, own: Entity, remote: Entity, pred: &mut OwnPrediction, n: u64) {
        for seq in 1..=n {
            live2(app, own, remote, seq);
            let cap = capture_world(app.world_mut(), EPOCH);
            pred.ring.record(seq, cap);
        }
    }

    fn remote_assertion_from(pred: &OwnPrediction, remote: Entity, ack: u64) -> RemoteAssertion {
        let (pos, rot, vel, ang) = own_body(pred.ring.get(ack).unwrap(), remote).unwrap();
        RemoteAssertion {
            wire: 2,
            epoch: 0,
            pos,
            rot,
            vel,
            ang,
        }
    }

    fn drive(
        remote: Entity,
        assertion: Option<RemoteAssertion>,
        from: u64,
        to: u64,
    ) -> RemoteDrive {
        RemoteDrive {
            body: remote,
            assertion,
            commands: Some((from..=to).map(remote_command).collect()),
        }
    }

    fn go2(
        app: &mut App,
        pred: &mut OwnPrediction,
        own: Entity,
        a: &OwnAssertion,
        remotes: &[RemoteDrive],
        newest: u64,
    ) -> Outcome {
        let cmds = commands(a.ack + 1, newest);
        reconcile(
            app.world_mut(),
            pred,
            &Request {
                own,
                assertion: a,
                newest,
                commands: &cmds,
                remotes,
                trailers: &[],
                epoch: EPOCH,
            },
        )
    }

    fn pose(app: &App, e: Entity) -> (Vec3, Quat, Vec3) {
        (
            app.world().get::<Position>(e).unwrap().0,
            app.world().get::<Rotation>(e).unwrap().0,
            app.world().get::<LinearVelocity>(e).unwrap().0,
        )
    }

    fn tracked(app: &mut App, remote: Entity, epoch: u8) {
        app.world_mut()
            .entity_mut(remote)
            .insert(PredictedRemoteBody {
                wire: 2,
                visual: Entity::PLACEHOLDER,
                epoch,
            });
    }

    fn body_epoch(app: &App, remote: Entity) -> u8 {
        app.world()
            .get::<PredictedRemoteBody>(remote)
            .unwrap()
            .epoch
    }

    /// The host reset the seat to where the recorded body already is: the
    /// poses agree, but the recorded past is from before a teleport, so
    /// the epoch alone forces one replay, and then the same assertion
    /// matches.
    #[test]
    fn a_newer_remote_epoch_forces_one_replay_even_when_the_poses_agree() {
        let (mut app, own, remote) = two_cars();
        tracked(&mut app, remote, 0);
        let mut pred = fresh();
        run2(&mut app, own, remote, &mut pred, 30);
        let a = assertion_from(&pred, own, 20);
        let mut ra = remote_assertion_from(&pred, remote, 20);
        ra.epoch = 1;
        let r = drive(remote, Some(ra), 21, 30);
        let out = go2(&mut app, &mut pred, own, &a, std::slice::from_ref(&r), 30);
        assert!(
            matches!(
                out,
                Outcome::Replayed {
                    remote_diverged: true,
                    ..
                }
            ),
            "{out:?}"
        );
        assert_eq!(body_epoch(&app, remote), 1);
        assert_eq!(pred.remote_resets, 1);
        // The next assertion of that epoch compares as usual.
        let a = assertion_from(&pred, own, 25);
        let mut ra = remote_assertion_from(&pred, remote, 25);
        ra.epoch = 1;
        let r = drive(remote, Some(ra), 26, 30);
        assert_eq!(
            go2(&mut app, &mut pred, own, &a, &[r], 30),
            Outcome::Matched
        );
    }

    /// A reset seat's recorded drivetrain belongs to the life before the
    /// teleport: the corrected history holds a fresh state for it, and
    /// the own car (no reset) keeps what it had.
    #[test]
    fn a_reset_remote_body_continues_from_a_fresh_state() {
        let (mut app, own, remote) = two_cars();
        tracked(&mut app, remote, 0);
        let mut pred = fresh();
        run2(&mut app, own, remote, &mut pred, 30);
        let speed_of = |pred: &OwnPrediction, e: Entity, tick: u64| {
            let past = pred.ring.get(tick).unwrap();
            past.vehicles
                .iter()
                .find(|(v, _)| *v == e)
                .unwrap()
                .1
                .forward_speed
        };
        let (own_before, remote_before) = (speed_of(&pred, own, 20), speed_of(&pred, remote, 20));
        assert!(remote_before.abs() > 0.1, "the body was driven");
        let a = assertion_from(&pred, own, 20);
        let mut ra = remote_assertion_from(&pred, remote, 20);
        ra.epoch = 1;
        let r = drive(remote, Some(ra), 21, 30);
        go2(&mut app, &mut pred, own, &a, std::slice::from_ref(&r), 30);
        assert_eq!(speed_of(&pred, remote, 20), 0.0);
        assert_eq!(speed_of(&pred, own, 20), own_before);
    }

    /// A remote state older than the body's epoch is dropped whatever its
    /// pose says: it can neither trigger a replay nor be written back over
    /// the reset.
    #[test]
    fn a_remote_state_from_before_the_reset_is_not_written_back() {
        let (mut app, own, remote) = two_cars();
        tracked(&mut app, remote, 3);
        let mut pred = fresh();
        run2(&mut app, own, remote, &mut pred, 30);
        let before = pose(&app, remote);
        let a = assertion_from(&pred, own, 20);
        let mut ra = remote_assertion_from(&pred, remote, 20);
        ra.epoch = 2;
        ra.pos += Vec3::new(30.0, 0.0, 0.0);
        let r = drive(remote, Some(ra), 21, 30);
        assert_eq!(
            go2(&mut app, &mut pred, own, &a, &[r], 30),
            Outcome::Matched
        );
        assert_eq!(pose(&app, remote), before);
        assert_eq!(body_epoch(&app, remote), 3);
        assert_eq!(pred.stale_remote, 1);
    }

    /// The epoch circle wraps: 0 is ahead of 255, and 255 is behind 0.
    #[test]
    fn remote_epochs_compare_on_the_wrapping_circle() {
        assert!(epoch_ahead(0, 255));
        assert!(!epoch_ahead(255, 0));
        assert!(!epoch_ahead(7, 7));
    }

    /// An assertion for the newest tick (nothing to step) still has to
    /// reach the world: the diverged remote body takes the host's pose.
    #[test]
    fn a_correction_with_nothing_to_step_reaches_the_world() {
        let (mut app, own, remote) = two_cars();
        tracked(&mut app, remote, 0);
        let mut pred = fresh();
        run2(&mut app, own, remote, &mut pred, 30);
        let a = assertion_from(&pred, own, 30);
        let mut ra = remote_assertion_from(&pred, remote, 30);
        ra.epoch = 1;
        ra.pos += Vec3::new(0.0, 0.0, 8.0);
        let want = ra.pos;
        let r = drive(remote, Some(ra), 31, 30);
        let out = go2(&mut app, &mut pred, own, &a, &[r], 30);
        assert!(matches!(out, Outcome::Replayed { steps: 0, .. }), "{out:?}");
        assert!(pose(&app, remote).0.distance(want) < 1e-3);
        assert_eq!(body_epoch(&app, remote), 1);
    }

    /// Both cars agreeing with the host change nothing, whatever the pair
    /// has done in between.
    #[test]
    fn two_agreeing_cars_change_nothing() {
        let (mut app, own, remote) = two_cars();
        let mut pred = fresh();
        run2(&mut app, own, remote, &mut pred, 30);
        let (before_o, before_r) = (pose(&app, own), pose(&app, remote));
        let a = assertion_from(&pred, own, 20);
        let r = drive(
            remote,
            Some(remote_assertion_from(&pred, remote, 20)),
            21,
            30,
        );
        assert_eq!(
            go2(&mut app, &mut pred, own, &a, &[r], 30),
            Outcome::Matched
        );
        assert_eq!((pose(&app, own), pose(&app, remote)), (before_o, before_r));
    }

    /// A remote body that alone disagrees with the host triggers the
    /// replay, and the result equals the twin world that took the host
    /// state of both cars at the common tick and ran the same commands.
    #[test]
    fn a_two_car_replay_equals_the_twin_world() {
        let (mut app, own, remote) = two_cars();
        let mut pred = fresh();
        run2(&mut app, own, remote, &mut pred, 30);
        let a = assertion_from(&pred, own, 8);
        let mut ra = remote_assertion_from(&pred, remote, 8);
        ra.pos += Vec3::new(-0.5, 0.0, 0.35);
        ra.vel += Vec3::new(0.0, 0.0, 1.2);
        let out = go2(
            &mut app,
            &mut pred,
            own,
            &a,
            &[drive(remote, Some(ra), 9, 30)],
            30,
        );
        let Outcome::Replayed {
            steps,
            remote_diverged,
            remote_error,
            ..
        } = out
        else {
            panic!("expected a replay, got {out:?}");
        };
        assert_eq!(steps, 22);
        assert!(remote_diverged && remote_error > 0.5);
        assert_eq!((pred.replays, pred.remote_replays), (1, 1));

        // Reference: the host state of both cars at tick 8, run live.
        let (mut twin, town, tremote) = two_cars();
        let mut scratch = fresh();
        run2(&mut twin, town, tremote, &mut scratch, 8);
        let mut cap = capture_vehicle(twin.world(), tremote, EPOCH).unwrap();
        cap.body.position = ra.pos.to_array();
        cap.body.linear_velocity = ra.vel.to_array();
        restore_vehicle(twin.world_mut(), tremote, &cap).unwrap();
        for seq in 9..=30 {
            live2(&mut twin, town, tremote, seq);
        }
        for (got, want) in [(own, town), (remote, tremote)] {
            let (g, w) = (pose(&app, got), pose(&twin, want));
            assert!(g.0.distance(w.0) < 1e-3, "position {} vs {}", g.0, w.0);
            assert!(g.2.distance(w.2) < 1e-2, "velocity {} vs {}", g.2, w.2);
        }
        // The history holds the corrected two-car past.
        let (p, ..) = own_body(pred.ring.get(30).unwrap(), remote).unwrap();
        assert!(p.distance(pose(&app, remote).0) < 1e-3);
        let again = assertion_from(&pred, own, 25);
        let r = drive(
            remote,
            Some(remote_assertion_from(&pred, remote, 25)),
            26,
            30,
        );
        assert_eq!(
            go2(&mut app, &mut pred, own, &again, &[r], 30),
            Outcome::Matched
        );
    }

    /// The remote body is not put back after the replay (its replayed
    /// state is the corrected one), while the delayed visual is.
    #[test]
    fn the_replayed_remote_body_stays_corrected_and_the_visual_is_put_back() {
        let (mut app, own, remote) = two_cars();
        let visual = app
            .world_mut()
            .spawn((
                vehicle_bundle(&VehicleConfig::default()),
                RemoteReplica,
                Position(Vec3::new(-6.0, 1.2, 4.0)),
                Transform::from_xyz(-6.0, 1.2, 4.0),
            ))
            .id();
        app.update();
        let mut pred = fresh();
        run2(&mut app, own, remote, &mut pred, 30);
        let visual_at = pose(&app, visual).0;
        let a = assertion_from(&pred, own, 10);
        let mut ra = remote_assertion_from(&pred, remote, 10);
        ra.pos.x += 1.0;
        let present = pose(&app, remote).0;
        let out = go2(
            &mut app,
            &mut pred,
            own,
            &a,
            &[drive(remote, Some(ra), 11, 30)],
            30,
        );
        assert!(matches!(out, Outcome::Replayed { .. }));
        assert!(pose(&app, remote).0.distance(present) > 0.5);
        assert_eq!(pose(&app, visual).0, visual_at);
    }

    /// A remote command that is unknown for a replayed step is the named
    /// command-gap recovery, which adopts the host state of both cars.
    #[test]
    fn an_unknown_remote_command_is_a_named_recovery_for_both_cars() {
        let (mut app, own, remote) = two_cars();
        let mut pred = fresh();
        run2(&mut app, own, remote, &mut pred, 30);
        let mut a = assertion_from(&pred, own, 10);
        a.pos.z += 1.0;
        let ra = remote_assertion_from(&pred, remote, 10);
        let mut r = drive(remote, Some(ra), 11, 30);
        r.commands = None;
        let out = go2(&mut app, &mut pred, own, &a, &[r], 30);
        assert_eq!(out, Outcome::Recovered(RecoveryCause::CommandGap));
        assert_eq!(pose(&app, own).0, a.pos);
        assert_eq!(pose(&app, remote).0, ra.pos);
        assert_eq!((pred.recoveries, pred.recovered_gap), (1, 1));
    }

    /// A stale history is the same bounded path for two cars as for one.
    #[test]
    fn a_two_car_state_older_than_the_history_is_a_named_recovery() {
        let (mut app, own, remote) = two_cars();
        let mut pred = fresh();
        run2(&mut app, own, remote, &mut pred, 30);
        let mut a = assertion_from(&pred, own, 20);
        a.ack = 10;
        let mut ra = remote_assertion_from(&pred, remote, 20);
        ra.pos.y += 0.1;
        pred.ring = HistoryRing::new(10);
        let cap = capture_world(app.world_mut(), EPOCH);
        pred.ring.record(30, cap);
        let out = go2(
            &mut app,
            &mut pred,
            own,
            &a,
            &[drive(remote, Some(ra), 11, 30)],
            30,
        );
        assert_eq!(out, Outcome::Recovered(RecoveryCause::HistoryExhausted));
        assert_eq!(pose(&app, remote).0, ra.pos);
        assert_eq!(pred.recovered_exhausted, 1);
    }

    /// A rotation compared with itself is no error, however its squared
    /// length rounds; `Quat::angle_between` reads ~1e-3 rad for this one.
    #[test]
    fn an_identical_rotation_has_no_error_whatever_its_norm_rounding() {
        let q = Quat::from_xyzw(0.0, 0.0, 0.0, 0.999_999_9);
        assert!(q.angle_between(q) > ROT_TOLERANCE, "the premise");
        assert!(rotation_error(q, q) < 1e-6);
        let r = Quat::from_rotation_y(0.01);
        assert!((rotation_error(Quat::IDENTITY, r) - 0.01).abs() < 1e-5);
        assert!((rotation_error(r, Quat::IDENTITY) - 0.01).abs() < 1e-5);
        assert!(rotation_error(r, -r) < 1e-6, "q and -q are one rotation");
    }
}

#[cfg(test)]
mod contact_tests;
#[cfg(test)]
mod deadline_tests;
#[cfg(test)]
mod multi_tests;
#[cfg(test)]
mod trailer_tests;
