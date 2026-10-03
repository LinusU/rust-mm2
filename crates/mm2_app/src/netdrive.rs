//! The session data plane (F25-A.1): host-authoritative remote driving.
//!
//! Clients stream quantized [`DriveInput`] frames up; the host simulates
//! every participant — its own seat plus each remote car — and broadcasts
//! [`Snap`](mm2_net::Message::Snap) pose snapshots down. In `mm2_game`
//! authority terms:
//!
//! - **Host** (`SessionAuthority::Host` → `AuthorityRole::Authority`):
//!   remote-driven cars are real dynamic participants whose
//!   [`VehicleInput`] is fed from the wire mailbox instead of local
//!   devices. They are stamped `PlayerControl::Remote` and resolved by
//!   the authority's rule pipeline like AI (F25-A.4): damage accrues
//!   against the authored record, stuck/water/out-of-bounds episodes
//!   recover in place, a wreck resets in place and repairs, and the
//!   smoke↔torque impairment applies. Every reset the authority performs
//!   bumps the seat's [`ResetEpoch`] (F25-A.5) — the counter rides each
//!   snapshot entry, so a teleport is a declared fact on the wire rather
//!   than a pose jump receivers must infer. Snapshot entries also carry
//!   a small presentation tail (protocol v7, F25-B): steering angle,
//!   mean grounded-wheel spin rate, mean suspension compression and the
//!   brake/reverse/grounded flags — enough for a remote copy's wheels
//!   to steer, spin and droop and its brake/reverse lights to work.
//!   The v8 tail adds the seat's authoritative *damage fraction*
//!   (F25-B): a copy's `VehicleDamage` carries the replicated total
//!   (never locally accumulated — F05 req 6), which is what its bound
//!   `VehicleSmoke` rig emits from. The v10 tail adds [`SnapImpact`]
//!   rows (F25-B): each authority-side [`ImpactEvent`] whose
//!   participants map to `NetPlayer` seats rides the next snapshot as
//!   one row per seat, and clients replay them through the
//!   [`RemoteImpact`] stream — per-impact point/normal/severity the
//!   damage fraction cannot carry — so remote cars spark, sound and
//!   splat their skin on every process (the copy binds the authored
//!   `TexelDamageRig` like a local pick, and the v8 byte's >0→0
//!   transition is the repair that clears it). The v11 tail adds the
//!   seat's breakaway bitmask (F25-B): [`SnapEntry::breaks`] carries
//!   the authority rig's detached-part set — `detach_breaks` sheds
//!   parts off remote seats on the authority like it does AI — and a
//!   copy's rig diffs the wire mask every snap: a set bit hides the
//!   intact node and claims a `BangerPool` fragment through the same
//!   [`crate::breakaway::spawn_break_fragment`] helper the authority
//!   uses (minus the impact kick — the wire carries the detach
//!   *state*, not the launch impulse), a cleared bit re-attaches the
//!   node and despawns the fragment — the authority's repair arriving
//!   as replicated state. The v13 tail adds [`SnapRace`] (F25-B): while
//!   the session runs an event the authority publishes its race phase,
//!   countdown remainder and clock on every snapshot, and a predicted
//!   client mirrors them — its own `advance_race` is authority-gated
//!   and never steps, so without the row a joined event's countdown
//!   would hold control forever. The v14 tail adds each seat's
//!   [`RaceProgress`] (F25-B) — lifecycle discriminant, resolution
//!   tick, `Ordered` counters, cleared-gate mask, evidence counters —
//!   so a predicted client mirrors every seat's standing and a
//!   terminal edge mints into its own `ResultLedger`, the local seat's
//!   edge ending `Playing → Results` exactly like a simulated finish.
//!   Because both producers a remote seat lives on gate on `Playing`,
//!   the authority defers its own `Playing → Results` until every
//!   `Remote` participant resolves or departs (see `advance_race`) —
//!   and this publisher then owes the wire exactly one `Results`-phase
//!   frame at the frozen transition tick, the terminal rows' only
//!   carrier.
//! - **Client** (`SessionAuthority::Remote` → `Predicted`): remote cars
//!   are kinematic copies blended between the two newest snapshots
//!   ([`RemoteLerp`]), marked [`RemoteReplica`] so the local sim never
//!   steps them — their `VehicleState`/`VehicleInput` carry the snapshot
//!   tail's replicated drive state for the wheel/glow visuals instead,
//!   and their `VehicleDamage` carries the replicated total the bound
//!   smoke rig emits from. The damage/stuck/recovery rule systems
//!   stay inert — the authority owns every outcome. Our own car keeps
//!   driving on local physics —
//!   snapshot entries naming our wire id apply only when their `epoch`
//!   advances: the authority teleported us, so the local pose snaps to
//!   the asserted state (F25-A.5). Between resets the local sim owns the
//!   seat — sub-epoch divergence stays local, which is what `Predicted`
//!   means; continuous drift correction is named later scope. The `R`
//!   key is the exception to a client's inertness: it sends a
//!   `ResetRequest` up (F25-B), and the host's granted answer arrives
//!   back as the own-seat epoch snap — the requester's car teleports to
//!   its grid seat without the local sim ever asserting the pose.
//!
//! Identities on the wire are the lobby's roster slots, never Bevy
//! entities: the host seat is wire id 0 (it never appears on the roster;
//! its pick rides `Start`), peers mint from 1. [`NetPlayer`] stamps that
//! identity on each participant entity — including the local car, so the
//! host's snapshots include its own pose and a client knows which entry
//! is itself.
//!
//! The lobby's humans share the session's start grid (F25-A.2): the
//! seat map — wire ids sorted ascending, the seated host's 0 included
//! — ranks every participant, and [`seat_pose`] resolves rank → pose
//! identically on every process (authored `_strtpnts` slot while the
//! race ships one, a designed fan-out past it). `load_session_world`
//! puts the local car on its seat through [`NetSeats`]; this module's
//! reconcile puts each remote car on its own.
//!
//! Everything here is loopback-scoped groundwork like the rest of F24/F25:
//! no lag compensation, no rematch/lobby-result lifecycle or bulk
//! late-joiner ledger sync (F26 owns those), and a remote copy's
//! breakaway fragment carries only the car's replicated motion — the
//! per-part launch impulse never rides the wire (named gaps, not
//! silent behavior).

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use avian3d::prelude::{
    AngularVelocity, LinearVelocity, Position, RigidBody, Rotation, TransformInterpolation,
};
use bevy::prelude::*;
use mm2_game::{
    Banger, BangerPhase, BangerPool, BangerStateChanged, BreakPartSpec, DamageSignals, DamageSpec,
    ImpactEvent, Mm2Vfs, ObjectId, ObjectIdentity, ParticipantState, Player, PlayerControl,
    PlayerVehicle, RaceDefinition, RacePhase, RaceProgress, RaceStarted, RaceState, RecoveryPolicy,
    ResultLedger, Session, SessionEntity, SessionOutcome, SessionPhase, SessionResult, SmokePolicy,
    SparkPolicy, StuckSpec, VehicleBreaks, VehicleDamage, VehicleRecovery, VehicleSmoke,
    VehicleSparks, VehicleStuck,
};
use mm2_net::{
    DriveInput, MAX_SNAP_IMPACTS, Message, RemoteInputs, SNAP_FLAG_BRAKE, SNAP_FLAG_GROUNDED,
    SNAP_FLAG_REVERSE, SnapEntry, SnapImpact, SnapRace, SnapTrailer, VehiclePick,
};
use mm2_vehicle::{
    DriveDirection, HandlingMetrics, RemoteReplica, ResetVehicle, Teleported, Vehicle,
    VehicleConfig, VehicleInput, VehicleState, vehicle_bundle,
};

use crate::banger::BangerMut;
use crate::breakaway::{self, BreakVisualMut};
use crate::car_visual;
use crate::input::{control_just_pressed, pad};
use crate::net::{HostLink, LobbyLink, LobbyState};
use crate::opponents::SPAWN_LIFT;
use crate::session::SpawnPoint;

/// How old the newest mailbox sample may be before a remote driver's
/// input reads as zero — a silent/stalled client should coast, not
/// keep the throttle it last sent. Sized generously for the loopback
/// scope (one missed input window is ~8 ms; this covers ~30).
pub const INPUT_STALE: Duration = Duration::from_millis(250);

/// Lateral spacing a designed fan-out puts between start slots a grid
/// does not author — no `_strtpnts` record at all, or more seated
/// humans than authored rows (designed; the authored grid itself is
/// the product of record, UNK-17).
const SEAT_STAGE_GAP: f32 = 4.0;

/// A snapshot correction larger than this snaps the remote copy to the
/// asserted pose instead of blending toward it (designed bound, F25-A.4
/// — spec req 4's bounded corrections). Inter-snapshot travel is at
/// most top-speed × the clamped arrival interval — well under 20 m.
/// Since F25-A.5 the authority's resets declare themselves through
/// [`ResetEpoch`]/`SnapEntry::epoch` — the bound stays as the catch-all
/// for teleports the epoch cannot describe (drift past the bound, a
/// pose written by hand).
const CORRECTION_SNAP_DIST: f32 = 20.0;

/// A granted reset request mutes further asks from the same seat for
/// this long (F25-B; designed — the original's networked reset rule is
/// unrecovered). The local `R` is human edge rate, but a wire ask can
/// arrive every socket read — unbounded grants would let one client
/// teleport-lock its own seat every frame. One per second is still an
/// immediate answer to a wedge.
pub const RESET_REQUEST_COOLDOWN: Duration = Duration::from_secs(1);

/// Client-side bound on replicated impact rows queued ahead of
/// [`apply_snapshots`] (protocol v10, F25-B). Snaps arrive off the
/// reader thread faster than the app applies them under load; the
/// queue is a backlog bound, not a reliability mechanism — past it the
/// oldest rows drop and count against [`NetDriveReport::impacts_dropped`].
const MAX_PENDING_IMPACTS: usize = 256;

/// Bound on the replicated-impact dedup window (protocol v10, F25-B):
/// `(seat, id)` pairs stay known long enough to suppress a duplicated
/// or reordered frame's rows, then retire FIFO — a seen-set that only
/// grows would itself be the leak.
const MAX_SEEN_IMPACTS: usize = 512;

/// The wire roster id this participant entity carries. `0` is the host
/// seat — the roster never lists it, but its `Start`-carried pick and
/// snapshot entries use it. Stamped on the local car too: the host's
/// snapshots include seat 0, and a client recognizes its own entry.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetPlayer(pub u16);

/// The roster pick a spawned remote participant was built from — a
/// mid-session `SetVehicle` rebroadcast that changes it respawns the
/// entity rather than leaving a stale shell. A remote trailer copy
/// carries its towing seat's pick, so a pick change respawns the whole
/// rig.
#[derive(Component, Debug, Clone, PartialEq)]
pub struct RemotePick(pub VehiclePick);

/// The towing seat's wire roster id on a spawned remote trailer —
/// `Snap`'s `trailers` list keys trailer rows by the *seat*, not a
/// fresh identity, so the copy reconciles against its owner's roster
/// presence and pick (F25-B, protocol v9). Stamped on both roles: on
/// the authority it marks which seat's snap the trailer rides; on a
/// client it marks the kinematic copy.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteTrailer {
    /// The towing participant's wire roster id (0 = the host seat).
    pub owner: u16,
}

/// The reset epoch this entity's pose currently reflects (F25-A.5).
/// Stamped `0` on every participant — the local car included — at
/// spawn/stamping time. On the authority, [`track_reset_epochs`] bumps
/// it once per [`ResetVehicle`] the rule pipeline lands on the entity,
/// and [`publish_snapshots`] carries it as `SnapEntry::epoch`; on a
/// predicted client it is the last applied epoch — a differing wire
/// value means the asserted pose is an authority teleport, not motion.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResetEpoch(pub u8);

/// Blend state on a `Predicted` remote copy: the pose it displayed when
/// the newest snapshot arrived, the pose that snapshot asserts, and the
/// session-time interval to blend across (the observed arrival gap —
/// snapshots pace the stream, so each blend takes as long as the gap
/// that produced it).
#[derive(Component, Debug)]
pub struct RemoteLerp {
    /// Displayed pose when the newest snapshot landed.
    pub from_pos: Vec3,
    /// Displayed rotation when the newest snapshot landed.
    pub from_rot: Quat,
    /// The newest snapshot's asserted pose.
    pub to_pos: Vec3,
    /// The newest snapshot's asserted rotation.
    pub to_rot: Quat,
    /// Session clock (`Time::elapsed_secs_f64`) at snapshot arrival.
    pub start: f64,
    /// Session clock to finish blending at — `start` + the observed
    /// inter-snapshot gap.
    pub end: f64,
}

/// The newest snapshot-asserted wheel spin rate on a predicted remote
/// copy, rad/s (F25-B). [`drive_remote_lerp`] integrates it into each
/// `WheelState::spin` — the same accumulation the sim performs on the
/// authority — so a copy's wheels visibly turn between snapshots. The
/// rate itself is replicated (mean grounded `vel_long / radius`), not
/// the angle: a stalled stream freezes the wheels rather than
/// extrapolating a stale pose.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct RemoteDrive {
    /// Newest asserted angular rate for every wheel, rad/s.
    pub spin_rate: f32,
}

/// A replicated participant impact delivered to a remote copy —
/// client-side presentation events minted by [`apply_snapshots`] from
/// `Snap.impacts` rows (protocol v10, F25-B). Presentation only: `point`
/// and `normal` position the effect, `severity` drives its strength, and
/// nothing here writes damage or physics — those stay on the v8 state
/// byte and the authority's own `ImpactEvent` stream. `entity` is the
/// resolved local remote copy; the receiver's own seat never gets one —
/// its predicted physics already rendered the hit through the local
/// stream, so replaying the wire row would double the effect.
#[derive(Message, Debug, Clone, Copy)]
pub struct RemoteImpact {
    /// The remote participant entity the impact presents on.
    pub entity: Entity,
    /// World-space contact point.
    pub point: Vec3,
    /// Outward contact normal from this participant's side of the hit.
    pub normal: Vec3,
    /// Relative impact speed, m/s — `ImpactEvent::severity`.
    pub severity: f32,
    /// The struck side's authored `dgBangerData` `AudioId` (protocol
    /// v12) — resolved on the authority at publish since the receiver
    /// cannot resolve a struck prop's `ObjectId` out of the
    /// authority's local id namespace. `0` is the catch-all: the
    /// world, another seat or a recordless body.
    pub audio_id: i64,
}

/// The client-side snapshot inbox: the newest `Snap` the lobby pump
/// drained, staged for [`apply_snapshots`]. Latest-wins like the host's
/// input mailbox — a backlog of poses is strictly worse than the newest
/// — on the *frame's own* `(generation, tick)`, not on arrival order:
/// a reordered or duplicated frame older than what is already staged
/// or applied keeps its impact rows but cannot displace the newer pose
/// (an old frame clobbering a newer pending one would roll a remote
/// copy back for a frame under a reorder recipe).
/// Impact rows are the exception: they are *events*, not state, so they
/// ride their own bounded `pending` queue — a superseded frame's poses
/// drop, its unreceived effects still land.
#[derive(Resource, Default)]
pub struct RemoteSnaps {
    latest: Option<Snap>,
    /// Session-clock arrival of `latest` — the lerp interval is the gap
    /// between consecutive arrivals.
    last_arrival: Option<f64>,
    /// The newest applied (generation, tick) — an older or equal frame
    /// is dropped; a snap from an older session can never apply, and a
    /// new generation resets the tick check.
    applied: Option<(u64, u64)>,
    /// Pose frames dropped stale at [`push`](Self::push) — a duplicated
    /// or reordered frame at or behind the staged/applied watermark.
    /// Folded into [`NetDriveReport`] by `apply_snapshots`, the only
    /// consumer that runs with one.
    stale: u64,
    /// Replicated impact rows awaiting `apply_snapshots`, FIFO; each
    /// carries its frame's generation for the apply-side staleness
    /// check.
    pending: VecDeque<(u64, SnapImpact)>,
    /// Per-seat repair ledger: the (generation, snap tick) of the
    /// newest `damage` byte `>0 → 0` transition the apply pass
    /// performed — the wire's repair signal. A pending impact row
    /// emitted at or before that tick predates the wipe on the
    /// authority (`SnapImpact::tick` is the host tick the
    /// `ImpactEvent` was emitted, never after its snap's publish
    /// tick), so the drain drops it rather than splatting *after* a
    /// wipe the authority ordered last. Only seats a snap actually
    /// named can record one, so the map stays roster-bounded.
    repaired: HashMap<u16, (u64, u64)>,
    /// Dedup window over `(generation, seat, id)` — a duplicated or
    /// reordered frame re-presents its rows; only the first lands. The
    /// generation rides the key so a new session's restarted id stream
    /// never collides with the last one's marks. Bounded by
    /// [`MAX_SEEN_IMPACTS`], retired FIFO through `seen_order`.
    seen: HashSet<(u64, u16, u64)>,
    /// Insertion order of `seen` for the bounded retire.
    seen_order: VecDeque<(u64, u16, u64)>,
    /// Rows dropped at push time — `pending` overflow — folded into
    /// [`NetDriveReport`] by `apply_snapshots`, the only consumer that
    /// runs with one. Stale-generation rows queue and drop at apply
    /// instead, where the session gate can count them.
    dropped: u64,
    /// The freshest race row the stream carried (protocol v13), staged
    /// apart from the pose watermark: while the authority's race
    /// counts down the session tick is frozen, so every `Snap` after
    /// the first reads stale on `(generation, tick)` — the row still
    /// has to move or a client's countdown would never visibly tick,
    /// let alone release. `apply_snapshots` consumes it on every run.
    race: Option<(u64, SnapRace)>,
    /// The staged row's [`race_order_key`] — the monotonic freshness
    /// the reorder recipe can otherwise regress (a deferred frame's
    /// row arrives *after* its successor's).
    race_key: Option<(u64, u8, u64)>,
    /// Race rows refused at push — a `phase` discriminant the wire
    /// cannot name — folded into [`NetDriveReport`] like `dropped`.
    /// Equal-or-older rows skip silently: state replication is
    /// idempotent, and a repeated row is not a drop.
    race_dropped: u64,
}

/// A staged snapshot frame.
struct Snap {
    generation: u64,
    tick: u64,
    entries: Vec<SnapEntry>,
    trailers: Vec<SnapTrailer>,
}

/// The `SnapRace.phase` encoding (protocol v13) — `mm2_net` carries the
/// discriminant opaque, so the `RacePhase` naming lives here with its
/// only consumer. Ranks order the lifecycle: a fresher row is a higher
/// rank, or the same rank further along.
const SNAP_PHASE_COUNTDOWN: u8 = 0;
const SNAP_PHASE_RUNNING: u8 = 1;
const SNAP_PHASE_COMPLETE: u8 = 2;

/// The `SnapEntry` v14 progress tail's `prog_state` encoding —
/// `ParticipantState`'s discriminant opaque on the wire, named here
/// with its only consumer like the `SnapRace` phases above.
const SNAP_PROG_AWAITING: u8 = 0;
const SNAP_PROG_RACING: u8 = 1;
const SNAP_PROG_FINISHED: u8 = 2;
const SNAP_PROG_TIMED_OUT: u8 = 3;

/// `RaceState` → the v13 `Snap.race` row (F25-B): the lifecycle
/// discriminant, the countdown remainder while counting, and the race
/// clock verbatim. Stale resources never encode — the caller filters.
fn encode_race(race: &RaceState) -> SnapRace {
    let (phase, countdown) = match race.phase {
        RacePhase::Countdown { remaining } => (SNAP_PHASE_COUNTDOWN, remaining),
        RacePhase::Running => (SNAP_PHASE_RUNNING, 0),
        RacePhase::Complete => (SNAP_PHASE_COMPLETE, 0),
    };
    SnapRace {
        phase,
        countdown,
        clock: race.clock,
    }
}

/// A wire row → `RacePhase` — `None` for a discriminant the consumer
/// cannot name (a peer speaking a wire we do not know).
fn race_phase(row: &SnapRace) -> Option<RacePhase> {
    match row.phase {
        SNAP_PHASE_COUNTDOWN => Some(RacePhase::Countdown {
            remaining: row.countdown,
        }),
        SNAP_PHASE_RUNNING => Some(RacePhase::Running),
        SNAP_PHASE_COMPLETE => Some(RacePhase::Complete),
        _ => None,
    }
}

/// The two `RaceProgress` borrows the race mirror needs — a
/// `ParamSet` because they overlap: `p0` walks *every* participant
/// for the countdown-release flip (seats and non-wire participants
/// alike), while `p1` resolves a `NetPlayer` seat's v14 row to its
/// entity inside `apply_snap_frame`'s seat loop.
type RaceProgressQueries<'w, 's> = bevy::ecs::system::ParamSet<
    'w,
    's,
    (
        Query<'w, 's, &'static mut RaceProgress>,
        Query<'w, 's, &'static mut RaceProgress, With<NetPlayer>>,
    ),
>;

/// The v13/v14 race rows' apply-side targets bundled as one param
/// (F25-B): `apply_snapshots` is at the system-parameter arity
/// ceiling, so the mirror's borrows ride together — the session's
/// [`RaceState`] (absent on a raceless session), the participants'
/// [`RaceProgress`] for both the release flip and the v14 per-seat
/// row mirror, the [`ResultLedger`] a replicated terminal edge
/// records into, and the [`RaceStarted`] writer for the one GO edge.
#[derive(bevy::ecs::system::SystemParam)]
pub struct RaceMirror<'w, 's> {
    /// The session's race resource the row mirrors into.
    race: Option<ResMut<'w, RaceState>>,
    /// See [`RaceProgressQueries`].
    progress: RaceProgressQueries<'w, 's>,
    /// The ledger a replicated terminal edge records into — the same
    /// dedup sink `advance_race` writes on the authority.
    ledger: ResMut<'w, ResultLedger>,
    /// The release event consumers observe for the unlock.
    started: MessageWriter<'w, RaceStarted>,
}

/// `RaceProgress` → the v14 per-seat progress tail on its
/// [`SnapEntry`] (F25-B): the lifecycle discriminant plus the
/// terminal state's resolution tick, the `Ordered` counters, the
/// cleared-gate mask and the evidence counters — verbatim state,
/// replicated like the v8 damage byte. `None` — a participant the
/// race does not track (or a session whose `RaceState` is stale —
/// the caller gates) — leaves the all-zero tail: "no progress"
/// reads as never-started, never a fabricated row.
fn encode_progress(progress: Option<&RaceProgress>, entry: &mut SnapEntry) {
    let Some(progress) = progress else { return };
    (entry.prog_state, entry.prog_ticks) = match progress.state {
        ParticipantState::AwaitingStart => (SNAP_PROG_AWAITING, 0),
        ParticipantState::Racing => (SNAP_PROG_RACING, 0),
        ParticipantState::Finished { race_ticks, .. } => (SNAP_PROG_FINISHED, race_ticks),
        ParticipantState::TimedOut { race_ticks, .. } => (SNAP_PROG_TIMED_OUT, race_ticks),
    };
    entry.prog_lap = progress.lap;
    entry.prog_next = progress.next.min(u32::MAX as usize) as u32;
    entry.prog_cleared = progress.cleared_mask();
    entry.prog_crossings = progress.crossings;
    entry.prog_route_clears = progress.route_clears;
}

/// The decoded v14 progress tail's lifecycle word — a `prog_state`
/// discriminant the wire cannot name reads `None` and drops counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SnapProgress {
    /// Awaiting the countdown's release.
    Awaiting,
    /// Racing.
    Racing,
    /// Finished at the carried race tick.
    Finished {
        /// The authority's finish tick.
        race_ticks: u64,
    },
    /// The deadline expired with objectives open.
    TimedOut {
        /// The authority's expiry tick — the limit's tick.
        race_ticks: u64,
    },
}

/// A [`SnapEntry`]'s v14 tail → [`SnapProgress`] — `None` for a
/// discriminant the consumer cannot name (a peer speaking a wire we
/// do not know).
fn decode_progress(entry: &SnapEntry) -> Option<SnapProgress> {
    match entry.prog_state {
        SNAP_PROG_AWAITING => Some(SnapProgress::Awaiting),
        SNAP_PROG_RACING => Some(SnapProgress::Racing),
        SNAP_PROG_FINISHED => Some(SnapProgress::Finished {
            race_ticks: entry.prog_ticks,
        }),
        SNAP_PROG_TIMED_OUT => Some(SnapProgress::TimedOut {
            race_ticks: entry.prog_ticks,
        }),
        _ => None,
    }
}

/// The v14 per-seat progress tail's receiving half (F25-B): mirror
/// the authority's `RaceProgress` counters verbatim and land the
/// terminal edge exactly like `advance_race` records it on the
/// authority — the minted [`SessionResult`] records into this
/// process's [`ResultLedger`] (deduped by identity), the
/// participant's state carries the minted id, and a resolved *local*
/// participant moves the session `Playing → Results` — UI-5's
/// local-resolution rule, mirrored. `snap_tick` is the authority's
/// session tick at the frame's mint — the same clock family the
/// authority stamps its own results' `tick` field on.
///
/// The wire is a predicted client's only progress truth —
/// `advance_race` is authority-gated. A locally-resolved seat keeps
/// its recorded state: an identical row is idempotent state (no
/// count, like the race row's), a conflicting one — a rewound
/// lifecycle or a different resolution — is a non-conforming
/// authority's word and drops counted rather than rewriting a
/// recorded result.
#[allow(clippy::too_many_arguments)] // the mirror genuinely threads the row, the session mint and the ledger
fn apply_progress(
    entry: &SnapEntry,
    player: &Player,
    progress: &mut RaceProgress,
    session: &mut Session,
    ledger: &mut ResultLedger,
    snap_tick: u64,
    report: &mut NetDriveReport,
) {
    let Some(wire) = decode_progress(entry) else {
        report.progress_dropped += 1;
        return;
    };
    // A recorded resolution is final — the local terminal edge
    // already minted and recorded this participant's result.
    let resolved = match &progress.state {
        ParticipantState::Finished { race_ticks, .. } => Some((*race_ticks, false)),
        ParticipantState::TimedOut { race_ticks, .. } => Some((*race_ticks, true)),
        _ => None,
    };
    if let Some((ticks, timed_out)) = resolved {
        let same = match wire {
            SnapProgress::Finished { race_ticks } => !timed_out && race_ticks == ticks,
            SnapProgress::TimedOut { race_ticks } => timed_out && race_ticks == ticks,
            _ => false,
        };
        if same {
            return;
        }
        report.progress_dropped += 1;
        return;
    }
    progress.apply_replicated(
        entry.prog_cleared,
        entry.prog_next as usize,
        entry.prog_lap,
        entry.prog_crossings,
        entry.prog_route_clears,
    );
    match wire {
        SnapProgress::Awaiting => progress.state = ParticipantState::AwaitingStart,
        SnapProgress::Racing => progress.state = ParticipantState::Racing,
        SnapProgress::Finished { .. } | SnapProgress::TimedOut { .. } => {
            // The terminal edge — mint and record exactly like
            // `advance_race` does: one result per participant per
            // generation, deduplicated by identity through the
            // ledger (the mint lives in this process's local id
            // namespace, like every id the replicated side holds).
            let id = session.mint_result_id(player.id);
            let outcome = match wire {
                SnapProgress::Finished { race_ticks } => SessionOutcome::Finished { race_ticks },
                SnapProgress::TimedOut { race_ticks } => SessionOutcome::TimedOut { race_ticks },
                _ => unreachable!(),
            };
            let result = SessionResult {
                id: id.clone(),
                tick: snap_tick,
                outcome,
            };
            if let Err(dup) = ledger.record(result) {
                warn!(duplicate = %dup, "replicated race result rejected");
            }
            progress.state = match wire {
                SnapProgress::Finished { race_ticks } => ParticipantState::Finished {
                    race_ticks,
                    result: id,
                },
                SnapProgress::TimedOut { race_ticks } => ParticipantState::TimedOut {
                    race_ticks,
                    result: id,
                },
                _ => unreachable!(),
            };
            // UI-5's local-resolution rule mirrored: the wire's word
            // that *our* seat resolved ends the local session's
            // `Playing`, exactly like a locally-simulated finish
            // does on the authority.
            if player.control == PlayerControl::Local && *session.phase() == SessionPhase::Playing {
                session
                    .transition(SessionPhase::Results)
                    .expect("Playing → Results is a legal transition");
            }
        }
    }
    report.progress_applied += 1;
}

/// The freshness key for a staged race row — `(generation, rank,
/// progress)`, strictly increasing along the authority's sequence:
/// ranks order the lifecycle and within a rank the progress runs
/// monotone (a countdown's *remaining* inverts — it counts down — and
/// the clock counts up). `None` for a discriminant `push` cannot
/// stage: a bogus rank would otherwise park the key above every legit
/// row and mute the mirror until the next generation.
fn race_order_key(generation: u64, row: &SnapRace) -> Option<(u64, u8, u64)> {
    let progress = match row.phase {
        SNAP_PHASE_COUNTDOWN => u64::from(u32::MAX - row.countdown),
        SNAP_PHASE_RUNNING | SNAP_PHASE_COMPLETE => row.clock,
        _ => return None,
    };
    Some((generation, row.phase, progress))
}

impl RemoteSnaps {
    /// Queue a received snapshot frame. Pose state is latest-wins on
    /// `(generation, tick)`: an incoming frame at or behind the staged
    /// or last-applied watermark is stale — it drops counted (a dup or
    /// a reorder's straggler) instead of displacing a newer pose. Its
    /// impact rows still queue below — events outlive their frame —
    /// and its race row still stages on its own key (race state moves
    /// while the frame's session tick is frozen, e.g. the whole
    /// countdown).
    pub fn push(
        &mut self,
        generation: u64,
        tick: u64,
        entries: Vec<SnapEntry>,
        trailers: Vec<SnapTrailer>,
        impacts: Vec<SnapImpact>,
        race: Option<SnapRace>,
    ) {
        let staged = self.latest.as_ref().map(|s| (s.generation, s.tick));
        let watermark = staged.into_iter().chain(self.applied).max();
        if watermark.is_some_and(|w| (generation, tick) <= w) {
            self.stale += 1;
        } else {
            self.latest = Some(Snap {
                generation,
                tick,
                entries,
                trailers,
            });
        }
        if let Some(race) = race {
            match race_order_key(generation, &race) {
                // A strictly newer row stages; an equal or regressed
                // one is the idempotent-state case — no count.
                Some(key) if self.race_key.is_none_or(|k| key > k) => {
                    self.race_key = Some(key);
                    self.race = Some((generation, race));
                }
                Some(_) => {}
                None => self.race_dropped += 1,
            }
        }
        for row in impacts {
            let key = (generation, row.seat, row.id);
            if !self.seen.insert(key) {
                continue;
            }
            self.seen_order.push_back(key);
            if self.seen_order.len() > MAX_SEEN_IMPACTS
                && let Some(old) = self.seen_order.pop_front()
            {
                self.seen.remove(&old);
            }
            if self.pending.len() >= MAX_PENDING_IMPACTS {
                self.pending.pop_front();
                self.dropped += 1;
            }
            self.pending.push_back((generation, row));
        }
    }

    /// Newest snapshot tick applied so far — for the record/tests.
    pub fn applied(&self) -> Option<(u64, u64)> {
        self.applied
    }

    /// End the stream's authority boundary: the staged frame, both
    /// watermarks, the per-seat repair ledger and the dedup window all
    /// describe the *ended* session's `(generation, tick)` sequence.
    /// `drive_lobby` calls this when the link dies and on every
    /// accepted `Start` — a *different* authority restarts its
    /// numbering (a fresh host process's first start mints generation
    /// 1), and the dead stream's watermark would stale-drop that
    /// restarted sequence forever. Still-queued impact rows die with
    /// the stream: they fold into `dropped`, the count the drain would
    /// have given them as foreign generations. `stale`/`dropped`
    /// themselves are report evidence awaiting the `apply_snapshots`
    /// fold, not stream state — they survive.
    pub fn reset(&mut self) {
        self.latest = None;
        self.last_arrival = None;
        self.applied = None;
        self.dropped += self.pending.len() as u64;
        self.pending.clear();
        self.repaired.clear();
        self.seen.clear();
        self.seen_order.clear();
        self.race = None;
        self.race_key = None;
    }
}

/// The client's outbound input counter — `seq` tags each sent sample so
/// a receiver can tell fresher from older on the sender's own clock.
#[derive(Resource, Default)]
pub struct InputSeq(u64);

/// Data-plane counters for the headless record's `net=` evidence and
/// the app-level tests: sent/applied inputs, sent/seen/applied
/// snapshots, and remote spawn/despawn reconciliations.
#[derive(Resource, Default)]
pub struct NetDriveReport {
    /// `Input` frames this client sent.
    pub inputs_sent: u64,
    /// Mailbox samples the host applied to a remote car.
    pub inputs_applied: u64,
    /// Mailbox reads zeroed for staleness or a wrong generation.
    pub inputs_staled: u64,
    /// `Snap` frames the host broadcast.
    pub snaps_sent: u64,
    /// Snapshot frames the client applied to its remote copies.
    pub snaps_applied: u64,
    /// Pose frames the client dropped stale at [`RemoteSnaps::push`] —
    /// a duplicated or reordered `Snap` at or behind the
    /// staged-or-applied `(generation, tick)` watermark. Its impact
    /// rows still queue; only the superseded pose drops.
    pub snaps_staled: u64,
    /// Remote participants currently spawned.
    pub remotes: usize,
    /// Reconciliation spawns over the session.
    pub spawned: u64,
    /// Reconciliation despawns over the session.
    pub despawned: u64,
    /// Authority resets observed this session — host: `ResetVehicle`
    /// landings that bumped a wire epoch; client: epoch-declared
    /// teleports applied to a copy or the own seat.
    pub resets: u64,
    /// Driver reset requests this client sent on `R`/pad — the
    /// predicted-session form of the local reset key (F25-B).
    pub requests_sent: u64,
    /// Requests the authority granted — each lands as a `ResetVehicle`
    /// the epoch tracker counts again in `resets` (host side).
    pub requests_granted: u64,
    /// Requests the authority dropped: a foreign or stale generation, a
    /// not-`Playing` phase, a seat with no spawned participant, or an
    /// ask inside [`RESET_REQUEST_COOLDOWN`] (host side).
    pub requests_dropped: u64,
    /// Wheel-spin radians driven into remote copies' `WheelState::spin`
    /// from the replicated rate (client side) — evidence the v7
    /// presentation tail visibly turned a copy's wheels.
    pub remote_spin: f64,
    /// Replicated damage totals written onto live `VehicleDamage`
    /// components (client side, F25-B v8 tail) — remote copies and the
    /// own seat both count; a participant with no authored damage
    /// record has no component to write and never counts.
    pub damage_synced: u64,
    /// `Snap.trailers` rows applied to a live trailer entity (client
    /// side, F25-B protocol v9) — remote copies and the own rig's
    /// trailer on a declared reset; a session with no trailered seats
    /// never counts.
    pub trailers_synced: u64,
    /// `Snap.impacts` rows broadcast (authority side, protocol v10,
    /// F25-B) — one per participant side of each authority
    /// [`ImpactEvent`], after the per-snapshot cap.
    pub impacts_sent: u64,
    /// `Snap.impacts` rows applied to a live remote participant's
    /// entity (client side, protocol v10) — emitted into the
    /// [`RemoteImpact`] stream for presentation consumers.
    pub impacts_applied: u64,
    /// Impact rows dropped: on the authority, rows over the
    /// per-snapshot cap; on the client, pending-queue overflow, stale
    /// generations, unsanitized rows, and rows whose seat has no live
    /// participant entity.
    pub impacts_dropped: u64,
    /// `SnapEntry.breaks` bitmask transitions applied on this client
    /// (protocol v11, F25-B): parts the wire newly reports off the
    /// rig — each hides its intact node and spawns its pooled
    /// fragment.
    pub breaks_detached: u64,
    /// Parts a clearing `breaks` bit put back on the rig — the
    /// authority's repair arriving as replicated state: the fragment
    /// despawns and the intact node shows.
    pub breaks_restored: u64,
    /// `Snap.race` rows applied to the session's `RaceState` (client
    /// side, protocol v13, F25-B): each fresher countdown tick, the
    /// release, the running clock and the `Complete` word land here.
    /// `0` on the authority and on raceless sessions.
    pub race_applied: u64,
    /// Race rows refused — a `phase` discriminant the wire cannot
    /// name at `push`, or at apply: a row for a session with no
    /// `RaceState`, a foreign generation, or a stale resource.
    /// Equal-or-older rows never reach here — idempotent state is
    /// not a drop.
    pub race_dropped: u64,
    /// `SnapEntry` v14 progress tails applied (client side, F25-B):
    /// a wire seat's replicated `RaceProgress` mirrored onto its
    /// participant — terminal edges minting a `SessionResult` into
    /// the local `ResultLedger` included. `0` on the authority and on
    /// seats the race does not track.
    pub progress_applied: u64,
    /// Progress tails refused — an unnamed `prog_state` discriminant,
    /// or a terminal row conflicting with a resolution this process
    /// already recorded (a non-conforming authority rewinding a
    /// lifecycle is refused, not believed).
    pub progress_dropped: u64,
}

/// A rotation off the wire, sanitized — a malformed-quaternion guard so
/// a corrupt packet can never poison the pose with NaNs (the wire
/// decoder bounds sizes but not math).
fn wire_quat(raw: [f32; 4]) -> Quat {
    let q = Quat::from_array(raw);
    if q.is_finite() && q.length_squared() > 1e-12 {
        q.normalize()
    } else {
        Quat::IDENTITY
    }
}

/// `VehicleInput` → a wire sample. Controls quantize to the `u8`/`i8`
/// fields; `forced_gear` is a local control command that never rides the
/// wire (the remote driver's own sim selects gears).
pub fn encode_input(input: &VehicleInput, generation: u64, seq: u64) -> DriveInput {
    let q8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    DriveInput {
        generation,
        seq,
        throttle: q8(input.throttle),
        brake: q8(input.brake),
        steer: (input.steering.clamp(-1.0, 1.0) * 127.0).round() as i8,
        handbrake: q8(input.handbrake),
    }
}

/// A wire sample → `VehicleInput`, the exact complement of
/// [`encode_input`].
pub fn decode_input(input: &DriveInput) -> VehicleInput {
    VehicleInput {
        throttle: input.throttle as f32 / 255.0,
        brake: input.brake as f32 / 255.0,
        steering: input.steer as f32 / 127.0,
        handbrake: input.handbrake as f32 / 255.0,
        ..VehicleInput::default()
    }
}

/// A receiver-side clamp on the replicated steer angle: milliradians
/// decode up to ±32 rad — far past any authored steering lock — so a
/// corrupt or hostile snap's cosmetic wheel turn is bounded.
const MAX_WIRE_STEER: f32 = 1.5;

/// `VehicleState`/`VehicleInput` → a [`SnapEntry`]'s presentation tail
/// (protocol v7, F25-B): the drive fields a remote copy's wheel and
/// glow visuals need but a kinematic replica cannot derive from pose.
/// `steer` is the *actual* angle (rate limits and assists already
/// reflected), `spin` the mean grounded-wheel `vel_long / radius` — the
/// same expression the sim integrates into `WheelState::spin` — and
/// `compression` the mean per-wheel `compression / travel`. Every field
/// saturates or clamps rather than wrapping.
fn encode_present(
    cfg: &VehicleConfig,
    state: &VehicleState,
    input: &VehicleInput,
) -> (i16, i16, u8, u8) {
    let steer = (state.steer_angle * 1000.0)
        .round()
        .clamp(i16::MIN as f32, i16::MAX as f32) as i16;
    let count = cfg.wheels.len().min(state.wheels.len());
    let (mut spin_sum, mut grounded, mut comp_sum) = (0.0f32, 0usize, 0.0f32);
    for (wheel, ws) in cfg.wheels.iter().zip(state.wheels.iter()).take(count) {
        if ws.grounded {
            spin_sum += ws.vel_long / wheel.radius.max(0.01);
            grounded += 1;
        }
        let travel = wheel.suspension.as_ref().unwrap_or(&cfg.suspension).travel;
        comp_sum += (ws.compression / travel.max(0.001)).clamp(0.0, 1.0);
    }
    // Airborne reads as rate 0 — the sim's own rule holds a lifted
    // wheel's angle rather than free-spinning it.
    let rate = if grounded > 0 {
        spin_sum / grounded as f32
    } else {
        0.0
    };
    let spin = (rate * 10.0)
        .round()
        .clamp(i16::MIN as f32, i16::MAX as f32) as i16;
    let compression = if count > 0 {
        (comp_sum / count as f32 * 255.0).round() as u8
    } else {
        0
    };
    let mut flags = 0u8;
    // The same threshold `update_glows` reads on the pedal — a remote
    // car's brake/reverse lights track the driver's actual brake.
    if input.brake > 0.05 {
        flags |= SNAP_FLAG_BRAKE;
    }
    if state.direction == DriveDirection::Reverse {
        flags |= SNAP_FLAG_REVERSE;
    }
    if state.grounded {
        flags |= SNAP_FLAG_GROUNDED;
    }
    (steer, spin, compression, flags)
}

/// The receiving half of [`encode_present`]: fold a [`SnapEntry`]'s
/// presentation tail into a remote copy's `VehicleState`/`VehicleInput`
/// so the stock presentation systems (`update_wheel_visuals`,
/// `update_glows`) render it like a live car. Clamp-before-trust like
/// the pose path — presentation fields are informational, never
/// authoritative over physics.
fn apply_present(
    entry: &SnapEntry,
    cfg: &VehicleConfig,
    state: &mut VehicleState,
    input: &mut VehicleInput,
    drive: Option<&mut RemoteDrive>,
) {
    state.steer_angle = (entry.steer as f32 / 1000.0).clamp(-MAX_WIRE_STEER, MAX_WIRE_STEER);
    state.direction = if entry.flags & SNAP_FLAG_REVERSE != 0 {
        DriveDirection::Reverse
    } else {
        DriveDirection::Forward
    };
    state.grounded = entry.flags & SNAP_FLAG_GROUNDED != 0;
    let frac = entry.compression as f32 / 255.0;
    for (wheel, ws) in cfg.wheels.iter().zip(state.wheels.iter_mut()) {
        ws.grounded = state.grounded;
        ws.compression = if state.grounded {
            let travel = wheel.suspension.as_ref().unwrap_or(&cfg.suspension).travel;
            frac * travel
        } else {
            0.0
        };
    }
    // The glow threshold `update_glows` applies — binary on the wire,
    // reconstituted as a full/defeat pedal press.
    input.brake = if entry.flags & SNAP_FLAG_BRAKE != 0 {
        1.0
    } else {
        0.0
    };
    if let Some(drive) = drive {
        drive.spin_rate = entry.spin as f32 * 0.1;
    }
}

/// The [`SnapTrailer`] row's grounded bit folded into a kinematic
/// copy's wheel state (F25-B). The row carries no compression field —
/// the seat entry's byte is aggregated across wheels by design and the
/// trailer tail omits it — so a grounded copy settles every wheel at
/// its authored rest sag and an airborne one at full droop: the two
/// poses `update_wheel_visuals` can draw from a single bit.
fn apply_trailer_present(row: &SnapTrailer, cfg: &VehicleConfig, state: &mut VehicleState) {
    let grounded = row.flags & SNAP_FLAG_GROUNDED != 0;
    state.grounded = grounded;
    if grounded {
        let rest = HandlingMetrics::of(cfg);
        for (ws, wm) in state.wheels.iter_mut().zip(rest.wheels.iter()) {
            ws.grounded = true;
            ws.compression = wm.rest_compression;
        }
    } else {
        for ws in &mut state.wheels {
            ws.grounded = false;
            ws.compression = 0.0;
        }
    }
}

/// `VehicleDamage` → a [`SnapEntry`]'s `damage` byte (protocol v8,
/// F25-B): the authority's accumulated total as a fraction of the
/// seat's authored `MaxDamage`, quantized ×255. `None` — a participant
/// with no authored `vehcardamage` — encodes 0: undamageable reads as
/// undamaged, never a fabricated spec. A degenerate (`<= 0` or
/// non-finite) bound encodes 0 the same way rather than dividing by
/// it; a saturating accumulator can never exceed `max_damage`, but the
/// clamp stands anyway — the byte means "fraction of the authored
/// bound", not "whatever total happened to accumulate".
fn encode_damage(damage: Option<&VehicleDamage>) -> u8 {
    let Some(damage) = damage else {
        return 0;
    };
    let fraction = damage.total() / damage.spec.max_damage;
    if !fraction.is_finite() || fraction <= 0.0 {
        return 0;
    }
    // A positive total never encodes 0: the byte's `>0 → 0` transition
    // is the wire's repair signal (the receiver wipes the seat's texel
    // splats on it), so a rounding-to-zero hit must not mint one.
    ((fraction.min(1.0) * 255.0).round() as u8).max(1)
}

/// `VehicleBreaks` → a [`SnapEntry`]'s `breaks` bitmask (protocol v11,
/// F25-B): bit *i* set = rig part *i* (authored order — identical on
/// every process under the gameplay fingerprint) is off the car.
/// `None` — a participant with no authored break inventory — encodes
/// 0, and parts past bit 31 never ride the wire: far past any authored
/// count (the retail roster tops out at single digits).
fn encode_breaks(breaks: Option<&VehicleBreaks>) -> u32 {
    let Some(breaks) = breaks else {
        return 0;
    };
    breaks
        .parts
        .iter()
        .enumerate()
        .take(32)
        .fold(0u32, |bits, (i, part)| {
            bits | (u32::from(!part.attached) << i)
        })
}

/// The damage side of a [`SnapEntry`]: reconstitute the wire fraction
/// onto the entity's own [`VehicleDamage`] spec. Runs for every
/// `NetPlayer` seat the snap names — remote copies *and* the own seat
/// (under a predicted session nothing local accumulates damage, so the
/// replicated total is the meter's truth — F05 req 6). Entities with
/// no authored damage record carry no component and are skipped.
///
/// A `>0 → 0` transition is the wire's repair signal: the authority's
/// `resolve_disabled` wipes the skin next to each `damage.reset()` it
/// performs, and this is the same wipe arriving by replication — the
/// rig's `Reset` re-blits clean. Splats stamped while the byte read
/// intact stay put: the retail rig splats every `ImpactsTable` entry
/// regardless of the accumulator, so only a real repair transition
/// re-blits. Returns `true` when the transition fired so the caller
/// can record it in [`RemoteSnaps::repaired`] — the ledger the
/// pending-impact drain reads to drop a pre-repair row rather than
/// splat it on top of the wipe.
fn apply_damage(
    entity: Entity,
    entry: &SnapEntry,
    damage: Option<Mut<'_, VehicleDamage>>,
    texel: &mut crate::texel_fx::TexelRepair,
    report: &mut NetDriveReport,
) -> bool {
    if let Some(mut damage) = damage {
        let was_damaged = damage.total() > 0.0;
        damage.set_replicated(entry.damage as f32 / 255.0);
        report.damage_synced += 1;
        if was_damaged && damage.total() <= 0.0 {
            texel.reset(entity);
            return true;
        }
    }
    false
}

/// The wire id this process's own seat carries: 0 on a host, our roster
/// slot on a joined client. `None` when no link exists.
fn self_wire(link: Option<&LobbyLink>, host: Option<&HostLink>) -> Option<u16> {
    if host.is_some() {
        Some(0)
    } else {
        link.map(|l| l.player_id())
    }
}

/// The grid seat every wire id maps to — the participants the lobby
/// seated, sorted so host and clients compute the same assignment.
/// Wire id 0 joins the list only while a host seat exists — `hosted`
/// is `HostLink::is_some` on a hosted app, `lobby.host_pick.is_some()`
/// (a `Start`-carried pick) on a joined client; a dedicated `mm2-host`
/// seats nobody, so its first roster member takes seat 0. Our own id
/// is part of the map — the local car occupies a grid slot like every
/// remote one.
fn seat_ids(lobby: Option<&LobbyState>, hosted: bool, self_wire: Option<u16>) -> Vec<u16> {
    let mut ids: Vec<u16> = lobby
        .map(|l| l.roster.iter().map(|e| e.player_id).collect())
        .unwrap_or_default();
    if let Some(wire) = self_wire
        && !ids.contains(&wire)
    {
        ids.push(wire);
    }
    // The host seat is never a roster entry — add it while the wire
    // says one is playing.
    if hosted || self_wire == Some(0) {
        ids.push(0);
    }
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// A participant's rank on the seat map — its grid slot index.
/// `None`/unknown ids rank 0, the same slot solo play takes.
fn seat_index(seats: &[u16], wire: Option<u16>) -> usize {
    wire.and_then(|w| seats.iter().position(|&id| id == w))
        .unwrap_or(0)
}

/// The pose one grid seat starts at — the resolution every process
/// computes identically for every participant (F25-A.2):
///
/// - seat *i* takes `start_slots[i]` verbatim — authored position,
///   authored `yaw_deg` or the derived course facing (`yaw_deg` `None`
///   is the `_strtpnts` `a = 0` no-heading case, WPT-4);
/// - past the authored grid the seats keep fanning right off the last
///   authored slot — the grid's own spacing continued (designed);
/// - with no definition at all (cruise, dev world) the seats fan
///   right off the roam base the same way.
///
/// `base` is the session's pre-seat spawn pose — `SpawnPoint.origin`,
/// not the already-seated `position`.
pub fn seat_pose(race: Option<&RaceDefinition>, base: (Vec3, f32), seat: usize) -> (Vec3, f32) {
    let (base_pos, base_yaw) = base;
    let right = |yaw: f32| Vec3::new(yaw.cos(), 0.0, -yaw.sin());
    if let Some(def) = race
        && let Some(&last) = def.start_slots.last()
    {
        let slot = def.start_slots.get(seat).unwrap_or(&last);
        let yaw = slot
            .yaw_deg
            .map(f32::to_radians)
            .or_else(|| def.course_yaw(slot.position))
            .unwrap_or(base_yaw);
        let pos = if seat < def.start_slots.len() {
            slot.position
        } else {
            slot.position
                + right(yaw) * (SEAT_STAGE_GAP * (seat + 1 - def.start_slots.len()) as f32)
        };
        return (pos, yaw);
    }
    (
        base_pos + right(base_yaw) * (SEAT_STAGE_GAP * seat as f32),
        base_yaw,
    )
}

/// Seat the local participant on the shared grid: `position`/`yaw`
/// become the seat's pose while `origin`/`origin_yaw` keep the pre-seat
/// roam base — the anchor remote seats' fan-out fallback resolves
/// against on every process.
pub fn apply_seat(spawn: &mut SpawnPoint, race: Option<&RaceDefinition>, seat: usize) {
    let (pos, yaw) = seat_pose(race, (spawn.origin, spawn.origin_yaw), seat);
    spawn.position = pos;
    spawn.yaw = yaw;
}

/// The session-load side of the seat map: the lobby/transport resources
/// `load_session_world` reads to learn the local participant's grid
/// seat without declaring each link itself.
#[derive(bevy::ecs::system::SystemParam)]
pub struct NetSeats<'w> {
    lobby: Option<Res<'w, LobbyState>>,
    link: Option<Res<'w, LobbyLink>>,
    host: Option<Res<'w, HostLink>>,
}

impl NetSeats<'_> {
    /// The local participant's seat index — its rank on the wire-id
    /// map; 0 while no link exists (solo play, or a link still in its
    /// handshake — the `Start` roster lands before `load_session_world`
    /// ever runs, so a joined client never races the map).
    pub fn self_seat(&self) -> usize {
        let (lobby, link, host) = (
            self.lobby.as_deref(),
            self.link.as_deref(),
            self.host.as_deref(),
        );
        let wire = self_wire(link, host);
        let hosted = host.is_some() || lobby.is_some_and(|l| l.host_pick.is_some());
        seat_index(&seat_ids(lobby, hosted, wire), wire)
    }
}

/// The remote participants the lobby state says should exist:
/// wire id → the pick to build. The host seat (0) enters from the
/// `Start`-carried `host_pick`; peers enter from the roster once they
/// have a committed pick. Our own seat is excluded — it's the local car.
fn desired_remotes(lobby: &LobbyState, self_wire: u16) -> BTreeMap<u16, VehiclePick> {
    let mut set = BTreeMap::new();
    if self_wire != 0
        && let Some(pick) = &lobby.host_pick
    {
        set.insert(0, pick.clone());
    }
    for entry in &lobby.roster {
        if entry.player_id == self_wire {
            continue;
        }
        if let Some(pick) = &entry.pick {
            set.insert(entry.player_id, pick.clone());
        }
    }
    set
}

/// Keep the world matching the lobby's remote roster: spawn a
/// participant entity per picked remote seat (host's included, on
/// clients), despawn ones whose player left or whose pick changed, and
/// stamp the local car's [`NetPlayer`] once it exists. Host and client
/// share the path — the authority role the session stamps decides
/// whether each spawn is a simulated participant or a kinematic copy.
///
/// Remote entities are `SessionEntity`-stamped like everything the
/// session owns, so teardown never needs a second sweep; `RemotePick`
/// is the marker the reconcile uses to tell them from the local car.
// Threads the session, lobby and both link resources plus the asset
// stores a spawn needs — a SystemParam bundle for them would exist only
// to satisfy the lint.
#[allow(clippy::too_many_arguments)]
pub fn reconcile_remote_players(
    mut commands: Commands,
    mut session: ResMut<Session>,
    lobby: Res<LobbyState>,
    link: Option<Res<LobbyLink>>,
    host: Option<Res<HostLink>>,
    vfs: Res<Mm2Vfs>,
    spawn: Res<SpawnPoint>,
    race: Option<Res<RaceState>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    remotes: Query<(Entity, &NetPlayer, &RemotePick)>,
    trailers: Query<(Entity, &car_visual::Trailer, &RemoteTrailer, &RemotePick)>,
    local: Query<Entity, (With<PlayerVehicle>, Without<NetPlayer>)>,
    mut report: ResMut<NetDriveReport>,
) {
    let Some(self_wire) = self_wire(link.as_deref(), host.as_deref()) else {
        return;
    };
    // Only reconcile inside the session the lobby minted — parked at
    // `Menu` nothing exists, and mid-teardown nothing should spawn.
    // `lobby.generation` is the wire's minted value, so it compares
    // against the session's wire generation — a different authority's
    // numbering may sit behind the local id counter.
    let live = lobby.generation == Some(session.wire_generation())
        && session.config().is_some()
        && matches!(
            session.phase(),
            SessionPhase::Ready | SessionPhase::Countdown | SessionPhase::Playing
        );
    if !live {
        return;
    }

    // The local car joins the wire namespace once it exists — the host's
    // snapshots carry it as seat 0, and a client reconciles its own
    // entry through `ResetEpoch` (F25-A.5).
    for entity in &local {
        commands
            .entity(entity)
            .insert((NetPlayer(self_wire), ResetEpoch(0)));
    }

    let desired = desired_remotes(&lobby, self_wire);
    // Departed players and changed picks despawn — the changed pick
    // respawns below with fresh tuning and visuals.
    let mut kept = 0usize;
    for (entity, wire, pick) in &remotes {
        if desired.get(&wire.0) == Some(&pick.0) {
            kept += 1;
        } else {
            commands.entity(entity).despawn();
            report.despawned += 1;
        }
    }
    let present: BTreeMap<u16, ()> = remotes
        .iter()
        .filter(|(_, w, p)| desired.get(&w.0) == Some(&p.0))
        .map(|(_, w, _)| (w.0, ()))
        .collect();
    // A trailer reconciles against its towing seat (F25-B): a departed
    // or re-picked owner takes the trailer down with it — the seat's
    // respawn below rebuilds the whole rig — and a trailer whose
    // `towing` entity is not a kept remote of the same wire id is
    // stale, so the participant despawn above never strands a copy.
    for (entity, trailer, marker, pick) in &trailers {
        let owner_live = remotes.iter().any(|(e, w, p)| {
            e == trailer.towing && w.0 == marker.owner && desired.get(&w.0) == Some(&p.0)
        });
        if desired.get(&marker.owner) != Some(&pick.0) || !owner_live {
            commands.entity(entity).despawn();
            report.despawned += 1;
        }
    }
    let owner = SessionEntity(session.generation());
    let role = session.authority_role();
    // The shared seat map — every remote's grid pose resolves through
    // the same function `load_session_world` seated the local car with.
    let seats = seat_ids(
        Some(&lobby),
        host.is_some() || lobby.host_pick.is_some(),
        Some(self_wire),
    );
    let mut spawned_now = 0usize;
    for (wire, pick) in desired {
        if present.contains_key(&wire) {
            continue;
        }
        if spawn_remote(
            &mut commands,
            &vfs.0,
            &mut session,
            race.as_deref(),
            &seats,
            &spawn,
            &mut meshes,
            &mut images,
            &mut materials,
            wire,
            &pick,
            owner,
            role,
        ) {
            report.spawned += 1;
            spawned_now += 1;
        }
    }
    report.remotes = kept + spawned_now;
}

/// Spawn one remote participant: session-owned, stably identified,
/// `PlayerControl::Remote` — then the authority role splits it. On the
/// host it is a dynamic `Vehicle` the input mailbox drives, carrying the
/// authored damage/stuck specs and the designed recovery detector so the
/// authority's rule pipeline resolves it like an AI opponent (F25-A.4);
/// on a client it is a kinematic copy a `RemoteLerp` blend drives, the
/// same components present but inert under a predicted session. A pick
/// that fails to load is warned and skipped — the validator already
/// gates roster picks, so this is a defensive path (a dev-car pick
/// cannot fail). Returns whether the entity was spawned.
// Every argument is a distinct borrow `reconcile_remote_players` already
// holds — bundling them into a struct would just move the same list.
#[allow(clippy::too_many_arguments)]
fn spawn_remote(
    commands: &mut Commands,
    vfs: &mm2_assets::Vfs,
    session: &mut Session,
    race: Option<&RaceState>,
    seats: &[u16],
    spawn: &SpawnPoint,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
    wire: u16,
    pick: &VehiclePick,
    owner: SessionEntity,
    role: mm2_game::AuthorityRole,
) -> bool {
    let predicted = !role.is_authority();
    let def = if pick.vehicle.is_empty() {
        None
    } else {
        match mm2_content::load_vehicle(vfs, &pick.vehicle, pick.paint as usize) {
            Ok(def) => Some(def),
            Err(e) => {
                warn!(
                    player = wire,
                    vehicle = %pick.vehicle,
                    error = %e,
                    "remote pick failed to load — slot skipped"
                );
                return false;
            }
        }
    };
    let cfg = def.as_ref().map(|d| d.config.clone()).unwrap_or_default();
    // The seat map's slot: an authored grid row, or the designed
    // fan-out past it — identical on host and clients (F25-A.2).
    let (mut pos, yaw) = seat_pose(
        race.map(|r| &r.definition),
        (spawn.origin, spawn.origin_yaw),
        seat_index(seats, Some(wire)),
    );
    // The same hull clearance every participant spawn applies.
    let hull_min_y = cfg
        .collider_points
        .as_ref()
        .and_then(|pts| pts.iter().map(|p| p[1]).reduce(f32::min))
        .unwrap_or(-cfg.chassis_size[1] * 0.5);
    pos.y += (SPAWN_LIFT - hull_min_y).max(0.35);

    let object = session.mint_object_id();
    let player_id = session.mint_player_id();
    let vehicle = commands
        .spawn((
            owner,
            ObjectIdentity(object),
            Player {
                id: player_id,
                control: PlayerControl::Remote,
            },
            role,
            NetPlayer(wire),
            RemotePick(pick.clone()),
            ResetEpoch(0),
            DamageSignals::default(),
            vehicle_bundle(&cfg),
            Transform::from_translation(pos).with_rotation(Quat::from_rotation_y(yaw)),
            TransformInterpolation,
            // Parents of renderable children need the visibility chain.
            Visibility::Visible,
        ))
        .id();
    if predicted {
        // A client never simulates a remote car's truth: kinematic —
        // it collides as the host says it is, and `RemoteLerp` blends
        // snapshots into its pose. `RemoteReplica` keeps
        // `vehicle_simulation` from stepping the copy's state — the
        // snap stream's presentation tail owns it now (F25-B) — and
        // `RemoteDrive` holds the replicated wheel rate the lerp
        // integrates between snapshots.
        commands.entity(vehicle).insert((
            RigidBody::Kinematic,
            RemoteReplica,
            RemoteDrive::default(),
            RemoteLerp {
                from_pos: pos,
                from_rot: Quat::from_rotation_y(yaw),
                to_pos: pos,
                to_rot: Quat::from_rotation_y(yaw),
                start: 0.0,
                end: 0.0,
            },
        ));
    }
    // F25-A.4: the authority resolves a remote driver's world outcomes
    // like an AI opponent's — the authored damage/stuck records gate
    // the components (absent = undamageable/unstuckable, never a
    // fabricated spec) and the designed recovery policy rides every
    // seat, anchored at its spawn pose. On a predicted client the
    // rule systems are inert — but `VehicleDamage` is where the v8
    // snap tail lands the replicated total, and `VehicleSmoke` renders
    // it.
    if let Some(d) = def.as_ref().and_then(|d| d.damage.as_ref()) {
        commands.entity(vehicle).insert((
            VehicleDamage::new(DamageSpec::from(d)),
            // F25-B: the authored engine-smoke rig rides with the
            // damage spec like any participant's — on the host it
            // emits from the authority's own accumulation, on a
            // client from the replicated total the v8 snap tail
            // writes. Seeded off the wire id rather than the locally
            // minted object slot so every process replays the same
            // emission stream for the seat.
            VehicleSmoke::new(
                d,
                SmokePolicy::default(),
                // The wire generation likewise — the local id
                // counter can diverge across processes when a
                // different authority's numbering joins.
                (session.wire_generation() << 32) | u64::from(wire),
            ),
            // The impact-spark renderer binds the same way (F25-B
            // protocol v10): on the authority the copy sparks off the
            // local `ImpactEvent` stream like any simulated seat, on a
            // client off the replicated `RemoteImpact` rows the v10
            // tail delivers — same wire-seeded stream on every
            // process.
            VehicleSparks::new(
                SparkPolicy::default(),
                (session.wire_generation() << 32) | u64::from(wire),
            ),
        ));
    }
    if let Some(s) = def.as_ref().and_then(|d| d.stuck.as_ref()) {
        commands
            .entity(vehicle)
            .insert(VehicleStuck::new(StuckSpec::from(s)));
    }
    // F25-B (protocol v11): the authored breakaway inventory rides
    // both roles — on the authority it is the live rig `detach_breaks`
    // sheds parts off (the seat's `SnapEntry.breaks` bitmask publishes
    // it); on a client it is the presentation rig the replicated
    // bitmask reconciles. Same absence policy as the local spawn: no
    // authored parts, no component.
    if let Some(def) = def.as_ref().filter(|d| !d.breaks.is_empty()) {
        commands.entity(vehicle).insert(VehicleBreaks::new(
            def.breaks
                .iter()
                .map(|b| BreakPartSpec {
                    name: b.name.clone(),
                    def: b.def.clone(),
                })
                .collect(),
        ));
    }
    commands
        .entity(vehicle)
        .insert(VehicleRecovery::with_anchor(
            RecoveryPolicy::default(),
            pos,
            yaw,
        ));
    // Race progress on the shared definition — a remote participant in
    // an event scores like any other driver on the authority that owns
    // it (the host); the component is inert on predicted copies.
    // `join`, not `new`: a spawn landing while the race already runs
    // starts `Racing` — `AwaitingStart` here would never see the
    // countdown's release flip (it fired before the entity existed).
    if let Some(race) = race {
        commands
            .entity(vehicle)
            .insert(RaceProgress::join(&race.definition, race));
    }
    match &def {
        Some(def) => {
            let missing = car_visual::spawn_vehicle_model(
                commands,
                vfs,
                &def.model,
                pick.paint as usize,
                meshes,
                images,
                materials,
                vehicle,
                // F25-B (protocol v10): the texel rig binds on the
                // authored damage record like a local pick's, seeded
                // off the wire id the same way the smoke/spark rigs
                // are. On the authority the seat's own `ImpactEvent`s
                // splat it (`apply_texel_damage`); on a client the
                // replicated `RemoteImpact` rows do
                // (`apply_remote_texels`), and the v8 byte's repair
                // transition clears it.
                def.damage
                    .as_ref()
                    .map(|d| (d, (session.wire_generation() << 32) | u64::from(wire))),
            );
            if !missing.is_empty() {
                warn!(car = %def.id, "remote vehicle missing textures: {}", missing.join(", "));
            }
        }
        None => car_visual::spawn_dev_car(commands, &cfg, meshes, materials, vehicle),
    }
    // F25-B (protocol v9): a trailered pick tows its trailer on every
    // process — the authority builds the real jointed body
    // `spawn_trailer` gives the local car, a predicted client holds a
    // kinematic copy the `Snap.trailers` rows drive (no joint: the wire
    // owns its pose; `RemoteReplica`/`RemoteLerp`/`RemoteDrive` give it
    // the same blend-and-spin treatment the car copy gets).
    if let Some(trailer) = def.as_ref().and_then(|d| d.trailer.as_ref()) {
        if predicted {
            let te = spawn_trailer_copy(
                commands, session, trailer, pick, wire, vehicle, pos, yaw, owner, role,
            );
            let missing = car_visual::spawn_vehicle_model(
                commands,
                vfs,
                &trailer.model,
                pick.paint as usize,
                meshes,
                images,
                materials,
                te,
                // Trailers carry no `vehcardamage` — no texel rig.
                None,
            );
            if !missing.is_empty() {
                warn!(car = %def.as_ref().map(|d| d.id.as_str()).unwrap_or("?"),
                    "remote trailer missing textures: {}", missing.join(", "));
            }
        } else {
            let car_xf = Transform::from_translation(pos).with_rotation(Quat::from_rotation_y(yaw));
            let (te, tmissing) = car_visual::spawn_trailer(
                commands,
                vfs,
                trailer,
                pick.paint as usize,
                meshes,
                images,
                materials,
                vehicle,
                car_xf,
                owner,
            );
            // Stamped like the local spawn's trailer: simulated under
            // the session's authority with an object id, plus the wire
            // markers reconcile/publish key on.
            commands.entity(te).insert((
                ObjectIdentity(session.mint_object_id()),
                role,
                DamageSignals::default(),
                RemoteTrailer { owner: wire },
                RemotePick(pick.clone()),
            ));
            if !tmissing.is_empty() {
                warn!(car = %def.as_ref().map(|d| d.id.as_str()).unwrap_or("?"),
                    "remote trailer missing textures: {}", tmissing.join(", "));
            }
        }
    }
    info!(player = wire, "remote participant spawned");
    true
}

/// The predicted-client half of [`spawn_remote`]'s trailer rig (F25-B):
/// a kinematic copy the `Snap.trailers` rows drive — no hitch joint,
/// the wire owns its pose. `RemoteReplica`/`RemoteLerp`/`RemoteDrive`
/// give it the same blend-and-spin treatment the seat copy gets, and
/// `DamageSignals` matches what the authority's real body and the local
/// trailer carry so a client's impact-signal consumers see the remote
/// rig's trailer contacts too.
// Every argument is a distinct borrow `spawn_remote` already holds —
// bundling them into a struct would just move the same list.
#[allow(clippy::too_many_arguments)]
fn spawn_trailer_copy(
    commands: &mut Commands,
    session: &mut Session,
    trailer: &mm2_content::TrailerDef,
    pick: &VehiclePick,
    wire: u16,
    car: Entity,
    pos: Vec3,
    yaw: f32,
    owner: SessionEntity,
    role: mm2_game::AuthorityRole,
) -> Entity {
    // The same rest geometry `car_visual::spawn_trailer` uses: both
    // hitch anchors coincide in world space while the trailer shares
    // the car's heading.
    let rest_offset = Vec3::from(trailer.car_hitch) - Vec3::from(trailer.trailer_hitch);
    let trot = Quat::from_rotation_y(yaw);
    let tpos = pos + trot * rest_offset;
    let te = commands
        .spawn((
            owner,
            ObjectIdentity(session.mint_object_id()),
            role,
            car_visual::Trailer {
                towing: car,
                rest_offset,
            },
            RemoteTrailer { owner: wire },
            RemotePick(pick.clone()),
            DamageSignals::default(),
            vehicle_bundle(&trailer.config),
            Transform::from_translation(tpos).with_rotation(trot),
            TransformInterpolation,
            Visibility::Visible,
        ))
        .id();
    // Same insert-over-bundle pattern the seat spawn uses:
    // `vehicle_bundle` already carries `RigidBody`, so the kinematic
    // override goes through `insert`, not the spawn tuple (a duplicate
    // component in one bundle panics).
    commands.entity(te).insert((
        RigidBody::Kinematic,
        RemoteReplica,
        RemoteDrive::default(),
        RemoteLerp {
            from_pos: tpos,
            from_rot: trot,
            to_pos: tpos,
            to_rot: trot,
            start: 0.0,
            end: 0.0,
        },
    ));
    te
}

/// Client-side: the local car's `VehicleInput` becomes a `DriveInput`
/// frame per update, generation-stamped so a stale session can never
/// inject input into a later one. Runs after every input owner
/// (`vehicle_input`, the scripted drivers) so the wire sees the settled
/// sample.
pub fn send_drive_input(
    link: Res<LobbyLink>,
    session: Res<Session>,
    mut seq: ResMut<InputSeq>,
    local: Query<&VehicleInput, With<PlayerVehicle>>,
    mut report: ResMut<NetDriveReport>,
) {
    if !session.is_playing() || link.closed || link.leaving() {
        return;
    }
    let Ok(input) = local.single() else {
        return;
    };
    seq.0 += 1;
    if link
        .ctl()
        .send_input(encode_input(input, session.wire_generation(), seq.0))
        .is_ok()
    {
        report.inputs_sent += 1;
    }
}

/// Host-side: each remote participant's `VehicleInput` comes from its
/// mailbox slot — the newest sample within [`INPUT_STALE`] for this
/// generation, else zero (a stalled driver's car coasts). A wrong
/// generation's sample is a previous session's — always zeroed, never
/// applied.
pub fn apply_remote_inputs(
    host: Res<HostLink>,
    session: Res<Session>,
    mut remotes: Query<(&NetPlayer, &mut VehicleInput), With<RemotePick>>,
    mut report: ResMut<NetDriveReport>,
) {
    if !session.is_playing() {
        return;
    }
    let inputs: RemoteInputs = host.remote_inputs();
    for (wire, mut input) in &mut remotes {
        let fresh = inputs.latest(wire.0).filter(|s| {
            s.input.generation == session.wire_generation() && s.received.elapsed() <= INPUT_STALE
        });
        match fresh {
            Some(stamped) => {
                *input = decode_input(&stamped.input);
                report.inputs_applied += 1;
            }
            None => {
                *input = VehicleInput::default();
                report.inputs_staled += 1;
            }
        }
    }
}

/// Client-side (F25-B): `R`/[`pad::RESET`] under a predicted (`Remote`)
/// session asks the authority for the reset `reset_input` is gated
/// against performing — a local teleport would diverge the own seat
/// from the host's copy forever (F25-A.5), so the key instead sends a
/// `ResetRequest` for the running generation. The granted answer is the
/// seat's epoch-declared `Snap`: [`apply_snapshots`]' own-seat reconcile
/// already applies it like any authority reset. Fire-and-forget — a
/// dropped request is a key press nothing answered, the same
/// dead-feeling the inert gate had (a lobby notice is future UX work);
/// no reply message exists by design.
pub fn send_reset_request(
    keys: Res<ButtonInput<KeyCode>>,
    pads: Query<&Gamepad>,
    windows: Query<&Window>,
    link: Res<LobbyLink>,
    session: Res<Session>,
    mut report: ResMut<NetDriveReport>,
) {
    if session.authority_role().is_authority()
        || !session.is_playing()
        || link.closed
        || link.leaving()
        || !control_just_pressed(&keys, &pads, &windows, KeyCode::KeyR, pad::RESET)
    {
        return;
    }
    if link.ctl().request_reset(session.wire_generation()).is_ok() {
        report.requests_sent += 1;
    }
}

/// Per-seat grant ledger for [`apply_reset_requests`] — the instant each
/// wire id's last request was honored. Scoped to the session generation:
/// a `Cancel`/`Start` cycle clears every debt.
#[derive(Default)]
pub struct RequestGrants {
    generation: u64,
    last: BTreeMap<u16, Instant>,
}

/// Host-side (F25-B): fold the drained reset requests into
/// [`ResetVehicle`]s — targeted at the *requesting* seat and landed on
/// its grid slot (the seat map every process resolves identically,
/// F25-A.2), so `vehicle_reset` applies it the same frame and
/// `track_reset_epochs` declares the bump on the `Snap` the teleported
/// pose rides — exactly like any other authority teleport. The writer
/// is scheduled ahead of the apply like every Update-side writer
/// (F25-A.6's ordering contract).
///
/// A request is dropped, never deferred: minted against a foreign or
/// stale generation, sent while the session is not `Playing`, naming a
/// seat with no spawned participant, or inside
/// [`RESET_REQUEST_COOLDOWN`]. The cooldown is the designed bound spec
/// req 1 wants on the channel — a wire ask arrives at socket rate, not
/// key-edge rate, so without it one client could teleport-lock its seat
/// every update. Requests carry no target — the sender's roster slot
/// names the seat — so a peer can only ever reset its own car.
// A Bevy system that has to see the host link, session, lobby, spawn
// point, race, participant query, reset writer, grant ledger and report
// — the seat-grant computation genuinely needs all of them.
#[allow(clippy::too_many_arguments)]
pub fn apply_reset_requests(
    host: Res<HostLink>,
    session: Res<Session>,
    lobby: Res<LobbyState>,
    spawn: Res<SpawnPoint>,
    race: Option<Res<RaceState>>,
    remotes: Query<(Entity, &NetPlayer, &Vehicle), With<RemotePick>>,
    mut resets: MessageWriter<ResetVehicle>,
    mut grants: Local<RequestGrants>,
    mut report: ResMut<NetDriveReport>,
) {
    // Requests arrive stamped with the *wire* generation — the
    // authority's minted value, not the local id counter.
    let generation = session.wire_generation();
    if grants.generation != generation {
        grants.generation = generation;
        grants.last.clear();
    }
    let requests = host.remote_inputs().drain_resets();
    if requests.is_empty() {
        return;
    }
    let playing = session.is_playing();
    let now = Instant::now();
    // The shared seat map — on a hosted app the local seat is wire id
    // 0, and `LobbyState` mirrors the remote roster.
    let seats = seat_ids(Some(&lobby), true, Some(0));
    for (wire, requested) in requests {
        let fresh = playing
            && requested == generation
            && grants
                .last
                .get(&wire)
                .is_none_or(|t| now.duration_since(*t) >= RESET_REQUEST_COOLDOWN);
        let target = if fresh {
            remotes.iter().find(|(_, w, _)| w.0 == wire)
        } else {
            None
        };
        let Some((entity, _, vehicle)) = target else {
            report.requests_dropped += 1;
            continue;
        };
        let (mut pos, yaw) = seat_pose(
            race.as_deref().map(|r| &r.definition),
            (spawn.origin, spawn.origin_yaw),
            seat_index(&seats, Some(wire)),
        );
        // The same hull clearance the spawn applies — the seat lands
        // the car just above the ground for gravity to settle.
        let hull_min_y = vehicle
            .config
            .collider_points
            .as_ref()
            .and_then(|pts| pts.iter().map(|p| p[1]).reduce(f32::min))
            .unwrap_or(-vehicle.config.chassis_size[1] * 0.5);
        pos.y += (SPAWN_LIFT - hull_min_y).max(0.35);
        resets.write(ResetVehicle {
            entity: Some(entity),
            position: pos,
            yaw,
        });
        grants.last.insert(wire, now);
        report.requests_granted += 1;
    }
}

/// Host-side: fold every [`ResetVehicle`] the session's reset paths
/// emit into the targets' [`ResetEpoch`] — the wire's reset signal
/// (F25-A.5). `vehicle_reset` consumes the same message stream to apply
/// the teleport; readers are independent cursors, so watching it here
/// can never steal the reset from its applier. A `None` entity resets
/// every vehicle, so every participant's epoch bumps. The schedule
/// keeps `writers → vehicle_reset → tracker → publish_snapshots`
/// (F25-A.6): every Update-scheduled reset writer is ordered ahead of
/// the apply and the tracker runs after it, so the bumped epoch and the
/// teleported pose leave on the same `Snap` — never a pose first with
/// its epoch trailing a snapshot later.
pub fn track_reset_epochs(
    mut resets: MessageReader<ResetVehicle>,
    mut players: Query<&mut ResetEpoch>,
    mut report: ResMut<NetDriveReport>,
) {
    for ev in resets.read() {
        match ev.entity {
            Some(entity) => {
                // Non-participant resets (a re-seated trailer) carry no
                // epoch — they ride their tractor's snap.
                if let Ok(mut epoch) = players.get_mut(entity) {
                    epoch.0 = epoch.0.wrapping_add(1);
                    report.resets += 1;
                }
            }
            None => {
                for mut epoch in &mut players {
                    epoch.0 = epoch.0.wrapping_add(1);
                }
                report.resets += 1;
            }
        }
    }
}

/// The snapshot publish query row — a participant's wire identity,
/// reset epoch, rigid truth and the drive state the v7 presentation
/// tail encodes, plus the damage state the v8 tail encodes. The drive
/// and damage rows are `Option`: a participant without a vehicle
/// bundle or authored damage record still publishes its pose rather
/// than vanishing. The `Entity` leads so a trailer row can key off its
/// towing car's wire id.
type SnapSourceRow<'a> = (
    Entity,
    &'a NetPlayer,
    &'a ResetEpoch,
    &'a ObjectIdentity,
    &'a Position,
    &'a Rotation,
    &'a LinearVelocity,
    &'a AngularVelocity,
    Option<&'a Vehicle>,
    Option<&'a VehicleState>,
    Option<&'a VehicleInput>,
    Option<&'a VehicleDamage>,
    // The v11 breakaway bitmask's source — `Option` like the drive
    // row: a participant with no authored break inventory publishes 0.
    Option<&'a VehicleBreaks>,
);

/// The publish-side trailer row (protocol v9, F25-B): the `Trailer`
/// relation names the towing seat the row keys off, and the optional
/// drive triple feeds `encode_present`'s spin/grounded half — a
/// trailer with no vehicle bundle still publishes its pose.
type SnapTrailerSourceRow<'a> = (
    &'a car_visual::Trailer,
    &'a Position,
    &'a Rotation,
    &'a LinearVelocity,
    &'a AngularVelocity,
    Option<&'a Vehicle>,
    Option<&'a VehicleState>,
    Option<&'a VehicleInput>,
);

/// Host-side: every participant's authoritative pose, broadcast once per
/// update while the session is live. `tick` is the host's session tick —
/// physics only moves inside fixed steps, so a same-tick snapshot is a
/// duplicate clients discard. Positions are `Position`/`Rotation` (the
/// solver's truth), not the render `Transform`; `epoch` is the seat's
/// [`ResetEpoch`] — the receiver's teleport signal. The v7 tail carries
/// the replicated drive presentation ([`encode_present`]): steering
/// angle, wheel spin rate, suspension droop and the brake/reverse/
/// grounded flags; the v8 tail adds [`encode_damage`], the seat's
/// authoritative damage fraction — what a remote copy needs to *look*
/// like the car the authority is simulating. The v10 tail replicates
/// this frame's [`ImpactEvent`] stream as [`SnapImpact`] rows: every
/// participant side that maps to a `NetPlayer` seat emits one row
/// (mirrored outward normal per side), capped at
/// [`MAX_SNAP_IMPACTS`] with the strongest hits kept — presentation
/// the damage fraction cannot express. The v11 tail adds
/// [`encode_breaks`], the seat's detached-part bitmask — replicated
/// rig state a receiver diffs, so a dropped snap or a repair can
/// never leave a copy's breakaway inventory diverged. The v13 tail
/// adds [`encode_race`], the session's race phase/clock — replicated
/// lifecycle state a predicted client mirrors instead of stepping.
#[allow(clippy::too_many_arguments)] // Bevy system — the borrows are the contract.
pub fn publish_snapshots(
    host: Res<HostLink>,
    session: Res<Session>,
    // The v13 race row's source — `None` on a raceless session (cruise,
    // dev worlds), so the field stays `None` on the wire too.
    race: Option<Res<RaceState>>,
    // The `(wire generation, session tick)` of the last broadcast —
    // bounds the `Results`-phase debt below to the one unpublished
    // transition frame.
    mut published: Local<Option<(u64, u64)>>,
    players: Query<SnapSourceRow<'_>, With<Player>>,
    // Every trailer towing a `NetPlayer` seat — the host's own rig's
    // trailer included — publishes under the owner's wire id.
    trailers: Query<SnapTrailerSourceRow<'_>, Without<Player>>,
    // The v12 struck-side lookup: a row's `audio_id` is the *other*
    // participant's authored `AudioId` — the same resolution
    // `impact_voices` runs locally, done here because a struck prop's
    // `ObjectId` means nothing in the receiver's id namespace.
    bangers: Query<(&ObjectIdentity, &Banger)>,
    mut impacts: MessageReader<ImpactEvent>,
    mut report: ResMut<NetDriveReport>,
    // The v14 progress tail's source: a seat's `ObjectId` → its
    // participant `RaceProgress`, when the race tracks it.
    progresses: Query<(&ObjectIdentity, &RaceProgress), With<Player>>,
) {
    // The reader drains every run — including gated-out phases — so a
    // pre-`Start` or post-session stream never replays stale hits into
    // the next publish.
    let drained: Vec<&ImpactEvent> = impacts.read().collect();
    let key = (session.wire_generation(), session.tick());
    // `Results` owes the wire exactly one more frame (F25-B): the fixed
    // step that resolves the last owed wire seat mints the terminal
    // progress rows and moves `Playing → Results` atomically — the snap
    // stamped with that transition tick is their only carrier. The
    // session clock freezes in `Results` (`advance_session_tick` is
    // `Playing`-only), so the debt is exactly one unpublished
    // `(generation, tick)`; everything earlier already left inside the
    // live phases `advance_race`'s wire-seat deferral preserves.
    let owed =
        *session.phase() == SessionPhase::Results && published.is_none_or(|last| last != key);
    if !matches!(
        session.phase(),
        SessionPhase::Ready | SessionPhase::Countdown | SessionPhase::Playing
    ) && !owed
    {
        return;
    }
    // A seat row is only progress-honest while the session actually
    // has a live race — a raceless or stale `RaceState` leaves every
    // tail zeroed: "the race does not track this seat".
    let race_live = race
        .as_ref()
        .is_some_and(|r| !r.is_stale(session.generation()));
    let seat_progress: HashMap<ObjectId, &RaceProgress> = if race_live {
        progresses.iter().map(|(id, p)| (id.0, p)).collect()
    } else {
        HashMap::new()
    };
    let wires: BTreeMap<Entity, u16> = players
        .iter()
        .map(|(entity, wire, ..)| (entity, wire.0))
        .collect();
    let mut entries: Vec<SnapEntry> = players
        .iter()
        .map(
            |(
                _,
                wire,
                epoch,
                identity,
                pos,
                rot,
                vel,
                ang,
                vehicle,
                state,
                input,
                damage,
                breaks,
            )| {
                let (steer, spin, compression, flags) = match (vehicle, state, input) {
                    (Some(v), Some(s), Some(i)) => encode_present(&v.config, s, i),
                    _ => (0, 0, 0, 0),
                };
                let mut entry = SnapEntry {
                    player: wire.0,
                    pos: pos.0.to_array(),
                    rot: rot.0.to_array(),
                    vel: vel.0.to_array(),
                    angvel: ang.0.to_array(),
                    epoch: epoch.0,
                    steer,
                    spin,
                    compression,
                    flags,
                    damage: encode_damage(damage),
                    breaks: encode_breaks(breaks),
                    ..SnapEntry::default()
                };
                // The v14 progress tail (F25-B): the seat's replicated
                // `RaceProgress` — a predicted client whose rule
                // pipeline never steps mirrors every seat's standing.
                encode_progress(seat_progress.get(&identity.0).copied(), &mut entry);
                entry
            },
        )
        .collect();
    entries.sort_by_key(|e| e.player);
    // The v9 trailer rows (F25-B): a trailer publishes the same
    // pose/spin/grounded truth a seat does, keyed by its towing seat's
    // wire id — the receiver snaps it on the owner entry's epoch, so it
    // carries no epoch of its own. Brake/reverse presentation follows
    // the tractor's `VehicleInput` through `trailer_input`, so the row
    // only carries what pose cannot derive.
    let mut trailer_rows: Vec<SnapTrailer> = trailers
        .iter()
        .filter_map(|(trailer, pos, rot, vel, ang, vehicle, state, input)| {
            let &owner = wires.get(&trailer.towing)?;
            let (_, spin, _, flags) = match (vehicle, state, input) {
                (Some(v), Some(s), Some(i)) => encode_present(&v.config, s, i),
                _ => (0, 0, 0, 0),
            };
            Some(SnapTrailer {
                owner,
                pos: pos.0.to_array(),
                rot: rot.0.to_array(),
                vel: vel.0.to_array(),
                angvel: ang.0.to_array(),
                spin,
                flags: flags & SNAP_FLAG_GROUNDED,
            })
        })
        .collect();
    trailer_rows.sort_by_key(|t| t.owner);
    // The v10 impact rows (F25-B): each drained `ImpactEvent` emits one
    // row per participant side that names a `NetPlayer` seat — a
    // car-vs-car hit rides the wire twice with mirrored outward
    // normals, one per seat. `point`/`normal`/`severity` are shared
    // presentation fields; the receiver resolves `seat` to its own
    // copy. Rows are strongest-first at the cap so a pile-up keeps its
    // worst hits rather than its first.
    let oid_wires: HashMap<ObjectId, u16> = players
        .iter()
        .map(|(_, wire, _, identity, ..)| (identity.0, wire.0))
        .collect();
    // The struck-side selector index — built only when the drained
    // stream has events to tag, so a publish tick with no impacts
    // never walks the prop inventory.
    let struck_audio: HashMap<ObjectId, i64> = if drained.is_empty() {
        HashMap::new()
    } else {
        bangers
            .iter()
            .map(|(id, b)| (id.0, b.def.audio_id))
            .collect()
    };
    // `ImpactEvent::generation` mints in the local id namespace, so the
    // filter compares the local counter — the `Snap` frame's own
    // `generation` field below is the wire namespace.
    let generation = session.generation();
    let wire_generation = session.wire_generation();
    let mut impact_rows: Vec<SnapImpact> = drained
        .into_iter()
        .filter(|e| e.generation == generation)
        .flat_map(|e| {
            // `ImpactEvent::normal` points from participant.0 toward
            // participant.1 — each seat's row carries its own side's
            // outward normal, and the *other* side's authored audio
            // selector (world/seat/recordless read the 0 catch-all).
            [
                (e.participants.0, e.participants.1, -e.normal),
                (e.participants.1, e.participants.0, e.normal),
            ]
            .into_iter()
            .filter_map(|(who, other, normal)| {
                let &seat = oid_wires.get(&who)?;
                let sane = e.point.is_finite() && normal.is_finite() && e.severity.is_finite();
                sane.then_some(SnapImpact {
                    seat,
                    id: e.id.0,
                    tick: e.tick,
                    point: e.point.to_array(),
                    normal: normal.to_array(),
                    severity: e.severity,
                    audio_id: struck_audio.get(&other).copied().unwrap_or(0),
                })
            })
        })
        .collect();
    impact_rows.sort_by(|a, b| {
        b.severity
            .total_cmp(&a.severity)
            .then_with(|| (a.seat, a.id).cmp(&(b.seat, b.id)))
    });
    if impact_rows.len() > MAX_SNAP_IMPACTS as usize {
        report.impacts_dropped += (impact_rows.len() - MAX_SNAP_IMPACTS as usize) as u64;
        impact_rows.truncate(MAX_SNAP_IMPACTS as usize);
    }
    let sent_rows = impact_rows.len() as u64;
    if host
        .ctl()
        .broadcast(&Message::Snap {
            generation: wire_generation,
            tick: session.tick(),
            entries,
            trailers: trailer_rows,
            impacts: impact_rows,
            // The race resource's own generation is the *local*
            // namespace — `is_stale` compares it that way (teardown
            // can leave a resource briefly while the wire id has
            // already moved).
            race: race
                .filter(|r| !r.is_stale(session.generation()))
                .map(|r| encode_race(&r)),
        })
        .is_ok()
    {
        *published = Some(key);
        report.snaps_sent += 1;
        report.impacts_sent += sent_rows;
    }
}

/// The snapshot application's query row — factored out of the system
/// signature for `clippy::type_complexity`. Covers every [`NetPlayer`]
/// entity: remote copies reconcile through their [`RemoteLerp`], the
/// own seat through its [`ResetEpoch`].
type SnapTargetRow<'a> = (
    Entity,
    &'a NetPlayer,
    &'a Player,
    &'a mut ResetEpoch,
    &'a mut Position,
    &'a mut Rotation,
    &'a mut LinearVelocity,
    &'a mut AngularVelocity,
    Option<&'a mut RemoteLerp>,
    // The v7 presentation tail lands here — `Option` so a participant
    // without a vehicle bundle still reconciles its pose. The local
    // seat's fields are live sim state and must not be overwritten by
    // its own entry.
    Option<&'a Vehicle>,
    Option<&'a mut VehicleState>,
    Option<&'a mut VehicleInput>,
    Option<&'a mut RemoteDrive>,
    // The v8 damage byte lands here — `Option` like the drive row:
    // a participant with no authored damage record has nothing to
    // write. Unlike the drive row the own seat *does* take it —
    // replicated damage is the only writer under prediction.
    Option<&'a mut VehicleDamage>,
    // The v11 breakaway bitmask reconciles here — `Option` like the
    // damage row: a participant with no authored break inventory has
    // no rig to diff the wire mask against.
    Option<&'a mut VehicleBreaks>,
);

/// The snapshot application's trailer row (protocol v9, F25-B). A
/// remote copy matches its row by `RemoteTrailer::owner`; the own rig's
/// trailer — a real dynamic body under the predicted session — matches
/// `Trailer::towing` == the local `NetPlayer` entity and only ever
/// snaps, on its owner's epoch advance, like the own seat itself.
type SnapTrailerRow<'a> = (
    Entity,
    &'a car_visual::Trailer,
    Option<&'a RemoteTrailer>,
    &'a mut Position,
    &'a mut Rotation,
    &'a mut LinearVelocity,
    &'a mut AngularVelocity,
    Option<&'a mut RemoteLerp>,
    Option<&'a mut RemoteDrive>,
    // The grounded bit's presentation target — `Option` like the seat
    // row's: a trailer without a vehicle bundle still reconciles its
    // pose, it just has no wheels to droop.
    Option<&'a Vehicle>,
    Option<&'a mut VehicleState>,
);

/// The players query's filter — `With<NetPlayer>` selects the session
/// seats; `Without<Banger>` proves them statically disjoint from the
/// `bangers` pool query the v11 breakaway reconcile claims fragments
/// through (a participant is never a Banger).
type SnapTargetFilter = (With<NetPlayer>, Without<Banger>);

/// The trailer row's filter — `Without<NetPlayer>` proves it disjoint
/// from `players` (a trailer never carries the seat marker),
/// `Without<Banger>` from `bangers`.
type SnapTrailerFilter = (
    With<car_visual::Trailer>,
    Without<NetPlayer>,
    Without<Banger>,
);

/// Client-side: fold the newest staged snapshot into the remote copies'
/// [`RemoteLerp`] blend and velocities, and reconcile the own seat on
/// an epoch advance (F25-A.5). Wrong-generation and stale-tick frames
/// drop untouched; entries without a spawned entity are skipped. The
/// queued replicated impact rows then drain into the [`RemoteImpact`]
/// stream — every run, and *after* the state pass so a repair byte
/// landing this frame is already recorded in [`RemoteSnaps::repaired`]
/// before the rows are judged against it.
///
/// Two snap triggers share the "teleport, not motion" rule: a changed
/// `epoch` — the authority's declared reset — or a correction past
/// [`CORRECTION_SNAP_DIST`]. A remote copy snaps its pose and collapses
/// the blend; the own seat takes the asserted state outright — pose and
/// velocities — since the authority moved the car the local sim thought
/// it owned. Between epochs the own seat's entries are ignored: local
/// physics predicts it, and a mid-drive blend would rubber-band the
/// driver toward a host copy that lags by the round-trip. Both snap
/// paths mark [`Teleported`] so a swept-segment consumer breaks rather
/// than banking the jump.
#[allow(clippy::too_many_arguments)] // Bevy system — the borrows are the contract.
pub fn apply_snapshots(
    mut commands: Commands,
    mut snaps: ResMut<RemoteSnaps>,
    mut session: ResMut<Session>,
    time: Res<Time>,
    // `SnapTargetFilter`'s `Without<Banger>` proves the players
    // disjoint from the `bangers` query the v11 breakaway reconcile
    // claims pool slots through — a participant is never a Banger.
    mut players: Query<SnapTargetRow<'_>, SnapTargetFilter>,
    // Every trailer — remote copies key off `RemoteTrailer::owner`, the
    // own rig's trailer off `Trailer::towing` == the local seat entity.
    mut trailers: Query<SnapTrailerRow<'_>, SnapTrailerFilter>,
    mut remote_fx: MessageWriter<RemoteImpact>,
    // The replicated repair lands here: a snap's damage byte going
    // >0→0 wipes the seat's texel rig like `resolve_disabled`'s own
    // `damage.reset()` + `texel.reset()` pair does on the authority.
    mut texel: crate::texel_fx::TexelRepair,
    // The v11 breakaway reconcile's fragment spawn claims pool slots
    // and shows/hides the intact nodes exactly like the authority's
    // `detach_breaks` does.
    pool: Res<BangerPool>,
    mut bangers: Query<BangerMut>,
    mut banger_writer: MessageWriter<BangerStateChanged>,
    mut break_visuals: Query<BreakVisualMut, Without<Banger>>,
    render_parts: Query<(&Mesh3d, &MeshMaterial3d<StandardMaterial>, &ChildOf)>,
    // The v13 race row's and v14 progress tails' targets — see
    // [`RaceMirror`].
    mut mirror: RaceMirror,
    mut report: ResMut<NetDriveReport>,
) {
    // Push-time stale drops fold into the report every run — a stale
    // frame counts even on a run with nothing staged, so this cannot
    // ride inside `apply_snap_frame`'s early return.
    report.snaps_staled += std::mem::take(&mut snaps.stale);
    report.race_dropped += std::mem::take(&mut snaps.race_dropped);
    // The state pass runs before the event drain: a repair byte
    // landing this frame records its snap tick in
    // `RemoteSnaps::repaired` before the queued impact rows are
    // judged, so a hit the authority wiped before publishing the snap
    // drops instead of splatting on top of the wipe that erased it.
    apply_snap_frame(
        &mut snaps,
        &mut session,
        &time,
        &mut commands,
        &mut players,
        &mut trailers,
        &mut mirror,
        &mut texel,
        &pool,
        &mut bangers,
        &mut banger_writer,
        &mut break_visuals,
        &render_parts,
        &mut report,
    );
    drain_pending_impacts(&mut snaps, &session, &players, &mut remote_fx, &mut report);
    // The v13 race row drains every run like the pending impacts —
    // it stages off the pose watermark precisely because the
    // authority's frozen countdown tick would otherwise hold it.
    if let Some((generation, row)) = snaps.race.take() {
        apply_race_snap(generation, row, &mut session, &mut mirror, &mut report);
    }
}

/// The v13 `Snap.race` row's consumer (F25-B): mirror the authority's
/// race phase and clock onto the session's [`RaceState`] — a predicted
/// client's `advance_race` never steps, so the wire is the race's only
/// clock. The generation gate matches the queued rows' (the wire
/// namespace, `0` never a live session's name); a row with no local
/// `RaceState` or a stale one drops counted — an event session's
/// resource exists from load, so a legit authority's row always has
/// its landing place. On the countdown → running/complete edge the
/// mirror performs the same release `advance_race` does: every
/// participant's `AwaitingStart` flips, the session moves to
/// `Playing`, and the one `RaceStarted` goes out for the GO
/// consumers. Per-seat checkpoint progress and results ride the
/// seat rows' v14 tail — see [`apply_progress`]; the phase word is
/// what unlocks control.
fn apply_race_snap(
    generation: u64,
    row: SnapRace,
    session: &mut Session,
    mirror: &mut RaceMirror,
    report: &mut NetDriveReport,
) {
    let Some(race) = mirror.race.as_deref_mut() else {
        // A race row for a raceless session — a cruise or dev world
        // has no `RaceState` to mirror into.
        report.race_dropped += 1;
        return;
    };
    if generation == 0
        || generation != session.wire_generation()
        || race.is_stale(session.generation())
    {
        report.race_dropped += 1;
        return;
    }
    let Some(phase) = race_phase(&row) else {
        // `push` already refused unnamed discriminants — defensive.
        report.race_dropped += 1;
        return;
    };
    let releasing = matches!(race.phase, RacePhase::Countdown { .. })
        && !matches!(phase, RacePhase::Countdown { .. });
    race.phase = phase;
    race.clock = row.clock;
    if releasing {
        for mut progress in mirror.progress.p0().iter_mut() {
            if progress.state == ParticipantState::AwaitingStart {
                progress.state = ParticipantState::Racing;
            }
        }
        // The wire's release moves the session `Countdown → Playing`
        // — the transition `advance_race` performs on the authority.
        // `Ready` releases too: snaps can land before the client's
        // own countdown edge, and a race that is already over on the
        // authority still means control is live.
        if matches!(
            session.phase(),
            SessionPhase::Countdown | SessionPhase::Ready
        ) && session.transition(SessionPhase::Playing).is_err()
        {
            report.race_dropped += 1;
            return;
        }
        mirror.started.write(RaceStarted);
    }
    report.race_applied += 1;
}

/// One seat's replicated [`SnapEntry::breaks`] bitmask diffed against
/// its rig (protocol v11, F25-B): a set bit on an attached part sheds
/// it — the intact node hides and the pooled fragment spawns carrying
/// the copy's replicated motion (the wire carries the detach *state*,
/// not the per-part launch impulse — designed presentation); a clear
/// bit on a detached part re-attaches it — the authority's repair
/// arriving as state, the same transition `resolve_disabled` +
/// `restore_rig` performs on the authority. Runs on every named seat,
/// the own seat included: `detach_breaks` is inert under a predicted
/// session, so the wire mask is the own rig's only detach truth —
/// mirrored the way the v8 damage byte is. Bits past the rig's part
/// count are unexpressible and ignored; the gameplay fingerprint makes
/// a longer wire rig impossible anyway.
#[allow(clippy::too_many_arguments)] // the fragment spawn genuinely threads the pool, the writers and the part/car geometry
fn apply_break_bits(
    entity: Entity,
    bits: u32,
    rig: &mut VehicleBreaks,
    pose: (Vec3, Quat),
    motion: (Vec3, Vec3),
    visuals: &mut Query<BreakVisualMut, Without<Banger>>,
    render_parts: &Query<(&Mesh3d, &MeshMaterial3d<StandardMaterial>, &ChildOf)>,
    bangers: &mut Query<BangerMut>,
    pool: &BangerPool,
    occupied: &mut Option<usize>,
    banger_writer: &mut MessageWriter<BangerStateChanged>,
    session: &mut Session,
    commands: &mut Commands,
    report: &mut NetDriveReport,
) {
    let owner = SessionEntity(session.generation());
    for i in 0..rig.parts.len().min(32) {
        let wire_detached = bits & (1 << i) != 0;
        if rig.parts[i].attached == !wire_detached {
            continue;
        }
        if wire_detached {
            let spec = rig.parts[i].spec.clone();
            // Same no-node refusal the authority applies: a part with
            // no render node cannot detach — an assembly inconsistency,
            // not data.
            let Some(node) = visuals
                .iter()
                .find(|(_, bpv, _, child)| child.parent() == entity && bpv.part == spec.name)
                .map(|(e, _, _, _)| e)
            else {
                continue;
            };
            let occupied = occupied.get_or_insert_with(|| {
                bangers
                    .iter()
                    .filter(|(_, _, b, _, _, _, _)| b.phase == BangerPhase::Active)
                    .count()
            });
            let spawned = breakaway::spawn_break_fragment(
                node,
                entity,
                i,
                &spec,
                pose,
                motion,
                None,
                owner,
                occupied,
                session,
                pool,
                bangers,
                banger_writer,
                visuals,
                render_parts,
                commands,
            );
            if rig.detach(i, spawned.map(|(e, _)| e)) {
                report.breaks_detached += 1;
            }
        } else {
            if let Some(fragment) = rig.attach(i) {
                commands.entity(fragment).despawn();
            }
            for (_, bpv, mut vis, child) in visuals.iter_mut() {
                if child.parent() == entity && bpv.part == rig.parts[i].spec.name {
                    *vis = Visibility::Visible;
                }
            }
            report.breaks_restored += 1;
        }
    }
}

/// The state half of [`apply_snapshots`]: apply the newest staged
/// snapshot's pose/velocity/presentation rows plus each entry's v8
/// damage byte — whose `>0 → 0` transition wipes the seat's texel rig
/// and records the snap tick in [`RemoteSnaps::repaired`] for the
/// drain that follows.
#[allow(clippy::too_many_arguments)] // Bevy system helper — the borrows are the contract.
fn apply_snap_frame(
    snaps: &mut RemoteSnaps,
    session: &mut Session,
    time: &Time,
    commands: &mut Commands,
    players: &mut Query<SnapTargetRow<'_>, SnapTargetFilter>,
    trailers: &mut Query<SnapTrailerRow<'_>, SnapTrailerFilter>,
    mirror: &mut RaceMirror,
    texel: &mut crate::texel_fx::TexelRepair,
    pool: &BangerPool,
    bangers: &mut Query<BangerMut>,
    banger_writer: &mut MessageWriter<BangerStateChanged>,
    visuals: &mut Query<BreakVisualMut, Without<Banger>>,
    render_parts: &Query<(&Mesh3d, &MeshMaterial3d<StandardMaterial>, &ChildOf)>,
    report: &mut NetDriveReport,
) {
    let Some(snap) = snaps.latest.take() else {
        return;
    };
    // A frame from another session is never applied — and a stale tick
    // inside this generation isn't either (physics only moves on fixed
    // steps, so a same-tick snap carries a duplicate pose). Either way
    // the queued impact rows still drain: events outlive the pose
    // frame that carried them. The frame's `generation` is the wire
    // namespace — a different authority's numbering can sit behind the
    // local id counter — and `0` is never a live session's name
    // (`begin_generation` refuses the at-rest value), so it drops like
    // any foreign frame even while `wire_generation()` still reads 0.
    if snap.generation == 0 || snap.generation != session.wire_generation() {
        return;
    }
    let stale = snaps
        .applied
        .is_some_and(|(g, t)| snap.generation == g && snap.tick <= t);
    if stale {
        // Unreachable through `push` (its watermark already gates
        // this), but the drop still counts if a future path stages one.
        report.snaps_staled += 1;
        return;
    }
    snaps.applied = Some((snap.generation, snap.tick));
    let now = time.elapsed_secs_f64();
    // The blend interval is the observed arrival gap, clamped so a
    // stalled stream doesn't smear a jump and a fast one doesn't snap.
    let interval = snaps
        .last_arrival
        .map(|prev| (now - prev).clamp(0.005, 0.5))
        .unwrap_or(0.0);
    snaps.last_arrival = Some(now);
    // Owners whose epoch advanced this frame — the trailer rows snap on
    // the same signal the seat does (v9, F25-B) — plus the local seat's
    // entity/wire pair the own rig's trailer keys off.
    let mut reset_owners: HashSet<u16> = HashSet::new();
    let mut own_seat: Option<(Entity, u16)> = None;
    // The frame's `Banger::active` census for the breakaway reconcile's
    // pool claims — one count serves every fragment this frame spawns.
    let mut occupied = None::<usize>;
    for entry in &snap.entries {
        for (
            entity,
            wire,
            player,
            mut epoch,
            mut pos,
            mut rot,
            mut vel,
            mut ang,
            lerp,
            vehicle,
            state,
            input,
            drive,
            damage,
            breaks,
        ) in players.iter_mut()
        {
            if wire.0 != entry.player {
                continue;
            }
            let to_pos = Vec3::from(entry.pos);
            let to_rot = wire_quat(entry.rot);
            let authority_reset = entry.epoch != epoch.0;
            epoch.0 = entry.epoch;
            if authority_reset {
                reset_owners.insert(entry.player);
            }
            if player.control == PlayerControl::Local {
                own_seat = Some((entity, wire.0));
            }
            // The v8 damage byte lands on every named seat, own seat
            // included: under a predicted session nothing local
            // accumulates `VehicleDamage`, so the replicated total is
            // the only truth the meter/smoke/impairment consumers can
            // read (F05 req 6). Applied on every snap, not just resets.
            if apply_damage(entity, entry, damage, texel, report) {
                // The wipe just ran — record the repair's snap tick so
                // the drain drops any still-queued pre-repair impact
                // row instead of splatting on top of it.
                snaps
                    .repaired
                    .insert(entry.player, (snap.generation, snap.tick));
            }
            // The v14 progress tail lands on every named wire seat
            // the race tracks — replicated `RaceProgress` mirrored
            // verbatim, terminal edges recording into this process's
            // `ResultLedger` like `advance_race` does on the
            // authority (F25-B). Runs before the own-seat branch's
            // `break` so the local participant resolves on the
            // wire's word too.
            if let Ok(mut progress) = mirror.progress.p1().get_mut(entity) {
                apply_progress(
                    entry,
                    player,
                    &mut progress,
                    session,
                    &mut mirror.ledger,
                    snap.tick,
                    report,
                );
            }
            // The v11 breakaway bitmask reconciles on every named
            // seat too — set bits shed the part's node onto a pooled
            // fragment, cleared bits put it back (the authority's
            // repair arriving as state).
            if let Some(mut rig) = breaks {
                apply_break_bits(
                    entity,
                    entry.breaks,
                    &mut rig,
                    (pos.0, rot.0),
                    (vel.0, ang.0),
                    visuals,
                    render_parts,
                    bangers,
                    pool,
                    &mut occupied,
                    banger_writer,
                    session,
                    commands,
                    report,
                );
            }
            // The own seat: only an authority reset may move it — the
            // host teleported our car (its copy of us is the truth),
            // so the predicted pose yields to the asserted one. Its
            // presentation fields stay ignored — the local sim's
            // `VehicleState`/`VehicleInput` is already the truth here.
            if player.control == PlayerControl::Local {
                if authority_reset {
                    *pos = Position(to_pos);
                    *rot = Rotation(to_rot);
                    *vel = LinearVelocity(Vec3::from(entry.vel));
                    *ang = AngularVelocity(Vec3::from(entry.angvel));
                    commands.entity(entity).insert(Teleported);
                    report.resets += 1;
                }
                break;
            }
            *vel = LinearVelocity(Vec3::from(entry.vel));
            *ang = AngularVelocity(Vec3::from(entry.angvel));
            // The v7 tail drives the copy's wheel/glow presentation
            // (F25-B) — the entity carries `RemoteReplica`, so nothing
            // local steps this state between snaps.
            if let (Some(vehicle), Some(mut state), Some(mut input)) = (vehicle, state, input) {
                apply_present(
                    entry,
                    &vehicle.config,
                    &mut state,
                    &mut input,
                    drive.map(|d| d.into_inner()),
                );
            }
            match lerp {
                Some(mut lerp) => {
                    if authority_reset || to_pos.distance(pos.0) > CORRECTION_SNAP_DIST {
                        // A teleport, not motion — the authority's
                        // reset/recovery moved the car. Snap rather
                        // than blend a slide through the world.
                        *pos = Position(to_pos);
                        *rot = Rotation(to_rot);
                        lerp.from_pos = to_pos;
                        lerp.from_rot = to_rot;
                    } else {
                        lerp.from_pos = pos.0;
                        lerp.from_rot = rot.0;
                    }
                    lerp.to_pos = to_pos;
                    lerp.to_rot = to_rot;
                    lerp.start = now;
                    lerp.end = now + interval;
                }
                // No blend state (shouldn't happen on a spawned copy) —
                // take the authoritative pose directly.
                None => {
                    *pos = Position(to_pos);
                    *rot = Rotation(to_rot);
                }
            }
            if authority_reset {
                commands.entity(entity).insert(Teleported);
                report.resets += 1;
            }
            break;
        }
    }
    // The v9 trailer rows (F25-B): a remote copy blends like its seat —
    // an owner-epoch advance snaps it — and the own rig's trailer snaps
    // only when *our* seat's epoch advanced (the authority reseated the
    // whole rig in the same broadcast).
    for t in &snap.trailers {
        for (
            entity,
            trailer,
            marker,
            mut pos,
            mut rot,
            mut vel,
            mut ang,
            lerp,
            drive,
            vehicle,
            mut state,
        ) in trailers.iter_mut()
        {
            let remote_copy = marker.is_some_and(|m| m.owner == t.owner);
            let own_rig = marker.is_none()
                && own_seat.is_some_and(|(e, w)| w == t.owner && trailer.towing == e);
            if !remote_copy && !own_rig {
                continue;
            }
            let to_pos = Vec3::from(t.pos);
            let to_rot = wire_quat(t.rot);
            let snap_to =
                reset_owners.contains(&t.owner) || to_pos.distance(pos.0) > CORRECTION_SNAP_DIST;
            // The own rig's trailer is a real body the local hitch
            // joint owns: like the own seat's epoch-equal entries, the
            // authority's lagged view of it is dropped — only a
            // declared reset (or a real divergence) reseats it, never a
            // ~20 Hz teleport fighting the joint.
            if own_rig && !snap_to {
                continue;
            }
            *vel = LinearVelocity(Vec3::from(t.vel));
            *ang = AngularVelocity(Vec3::from(t.angvel));
            if let Some(mut drive) = drive {
                drive.spin_rate = t.spin as f32 * 0.1;
            }
            // The row's only suspension truth — fold it into the
            // kinematic copy's wheel state. The own rig's trailer is a
            // real body whose `VehicleState` the local sim owns.
            if remote_copy && let (Some(vehicle), Some(state)) = (vehicle, state.as_deref_mut()) {
                apply_trailer_present(t, &vehicle.config, state);
            }
            match lerp {
                Some(mut lerp) if remote_copy && !snap_to => {
                    lerp.from_pos = pos.0;
                    lerp.from_rot = rot.0;
                    lerp.to_pos = to_pos;
                    lerp.to_rot = to_rot;
                    lerp.start = now;
                    lerp.end = now + interval;
                }
                // A snap — the owner's declared reset or a correction
                // past the snap distance — or the own rig's trailer,
                // which is a real body that takes its pose outright.
                _ => {
                    *pos = Position(to_pos);
                    *rot = Rotation(to_rot);
                    if let Some(mut lerp) = lerp {
                        lerp.from_pos = to_pos;
                        lerp.from_rot = to_rot;
                        lerp.to_pos = to_pos;
                        lerp.to_rot = to_rot;
                    }
                    if snap_to {
                        commands.entity(entity).insert(Teleported);
                    }
                }
            }
            report.trailers_synced += 1;
            break;
        }
    }
    report.snaps_applied += 1;
}

/// The event half of [`apply_snapshots`]: drain the queued replicated
/// impact rows into the [`RemoteImpact`] stream. Replicated impacts
/// are events, not state — the queue drains every run, not only when
/// a fresh frame applied (a superseded snap's poses drop, its effects
/// still land). It runs *after* the state pass so a repair byte
/// applied this frame is already in [`RemoteSnaps::repaired`]: a row
/// emitted at or before the repair's snap tick predates the wipe on
/// the authority (splat-then-wipe), so it drops rather than
/// re-stamping a hit the repair already erased. `push` already
/// deduped `(seat, id)` and binned foreign generations; here each row
/// gets the session gate, the own-seat skip — a predicted seat's
/// local physics stream already rendered the hit, so the wire row
/// must not double it — the wire sanitize, the repair-ledger
/// staleness check, and the resolve to this process's remote copy.
fn drain_pending_impacts(
    snaps: &mut RemoteSnaps,
    session: &Session,
    players: &Query<SnapTargetRow<'_>, SnapTargetFilter>,
    remote_fx: &mut MessageWriter<RemoteImpact>,
    report: &mut NetDriveReport,
) {
    report.impacts_dropped += snaps.dropped;
    snaps.dropped = 0;
    if snaps.pending.is_empty() {
        return;
    }
    // Rows queue stamped with their frame's wire generation — compare
    // against the session's wire namespace, not the local id counter.
    let generation = session.wire_generation();
    let seats: HashMap<u16, (Entity, bool)> = players
        .iter()
        .map(|(entity, wire, player, ..)| {
            (wire.0, (entity, player.control == PlayerControl::Local))
        })
        .collect();
    while let Some((row_gen, row)) = snaps.pending.pop_front() {
        // `0` is never a live session's name either — the at-rest
        // `wire_generation` of a never-begun session must not pass a
        // non-conforming peer's gen-0 rows.
        if row_gen == 0 || row_gen != generation {
            report.impacts_dropped += 1;
            continue;
        }
        let Some(&(entity, is_local)) = seats.get(&row.seat) else {
            // Departed or unspawned seat — nothing to present on.
            report.impacts_dropped += 1;
            continue;
        };
        if is_local {
            continue;
        }
        let sane = row.point.iter().all(|v| v.is_finite())
            && row.normal.iter().all(|v| v.is_finite())
            && row.severity.is_finite()
            && row.severity >= 0.0;
        if !sane {
            report.impacts_dropped += 1;
            continue;
        }
        // The seat's repair wiped the skin at or after this row's
        // emit tick — the authority ordered splat-then-wipe, so the
        // hit must not land on top of the wipe.
        if snaps
            .repaired
            .get(&row.seat)
            .is_some_and(|&(g, tick)| g == row_gen && row.tick <= tick)
        {
            report.impacts_dropped += 1;
            continue;
        }
        remote_fx.write(RemoteImpact {
            entity,
            point: Vec3::from_array(row.point),
            normal: Vec3::from_array(row.normal)
                .try_normalize()
                .unwrap_or(Vec3::Y),
            severity: row.severity,
            audio_id: row.audio_id,
        });
        report.impacts_applied += 1;
    }
}

/// The remote-copy blend query row — pose, the `RemoteLerp` window and
/// the optional drive state the v7 tail feeds (a `RemotePick` without
/// a vehicle bundle still blends its pose).
type LerpRow<'a> = (
    &'a mut Position,
    &'a mut Rotation,
    &'a RemoteLerp,
    Option<&'a mut VehicleState>,
    Option<&'a RemoteDrive>,
);

/// Advance remote copies along their [`RemoteLerp`] blend — one blend
/// interval behind the wire, so motion is smooth rather than
/// snap-to-pose. Kinematic bodies take their pose from `Position`.
/// Also integrates the newest replicated wheel rate
/// ([`RemoteDrive::spin_rate`], F25-B) into each `WheelState::spin` —
/// the copy's wheels visibly turn between snapshots, and a stalled
/// stream freezes them rather than extrapolating.
pub fn drive_remote_lerp(
    time: Res<Time>,
    mut remotes: Query<LerpRow, With<RemotePick>>,
    mut report: ResMut<NetDriveReport>,
) {
    let now = time.elapsed_secs_f64();
    let dt = time.delta_secs();
    for (mut pos, mut rot, lerp, state, drive) in &mut remotes {
        let span = (lerp.end - lerp.start).max(f64::EPSILON);
        let t = ((now - lerp.start) / span).clamp(0.0, 1.0) as f32;
        pos.0 = lerp.from_pos.lerp(lerp.to_pos, t);
        rot.0 = wire_quat(lerp.from_rot.slerp(lerp.to_rot, t).to_array());
        let (Some(mut state), Some(drive)) = (state, drive) else {
            continue;
        };
        let step = drive.spin_rate * dt;
        if step != 0.0 && !state.wheels.is_empty() {
            for ws in &mut state.wheels {
                ws.spin += step;
            }
            report.remote_spin += step.abs() as f64;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The quantized wire sample is the exact complement of the input
    /// dequantize — extremes and a midpoint both ways.
    #[test]
    fn drive_input_quantization_round_trips() {
        let input = VehicleInput {
            throttle: 1.0,
            brake: 0.5,
            steering: -1.0,
            handbrake: 0.0,
            forced_gear: Some(3),
        };
        let wire = encode_input(&input, 4, 9);
        assert_eq!(wire.generation, 4);
        assert_eq!(wire.seq, 9);
        assert_eq!(wire.throttle, 255);
        assert_eq!(wire.brake, 128);
        assert_eq!(wire.steer, -127);
        assert_eq!(wire.handbrake, 0);
        let back = decode_input(&wire);
        assert_eq!(back.throttle, 1.0);
        assert!((back.brake - 0.5).abs() < 0.01);
        assert_eq!(back.steering, -1.0);
        assert_eq!(back.handbrake, 0.0);
        // A local gear command never rides the wire.
        assert_eq!(back.forced_gear, None);
    }

    /// Out-of-range analog values clamp rather than wrap the integer
    /// fields — a NaN or stray >1 input cannot smear the wire sample.
    #[test]
    fn drive_input_quantization_clamps() {
        let wire = encode_input(
            &VehicleInput {
                throttle: 4.0,
                brake: -1.0,
                steering: 99.0,
                handbrake: f32::NAN,
                forced_gear: None,
            },
            0,
            0,
        );
        assert_eq!(wire.throttle, 255);
        assert_eq!(wire.brake, 0);
        assert_eq!(wire.steer, 127);
        // NaN clamps to a valid sample, never a wrap.
        assert_eq!(wire.handbrake, 0);
    }

    /// A corrupt rotation off the wire — NaNs or a zero quaternion —
    /// resolves to identity rather than poisoning the pose.
    #[test]
    fn wire_rotations_are_sanitized() {
        assert_eq!(wire_quat([f32::NAN; 4]), Quat::IDENTITY);
        assert_eq!(wire_quat([0.0; 4]), Quat::IDENTITY);
        assert_eq!(wire_quat([0.0, 0.0, 0.0, 1.0]), Quat::IDENTITY);
        let q = wire_quat([
            0.0,
            0.0,
            std::f32::consts::FRAC_1_SQRT_2,
            std::f32::consts::FRAC_1_SQRT_2,
        ]);
        assert!((q.length() - 1.0).abs() < 1e-4);
    }

    /// Host-side epochs: a `ResetVehicle` landing on a participant bumps
    /// its counter, a non-participant target (a trailer) touches none,
    /// and a reset-all bumps every participant. The wrapping counter
    /// rolls at 256 — a wrap is a false snap, never a missed reset.
    #[test]
    fn reset_vehicle_events_bump_the_seat_epoch() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<ResetVehicle>()
            .init_resource::<NetDriveReport>()
            .add_systems(Update, track_reset_epochs);
        let car = app.world_mut().spawn(ResetEpoch(0)).id();
        let trailer = app.world_mut().spawn_empty().id();

        let write = |app: &mut App, entity: Option<Entity>| {
            app.world_mut()
                .resource_mut::<Messages<ResetVehicle>>()
                .write(ResetVehicle {
                    entity,
                    position: Vec3::ZERO,
                    yaw: 0.0,
                });
        };

        write(&mut app, Some(car));
        app.update();
        assert_eq!(app.world().get::<ResetEpoch>(car).unwrap().0, 1);

        write(&mut app, Some(trailer));
        app.update();
        assert_eq!(
            app.world().get::<ResetEpoch>(car).unwrap().0,
            1,
            "a non-participant reset touches no epoch"
        );

        write(&mut app, None);
        app.update();
        assert_eq!(
            app.world().get::<ResetEpoch>(car).unwrap().0,
            2,
            "a reset-all bumps every participant"
        );
        assert_eq!(app.world().resource::<NetDriveReport>().resets, 2);
    }

    fn roster_entry(id: u16) -> mm2_net::RosterEntry {
        mm2_net::RosterEntry {
            player_id: id,
            driver: format!("p{id}"),
            build: String::new(),
            ready: true,
            pick: Some(VehiclePick {
                vehicle: "vpbug".into(),
                paint: 0,
            }),
        }
    }

    /// A `RaceDefinition` carrying just enough for the `seat_pose`
    /// legs — authored slots plus one course-defining gate 200 m out
    /// on −Z, so a slot at the origin resolves `course_yaw` to 0
    /// exactly (`atan2(-0, 200)`).
    fn grid_def(slots: &[(f32, f32, Option<f32>)]) -> RaceDefinition {
        RaceDefinition {
            checkpoints: vec![mm2_game::Checkpoint {
                center: Vec3::new(0.0, 0.0, -200.0),
                radius: 15.0,
                height: mm2_game::DEFAULT_CHECKPOINT_HEIGHT,
                heading_deg: 0.0,
                require_direction: false,
            }],
            finish: None,
            rule: mm2_game::CheckpointRule::AnyOrder,
            laps: 0,
            time_limit_ticks: None,
            params: mm2_game::EventParams::default(),
            countdown_ticks: 1,
            start_slots: slots
                .iter()
                .map(|&(x, z, yaw_deg)| mm2_game::RaceStart {
                    position: Vec3::new(x, 0.0, z),
                    yaw_deg,
                })
                .collect(),
        }
    }

    /// The seat map is the lobby's wire ids ranked ascending — the same
    /// list on every process — with the host seat in only while a host
    /// is playing, and our own id included once.
    #[test]
    fn seat_ids_rank_the_lobby_deterministically() {
        let mut lobby = LobbyState {
            roster: vec![roster_entry(3), roster_entry(1)],
            host_pick: Some(roster_entry(0).pick.unwrap()),
            ..LobbyState::default()
        };

        // A joined client's view: self 2 on the roster view plus the
        // host seat the `Start` pick announced → 0 ranks first.
        assert_eq!(
            seat_ids(Some(&lobby), true, Some(2)),
            vec![0, 1, 2, 3],
            "seats sort by wire id, self deduped against the roster"
        );

        // A hosted app's view: self 0, remotes on the roster.
        assert_eq!(seat_ids(Some(&lobby), true, Some(0)), vec![0, 1, 3]);

        // A dedicated-host client's view: no seat 0 anywhere, so the
        // first roster member takes seat 0's slot.
        lobby.host_pick = None;
        assert_eq!(seat_ids(Some(&lobby), false, Some(2)), vec![1, 2, 3]);

        // Solo — no lobby at all — is one seat.
        assert_eq!(seat_ids(None, false, None), Vec::<u16>::new());
        assert_eq!(seat_index(&[], None), 0);
    }

    /// Each seat consumes one authored `_strtpnts` row in order; a slot
    /// carrying the authored "no heading" sentinel resolves to the
    /// course-facing yaw instead of verbatim 0.
    #[test]
    fn seat_pose_takes_authored_slots_in_order() {
        let def = grid_def(&[(10.0, 0.0, Some(90.0)), (0.0, 0.0, None)]);
        // A non-zero base yaw — seat 1 falling back to it (instead of
        // the course facing) would fail the assertion below.
        let base = (Vec3::ZERO, 0.7);
        // Seat 0 gets row 0 verbatim — position and authored yaw.
        let (p0, y0) = seat_pose(Some(&def), base, 0);
        assert_eq!(p0, Vec3::new(10.0, 0.0, 0.0));
        assert!((y0 - 90f32.to_radians()).abs() < 1e-4);
        // Seat 1's `None` yaw resolves the course facing — the gate
        // sits −Z of the origin slot → `atan2(-0, 200)` = 0, not the
        // base yaw and not a verbatim 0 row read.
        let (p1, y1) = seat_pose(Some(&def), base, 1);
        assert_eq!(p1, Vec3::ZERO);
        assert!(y1.abs() < 1e-4, "course-facing yaw, got {y1}");
    }

    /// Grid exhaustion fans the extra seats out on the last slot's
    /// right — deterministic, unbounded by the authored row count.
    #[test]
    fn seat_pose_fans_out_past_the_grid() {
        let def = grid_def(&[(10.0, 0.0, Some(90.0))]);
        let (p, yaw) = seat_pose(Some(&def), (Vec3::ZERO, 0.0), 2);
        // Seat 2 = the one-row grid's last slot + two seat-gaps along
        // its right vector.
        let yaw90 = 90f32.to_radians();
        let right = Vec3::new(yaw90.cos(), 0.0, -yaw90.sin());
        let expect = Vec3::new(10.0, 0.0, 0.0) + right * SEAT_STAGE_GAP * 2.0;
        assert!((p - expect).length() < 1e-3);
        assert!((yaw - yaw90).abs() < 1e-4);

        // An empty grid treats the base pose as the anchor — seat 1
        // lands one gap right of it, facing the base yaw.
        let empty = grid_def(&[]);
        let (p, yaw) = seat_pose(
            Some(&empty),
            (Vec3::new(5.0, 0.0, 5.0), std::f32::consts::PI),
            1,
        );
        let right = Vec3::new(std::f32::consts::PI.cos(), 0.0, -std::f32::consts::PI.sin());
        assert!((p - (Vec3::new(5.0, 0.0, 5.0) + right * SEAT_STAGE_GAP)).length() < 1e-3);
        assert_eq!(yaw, std::f32::consts::PI);
    }

    /// Dev worlds carry no race at all — seats fan out on the spawn
    /// base exactly like an empty grid does.
    #[test]
    fn seat_pose_without_a_race_fans_off_the_base() {
        let base = (Vec3::new(0.0, 1.5, 0.0), 0.0);
        assert_eq!(seat_pose(None, base, 0), base);
        let (p, yaw) = seat_pose(None, base, 1);
        // Yaw 0 → forward (0,0,-1), right (1,0,0).
        assert!((p - Vec3::new(SEAT_STAGE_GAP, 1.5, 0.0)).length() < 1e-3);
        assert_eq!(yaw, 0.0);
    }

    /// `apply_seat` writes the resolved pose while keeping the base
    /// origin — the pole anchor `spawn_pose`'s AI fallback still needs.
    #[test]
    fn apply_seat_moves_the_spawn_but_keeps_its_origin() {
        let def = grid_def(&[(10.0, 0.0, Some(90.0)), (14.0, 0.0, Some(90.0))]);
        let mut spawn = SpawnPoint::new(Vec3::new(0.0, 1.5, 0.0), 0.0);
        apply_seat(&mut spawn, Some(&def), 1);
        assert_eq!(spawn.position, Vec3::new(14.0, 0.0, 0.0));
        assert!((spawn.yaw - 90f32.to_radians()).abs() < 1e-4);
        // Origin stays the pole anchor.
        assert_eq!(spawn.origin, Vec3::new(0.0, 1.5, 0.0));
        assert_eq!(spawn.origin_yaw, 0.0);
        // Seat 0 was untouched — reseating is idempotent for index 0.
        let mut solo = SpawnPoint::new(Vec3::new(0.0, 1.5, 0.0), 0.25);
        apply_seat(&mut solo, Some(&def), 0);
        assert_eq!(solo.position, Vec3::new(10.0, 0.0, 0.0));
        assert!((solo.yaw - 90f32.to_radians()).abs() < 1e-4);
    }

    /// The v7 presentation tail encodes the sim's own aggregates: the
    /// actual steer angle in milliradians, the *grounded* wheels' mean
    /// `vel_long / radius` (an airborne wheel neither spins up nor
    /// skews the rate — the sim holds its angle), and the mean
    /// `compression / travel` fraction over all wheels.
    #[test]
    fn present_tail_encodes_the_sim_state() {
        let cfg = VehicleConfig::default(); // dev car: 4 wheels, r 0.34, travel 0.35
        let mut state = VehicleState::new(&cfg);
        state.steer_angle = 0.32;
        state.direction = DriveDirection::Reverse;
        state.grounded = true;
        // The grounded fronts roll at 6.8 m/s patch speed (20 rad/s);
        // the lifted rears are excluded from the rate but still count
        // toward the compression mean.
        for ws in &mut state.wheels[..2] {
            ws.grounded = true;
            ws.vel_long = 6.8;
            ws.compression = 0.14;
        }
        let input = VehicleInput {
            brake: 1.0,
            ..VehicleInput::default()
        };
        let (steer, spin, compression, flags) = encode_present(&cfg, &state, &input);
        assert_eq!(steer, 320);
        assert_eq!(spin, 200, "6.8 / 0.34 rad/s at 0.1 rad/s units");
        assert_eq!(
            compression, 51,
            "two wheels at 0.4 of travel over four wheels: 0.2 * 255"
        );
        assert_eq!(
            flags,
            SNAP_FLAG_BRAKE | SNAP_FLAG_REVERSE | SNAP_FLAG_GROUNDED
        );
    }

    /// Extremes saturate rather than wrap; a fully airborne car reports
    /// rate 0 — the sim's own rule holds a lifted wheel's angle.
    #[test]
    fn present_tail_saturates_and_airborne_freezes() {
        let cfg = VehicleConfig::default();
        let mut state = VehicleState::new(&cfg);
        state.steer_angle = 45.0; // impossible, but the encode must bound it
        for ws in &mut state.wheels {
            ws.vel_long = -5000.0; // airborne: stale patch speed must not spin
        }
        let (steer, spin, _, flags) = encode_present(&cfg, &state, &VehicleInput::default());
        assert_eq!(steer, i16::MAX);
        assert_eq!(spin, 0, "no grounded wheel means a frozen spin");
        assert_eq!(flags, 0);
    }

    /// The receiving half writes the copy's `VehicleState`/`VehicleInput`
    /// the glow/wheel systems read — and clamps a hostile steer field
    /// instead of trusting the wire.
    #[test]
    fn apply_present_drives_the_copy_state() {
        let cfg = VehicleConfig::default();
        let mut state = VehicleState::new(&cfg);
        let mut input = VehicleInput::default();
        let mut drive = RemoteDrive::default();
        let entry = SnapEntry {
            player: 0,
            pos: [0.0; 3],
            rot: [0.0, 0.0, 0.0, 1.0],
            vel: [0.0; 3],
            angvel: [0.0; 3],
            epoch: 0,
            steer: -260,
            spin: 150,
            compression: 102, // 0.4 of travel
            flags: SNAP_FLAG_BRAKE | SNAP_FLAG_GROUNDED,
            damage: 0,
            breaks: 0,
            ..SnapEntry::default()
        };
        apply_present(&entry, &cfg, &mut state, &mut input, Some(&mut drive));
        assert_eq!(state.steer_angle, -0.26);
        assert_eq!(state.direction, DriveDirection::Forward);
        assert!(state.grounded);
        for (wheel, ws) in cfg.wheels.iter().zip(state.wheels.iter()) {
            assert!(ws.grounded);
            assert!(
                (ws.compression
                    - 0.4 * wheel.suspension.as_ref().unwrap_or(&cfg.suspension).travel)
                    .abs()
                    < 1e-3,
                "the fraction lands on each wheel's own travel"
            );
        }
        assert_eq!(input.brake, 1.0);
        assert_eq!(drive.spin_rate, 15.0);

        // A hostile tail clamps: 32 rad of steer becomes the bound.
        let hostile = SnapEntry {
            steer: i16::MAX,
            ..entry
        };
        apply_present(&hostile, &cfg, &mut state, &mut input, None);
        assert_eq!(state.steer_angle, MAX_WIRE_STEER);
    }

    /// The v8 damage byte encodes the authority's fraction of the
    /// authored `MaxDamage` — saturating at the bound, `0` for a seat
    /// with no damage record or a degenerate one.
    #[test]
    fn damage_tail_encodes_the_authority_fraction() {
        const SPEC: DamageSpec = DamageSpec {
            impact_threshold: 1500.0,
            med_damage: 150_000.0,
            max_damage: 321_300.0,
            regenerate_rate: 0.0,
        };
        // No authored record → undamageable reads as undamaged.
        assert_eq!(encode_damage(None), 0);
        let mut damage = VehicleDamage::new(SPEC);
        assert_eq!(encode_damage(Some(&damage)), 0, "intact");
        damage.apply(mm2_game::ImpactId(1), SPEC.max_damage * 0.5);
        assert_eq!(encode_damage(Some(&damage)), 128, "half of max");
        damage.apply(mm2_game::ImpactId(2), SPEC.max_damage);
        assert_eq!(
            encode_damage(Some(&damage)),
            255,
            "the saturating accumulator can never exceed the bound"
        );
        // A sub-byte positive total still encodes nonzero — byte 0 is
        // the repair signal, so a rounding-to-zero fraction must not
        // mint a wipe the authority never performed. (`apply` can't
        // land here — an accepted severity always clears ~0.4% of max —
        // but a regenerating total could, so the encoder guards it.)
        let mut graze = VehicleDamage::new(SPEC);
        graze.set_replicated(0.001);
        assert!(graze.total() > 0.0);
        assert_eq!(
            encode_damage(Some(&graze)),
            1,
            "any positive damage encodes at least byte 1"
        );
        // A degenerate spec (max <= 0) encodes 0 rather than dividing
        // by it.
        let degenerate = VehicleDamage::new(DamageSpec {
            max_damage: 0.0,
            ..SPEC
        });
        assert_eq!(encode_damage(Some(&degenerate)), 0);
    }

    /// The v11 breakaway bitmask encodes the rig's detached set in
    /// authored order — `None` and an intact rig both read 0 — and
    /// parts past bit 31 are unexpressible by design (far past any
    /// authored count, and the gameplay fingerprint keeps every
    /// process's rig identical anyway).
    #[test]
    fn breaks_tail_encodes_the_detached_mask() {
        let part = |name: String| BreakPartSpec {
            name,
            def: mm2_game::BangerDefinition {
                name: "frag".into(),
                mass: 100.0,
                friction: 0.9,
                elasticity: 0.3,
                impulse_limit2: 500.0,
                size: [0.8, 0.4, 1.2],
                cg: [0.0, 0.2, 0.0],
                num_parts: 0,
                audio_id: 0,
            },
        };
        assert_eq!(encode_breaks(None), 0, "no authored rig reads intact");
        let mut rig =
            VehicleBreaks::new(vec![part("a".into()), part("b".into()), part("c".into())]);
        assert_eq!(encode_breaks(Some(&rig)), 0);
        rig.detach(0, None);
        rig.detach(2, None);
        assert_eq!(encode_breaks(Some(&rig)), 0b101);
        // Part 33 cannot ride a u32 mask.
        let mut wide = VehicleBreaks::new((0..33).map(|i| part(format!("p{i}"))).collect());
        wide.detach(32, None);
        assert_eq!(encode_breaks(Some(&wide)), 0);
        wide.detach(1, None);
        assert_eq!(encode_breaks(Some(&wide)), 0b10);
    }

    /// The predicted trailer copy lands the same contract the
    /// authority's real body and the local trailer carry — `DamageSignals`
    /// included, so a client's impact-signal consumers see the remote
    /// rig's trailer contacts — plus the kinematic `RemoteReplica`/
    /// `RemoteLerp`/`RemoteDrive` rig the snap rows drive.
    #[test]
    fn a_predicted_trailer_copy_carries_damage_signals() {
        let trailer = mm2_content::TrailerDef {
            config: VehicleConfig {
                trailer: true,
                ..VehicleConfig::default()
            },
            model: mm2_content::VehicleModel::default(),
            car_hitch: [0.0, 0.0, 1.9],
            trailer_hitch: [0.0, 0.0, -2.4],
            wheels: Vec::new(),
        };
        let pick = VehiclePick {
            vehicle: "vpsemi".into(),
            paint: 0,
        };
        let mut world = World::new();
        let mut session = Session::new();
        let car = world.spawn_empty().id();
        let te = {
            let mut queue = bevy::ecs::world::CommandQueue::default();
            let mut commands = Commands::new(&mut queue, &world);
            let te = spawn_trailer_copy(
                &mut commands,
                &mut session,
                &trailer,
                &pick,
                3,
                car,
                Vec3::new(2.0, 0.6, -4.0),
                0.5,
                SessionEntity(7),
                mm2_game::AuthorityRole::Predicted,
            );
            queue.apply(&mut world);
            te
        };
        assert!(
            world.get::<DamageSignals>(te).is_some(),
            "the copy accumulates impact signals like every trailer"
        );
        assert!(
            matches!(world.get::<RigidBody>(te), Some(RigidBody::Kinematic)),
            "a client's copy is kinematic — the wire owns its pose"
        );
        assert!(world.get::<RemoteReplica>(te).is_some());
        assert!(world.get::<RemoteDrive>(te).is_some());
        assert!(world.get::<RemoteLerp>(te).is_some());
        assert_eq!(
            world.get::<RemoteTrailer>(te).unwrap().owner,
            3,
            "keyed by the towing seat's wire id"
        );
        let link = world.get::<car_visual::Trailer>(te).unwrap();
        assert_eq!(link.towing, car);
        assert!(
            (link.rest_offset - Vec3::new(0.0, 0.0, 4.3)).length() < 1e-3,
            "car_hitch - trailer_hitch, matching spawn_trailer's geometry"
        );
        // The copy starts hitched: trailer origin sits at the
        // yaw-rotated rest offset off the car's pose.
        let want =
            Vec3::new(2.0, 0.6, -4.0) + Quat::from_rotation_y(0.5) * Vec3::new(0.0, 0.0, 4.3);
        assert!(
            (world.get::<Transform>(te).unwrap().translation - want).length() < 1e-3,
            "spawned at the hitched rest pose"
        );
        assert_eq!(world.get::<SessionEntity>(te).unwrap().0, 7);
        assert!(world.get::<ObjectIdentity>(te).is_some());
    }

    fn snap_entry() -> SnapEntry {
        SnapEntry {
            player: 1,
            pos: [0.0; 3],
            rot: [0.0, 0.0, 0.0, 1.0],
            vel: [0.0; 3],
            angvel: [0.0; 3],
            epoch: 0,
            steer: 0,
            spin: 0,
            compression: 0,
            flags: 0,
            damage: 0,
            breaks: 0,
            ..SnapEntry::default()
        }
    }

    fn snap_impact(id: u64) -> SnapImpact {
        SnapImpact {
            seat: 1,
            id,
            tick: 0,
            point: [0.0; 3],
            normal: [0.0, 1.0, 0.0],
            severity: 1.0,
            audio_id: 0,
        }
    }

    /// `push` is latest-wins on the frame's own `(generation, tick)`,
    /// not arrival order: a reordered straggler or duplicated copy at
    /// or behind the staged-or-applied watermark drops counted instead
    /// of displacing the newer pose — while its impact rows still
    /// queue (events outlive their frame).
    #[test]
    fn a_stale_snap_drops_at_push_but_keeps_its_events() {
        let mut snaps = RemoteSnaps::default();
        snaps.push(1, 10, vec![snap_entry()], Vec::new(), Vec::new(), None);
        // A reordered straggler cannot displace the newer staged pose.
        snaps.push(
            1,
            9,
            vec![snap_entry()],
            Vec::new(),
            vec![snap_impact(7)],
            None,
        );
        assert_eq!(snaps.latest.as_ref().unwrap().tick, 10);
        assert_eq!(snaps.stale, 1);
        assert_eq!(
            snaps.pending.len(),
            1,
            "the stale frame's impact rows still queue"
        );
        // A duplicated copy of the staged frame is stale too — and its
        // repeat impact row hits the dedup window.
        snaps.push(
            1,
            10,
            vec![snap_entry()],
            Vec::new(),
            vec![snap_impact(7)],
            None,
        );
        assert_eq!(snaps.stale, 2);
        assert_eq!(snaps.pending.len(), 1);
        // Latest-wins still moves forward.
        snaps.push(1, 11, vec![snap_entry()], Vec::new(), Vec::new(), None);
        assert_eq!(snaps.latest.as_ref().unwrap().tick, 11);
        // The applied watermark gates too — nothing staged needed.
        snaps.applied = Some((1, 11));
        snaps.latest = None;
        snaps.push(1, 11, vec![snap_entry()], Vec::new(), Vec::new(), None);
        assert!(snaps.latest.is_none());
        assert_eq!(snaps.stale, 3);
        // A new generation is always newer — a session restart never
        // reads as a straggler of the last one.
        snaps.push(2, 1, vec![snap_entry()], Vec::new(), Vec::new(), None);
        assert_eq!(snaps.latest.as_ref().unwrap().generation, 2);
        assert_eq!(snaps.stale, 3);
    }

    /// `apply_snapshots` folds the push-time stale count into
    /// `NetDriveReport` even on a run where nothing applies — the fold
    /// sits outside `apply_snap_frame`'s early return.
    #[test]
    fn apply_snapshots_reports_the_stale_drops() {
        let mut session = Session::new();
        session
            .begin(mm2_game::SessionConfig {
                authority: mm2_game::SessionAuthority::Remote,
                ..mm2_game::SessionConfig::default()
            })
            .unwrap();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Playing).unwrap();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(session)
            .init_resource::<RemoteSnaps>()
            .init_resource::<NetDriveReport>()
            .init_resource::<crate::texel_fx::TexelDamageReport>()
            .add_message::<RemoteImpact>()
            .add_message::<RaceStarted>()
            // The v11 breakaway reconcile's pool claims and lifecycle
            // stream — never exercised by this leg, but the system's
            // parameters require them registered.
            .add_message::<BangerStateChanged>()
            .init_resource::<BangerPool>()
            // The v14 progress tails' terminal edges record into the
            // session ledger — never exercised by every leg, but the
            // system's parameters require it registered.
            .init_resource::<ResultLedger>()
            .add_systems(Update, apply_snapshots);

        // A stale push with nothing staged still folds — the count is
        // the client's evidence a reordered/duplicated stream dropped.
        let mut snaps = app.world_mut().resource_mut::<RemoteSnaps>();
        snaps.push(1, 5, vec![snap_entry()], Vec::new(), Vec::new(), None);
        snaps.push(1, 4, vec![snap_entry()], Vec::new(), Vec::new(), None);
        app.update();
        let report = app.world().resource::<NetDriveReport>();
        assert_eq!(report.snaps_applied, 1);
        assert_eq!(report.snaps_staled, 1);
        // The next stale drop rides a run with no staged snap at all.
        app.world_mut().resource_mut::<RemoteSnaps>().push(
            1,
            3,
            vec![snap_entry()],
            Vec::new(),
            Vec::new(),
            None,
        );
        app.update();
        assert_eq!(app.world().resource::<NetDriveReport>().snaps_staled, 2);
    }

    /// `reset` ends a stream at its authority boundary: the staged
    /// frame, watermarks, repair ledger and dedup window all describe
    /// the dead authority's `(generation, tick)` sequence — a fresh
    /// authority restarting its numbering must stage and apply
    /// cleanly rather than stale-drop under the dead watermark. The
    /// report's stale/dropped evidence survives the reset, and
    /// still-queued impact rows fold into `dropped` rather than
    /// vanishing.
    #[test]
    fn a_stream_reset_rebases_the_inbox_on_a_new_authority() {
        let mut snaps = RemoteSnaps::default();
        snaps.push(
            2,
            900,
            vec![snap_entry()],
            Vec::new(),
            vec![snap_impact(7)],
            None,
        );
        // A stale drop for the evidence counter, then clear the staged
        // frame and mark the watermark applied — the state a live
        // stream carries when its authority dies.
        snaps.push(2, 800, vec![snap_entry()], Vec::new(), Vec::new(), None);
        snaps.applied = Some((2, 900));
        snaps.latest = None;
        snaps.repaired.insert(1, (2, 800));
        assert_eq!(snaps.stale, 1);
        assert_eq!(snaps.pending.len(), 1);

        snaps.reset();
        assert!(snaps.latest.is_none());
        assert_eq!(snaps.applied, None);
        assert!(snaps.last_arrival.is_none());
        assert!(snaps.pending.is_empty());
        assert_eq!(snaps.dropped, 1, "the queued row folds into the drop count");
        assert!(snaps.repaired.is_empty());
        assert!(snaps.seen.is_empty() && snaps.seen_order.is_empty());
        assert_eq!(snaps.stale, 1, "the stale evidence is preserved");

        // The restarted sequence is not a straggler of the dead
        // stream: a fresh authority's generation-1 frame stages even
        // under the old (2, 900) watermark — and the dedup window's
        // memory is gone, so an impact id the old stream already saw
        // queues again under the new authority.
        snaps.push(
            1,
            1,
            vec![snap_entry()],
            Vec::new(),
            vec![snap_impact(7)],
            None,
        );
        assert_eq!(snaps.latest.as_ref().unwrap().generation, 1);
        assert_eq!(snaps.stale, 1);
        assert_eq!(snaps.pending.len(), 1, "the new stream's rows queue");
    }

    /// The accept-to-begin gap (F25-B): a parked `Start`'s new stream
    /// can queue impact rows while the outgoing session still tears
    /// down — `apply_snapshots` runs every `Update`, gated on the
    /// not-yet-adopted wire generation. Foreign-generation rows
    /// drain-drop counted rather than landing on the dying session or
    /// lingering past the begin; a row stamped `0` drops the same way
    /// — the at-rest value is never a session's name, on the frame
    /// gate and the row gate alike.
    #[test]
    fn foreign_generation_impact_rows_drain_drop_at_the_session_gate() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(Session::new())
            .init_resource::<RemoteSnaps>()
            .init_resource::<NetDriveReport>()
            .init_resource::<crate::texel_fx::TexelDamageReport>()
            .add_message::<RemoteImpact>()
            .add_message::<RaceStarted>()
            .add_message::<BangerStateChanged>()
            .init_resource::<BangerPool>()
            // The v14 progress tails' terminal edges record into the
            // session ledger — never exercised by every leg, but the
            // system's parameters require it registered.
            .init_resource::<ResultLedger>()
            .add_systems(Update, apply_snapshots);
        // A remote seat for the drain to resolve — the required half
        // of the `SnapTargetRow` tuple, nothing more.
        app.world_mut().spawn((
            NetPlayer(7),
            Player {
                id: mm2_game::PlayerId(9),
                control: PlayerControl::Remote,
            },
            ResetEpoch(0),
            Position(Vec3::ZERO),
            Rotation(Quat::IDENTITY),
            LinearVelocity(Vec3::ZERO),
            AngularVelocity(Vec3::ZERO),
        ));

        // At rest (`wire_generation() == 0`) gen-0 wire traffic is not
        // "this session's" — the frame and its rows drop.
        let mut row = snap_impact(1);
        row.seat = 7;
        app.world_mut().resource_mut::<RemoteSnaps>().push(
            0,
            10,
            Vec::new(),
            Vec::new(),
            vec![row],
            None,
        );
        app.update();
        let report = app.world().resource::<NetDriveReport>();
        assert_eq!(report.snaps_applied, 0, "a gen-0 frame is foreign");
        assert_eq!(report.impacts_dropped, 1);
        assert_eq!(report.impacts_applied, 0);

        // A live session adopts wire generation 1 — its own rows land.
        {
            let mut session = app.world_mut().resource_mut::<Session>();
            session
                .begin_generation(
                    mm2_game::SessionConfig {
                        authority: mm2_game::SessionAuthority::Remote,
                        ..mm2_game::SessionConfig::default()
                    },
                    1,
                )
                .unwrap();
            session.transition(SessionPhase::Ready).unwrap();
            session.transition(SessionPhase::Playing).unwrap();
        }
        app.world_mut().resource_mut::<RemoteSnaps>().push(
            1,
            10,
            Vec::new(),
            Vec::new(),
            vec![row],
            None,
        );
        app.update();
        let report = app.world().resource::<NetDriveReport>();
        assert_eq!(report.impacts_applied, 1, "the seat/gen-matching row lands");
        assert_eq!(report.snaps_applied, 1);

        // The boundary: `start()` resets the inbox on accept while the
        // parked session tears down — rows the *next* stream queues in
        // the gap are stamped with its generation, so the drain reads
        // them as foreign against the not-yet-adopted wire value and
        // drops them counted rather than holding them for the begin.
        {
            let mut gap_row = snap_impact(2);
            gap_row.seat = 7;
            let mut snaps = app.world_mut().resource_mut::<RemoteSnaps>();
            snaps.reset();
            snaps.push(2, 5, Vec::new(), Vec::new(), vec![gap_row], None);
        }
        {
            let mut session = app.world_mut().resource_mut::<Session>();
            session.transition(SessionPhase::Unloading).unwrap();
            session.transition(SessionPhase::Menu).unwrap();
        }
        app.update();
        let report = app.world().resource::<NetDriveReport>();
        assert_eq!(
            report.impacts_dropped, 2,
            "the new stream's queued row drains out as foreign"
        );
        assert_eq!(report.impacts_applied, 1);
        assert_eq!(report.snaps_applied, 1, "its frame is foreign too");
        assert!(
            app.world().resource::<RemoteSnaps>().pending.is_empty(),
            "foreign rows drop, they do not linger past the begin"
        );

        // The begin adopts the new generation — the stream's rows then
        // land like the first session's did.
        {
            let mut session = app.world_mut().resource_mut::<Session>();
            session
                .begin_generation(
                    mm2_game::SessionConfig {
                        authority: mm2_game::SessionAuthority::Remote,
                        ..mm2_game::SessionConfig::default()
                    },
                    2,
                )
                .unwrap();
        }
        let mut row = snap_impact(3);
        row.seat = 7;
        app.world_mut().resource_mut::<RemoteSnaps>().push(
            2,
            5,
            Vec::new(),
            Vec::new(),
            vec![row],
            None,
        );
        app.update();
        let report = app.world().resource::<NetDriveReport>();
        assert_eq!(report.impacts_applied, 2);
        assert_eq!(report.snaps_applied, 2);
    }

    /// A predicted session's race counts down and releases on the
    /// wire's word (protocol v13, F25-B): the client's own
    /// `advance_race` is authority-gated and never steps, so the
    /// `Snap.race` rows are its only race clock. A fresher countdown
    /// row mirrors the remainder; the `Running` row performs the
    /// release — participants flip `AwaitingStart → Racing`, the
    /// session moves `Countdown → Playing`, one `RaceStarted` goes
    /// out. A regressed reorder never restages, a foreign-generation
    /// row stages past the key but dies at the session gate, an
    /// unnamed phase dies at `push`, and a raceless session's row has
    /// nothing to mirror into.
    #[test]
    fn a_race_row_releases_the_predicted_countdown() {
        let mut session = Session::new();
        session
            .begin_generation(
                mm2_game::SessionConfig {
                    authority: mm2_game::SessionAuthority::Remote,
                    ..mm2_game::SessionConfig::default()
                },
                1,
            )
            .unwrap();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Countdown).unwrap();
        let generation = session.generation();
        let def = grid_def(&[]);
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(session)
            .insert_resource(RaceState::new(def.clone(), generation))
            .init_resource::<RemoteSnaps>()
            .init_resource::<NetDriveReport>()
            .init_resource::<crate::texel_fx::TexelDamageReport>()
            .add_message::<RemoteImpact>()
            .add_message::<RaceStarted>()
            .add_message::<BangerStateChanged>()
            .init_resource::<BangerPool>()
            // The v14 progress tails' terminal edges record into the
            // session ledger — never exercised by every leg, but the
            // system's parameters require it registered.
            .init_resource::<ResultLedger>()
            .add_systems(Update, apply_snapshots);
        // A participant awaiting the countdown — the release flips it.
        let participant = app.world_mut().spawn(RaceProgress::new(&def)).id();

        let push_race = |app: &mut App, generation: u64, row: SnapRace| {
            app.world_mut().resource_mut::<RemoteSnaps>().push(
                generation,
                0,
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Some(row),
            );
        };
        let countdown = |remaining| SnapRace {
            phase: SNAP_PHASE_COUNTDOWN,
            countdown: remaining,
            clock: 0,
        };
        let running = |clock| SnapRace {
            phase: SNAP_PHASE_RUNNING,
            countdown: 0,
            clock,
        };
        let phase = |app: &App| app.world().resource::<RaceState>().phase;
        let clock = |app: &App| app.world().resource::<RaceState>().clock;

        // Countdown rows stage off the pose watermark — every snap
        // carries the same frozen session tick while the authority
        // counts down, so the pose side reads them all stale after
        // the first. The freshest row still mirrors.
        push_race(&mut app, 1, countdown(120));
        push_race(&mut app, 1, countdown(100));
        app.update();
        assert_eq!(
            phase(&app),
            RacePhase::Countdown { remaining: 100 },
            "the freshest countdown row wins"
        );
        assert_eq!(
            app.world().resource::<Session>().phase(),
            &SessionPhase::Countdown
        );
        assert!(app.world().resource::<NetDriveReport>().race_applied == 1);

        // The release: `Running` flips the participant, moves the
        // session and writes the one `RaceStarted`.
        push_race(&mut app, 1, running(0));
        app.update();
        {
            assert_eq!(phase(&app), RacePhase::Running);
            assert_eq!(clock(&app), 0);
            assert_eq!(
                app.world().resource::<Session>().phase(),
                &SessionPhase::Playing,
                "the wire's release is the client's `advance_race`"
            );
            assert_eq!(
                app.world().get::<RaceProgress>(participant).unwrap().state,
                ParticipantState::Racing,
                "the release flips awaiting participants"
            );
            let started: Vec<_> = app
                .world_mut()
                .resource_mut::<Messages<RaceStarted>>()
                .drain()
                .collect();
            assert_eq!(started.len(), 1, "one release event, like the authority's");
            assert_eq!(app.world().resource::<NetDriveReport>().race_applied, 2);
        }

        // The running clock ticks along off the wire.
        push_race(&mut app, 1, running(41));
        app.update();
        assert_eq!(clock(&app), 41);
        assert_eq!(app.world().resource::<NetDriveReport>().race_applied, 3);
        // No second release edge — `Running → Running` is no countdown.
        assert!(
            app.world_mut()
                .resource_mut::<Messages<RaceStarted>>()
                .drain()
                .next()
                .is_none()
        );

        // A reorder's straggler cannot re-hold control: the countdown
        // row ranks behind the applied `Running`, so `push` never even
        // stages it — the count stays silent (idempotent state).
        push_race(&mut app, 1, countdown(60));
        app.update();
        assert_eq!(phase(&app), RacePhase::Running);
        assert_eq!(app.world().resource::<NetDriveReport>().race_dropped, 0);

        // A foreign generation stages past the key — a *different*
        // authority's numbering ranks ahead — but dies at the session
        // gate the queued rows share. (Once staged it also mutes lower
        // gen-1 rows; in production a new authority resets the inbox.)
        push_race(&mut app, 9, running(99));
        app.update();
        assert_eq!(phase(&app), RacePhase::Running);
        assert_eq!(clock(&app), 41);
        assert_eq!(
            app.world().resource::<NetDriveReport>().race_dropped,
            1,
            "the foreign-generation row dropped at the session gate"
        );
        assert_eq!(app.world().resource::<NetDriveReport>().race_applied, 3);

        // A discriminant the wire cannot name never stages.
        push_race(
            &mut app,
            9,
            SnapRace {
                phase: 99,
                countdown: 0,
                clock: 0,
            },
        );
        app.update();
        assert_eq!(
            app.world().resource::<NetDriveReport>().race_dropped,
            2,
            "the unnamed phase died at push"
        );
    }

    /// A `Snap.race` row on a session with no `RaceState` — cruise and
    /// dev worlds — has nothing to mirror into; the row drops counted
    /// rather than fabricating a race.
    #[test]
    fn a_race_row_on_a_raceless_session_drops() {
        let mut session = Session::new();
        session
            .begin_generation(
                mm2_game::SessionConfig {
                    authority: mm2_game::SessionAuthority::Remote,
                    ..mm2_game::SessionConfig::default()
                },
                1,
            )
            .unwrap();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Playing).unwrap();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(session)
            .init_resource::<RemoteSnaps>()
            .init_resource::<NetDriveReport>()
            .init_resource::<crate::texel_fx::TexelDamageReport>()
            .add_message::<RemoteImpact>()
            .add_message::<RaceStarted>()
            .add_message::<BangerStateChanged>()
            .init_resource::<BangerPool>()
            // The v14 progress tails' terminal edges record into the
            // session ledger — never exercised by every leg, but the
            // system's parameters require it registered.
            .init_resource::<ResultLedger>()
            .add_systems(Update, apply_snapshots);

        app.world_mut().resource_mut::<RemoteSnaps>().push(
            1,
            0,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Some(SnapRace {
                phase: SNAP_PHASE_RUNNING,
                countdown: 0,
                clock: 7,
            }),
        );
        app.update();
        let report = app.world().resource::<NetDriveReport>();
        assert_eq!(report.race_applied, 0);
        assert_eq!(report.race_dropped, 1);
    }

    /// The encode/mirror pair: every `RacePhase` maps to the wire
    /// tuple `race_phase` reconstitutes, and the freshness key ranks
    /// the lifecycle monotonically.
    #[test]
    fn race_rows_encode_and_order_the_lifecycle() {
        let def = grid_def(&[]);
        let mut state = RaceState::new(def, 3);
        let countdown = encode_race(&state);
        assert_eq!(
            countdown,
            SnapRace {
                phase: SNAP_PHASE_COUNTDOWN,
                countdown: 1,
                clock: 0
            }
        );
        assert_eq!(
            race_phase(&countdown),
            Some(RacePhase::Countdown { remaining: 1 })
        );
        state.phase = RacePhase::Running;
        state.clock = 41;
        let running = encode_race(&state);
        assert_eq!(
            race_phase(&running),
            Some(RacePhase::Running),
            "the running row keeps the clock"
        );
        assert_eq!(running.clock, 41);
        state.phase = RacePhase::Complete;
        let complete = encode_race(&state);
        assert_eq!(race_phase(&complete), Some(RacePhase::Complete));
        assert_eq!(
            race_phase(&SnapRace {
                phase: 7,
                countdown: 0,
                clock: 0
            }),
            None,
            "an unnamed discriminant decodes nothing"
        );
        // The order key is monotone along the real sequence: the
        // countdown's *remaining* descends while its key climbs, then
        // the release outranks any countdown, then the clock counts up.
        let key = |g: u64, phase: u8, countdown: u32, clock: u64| {
            race_order_key(
                g,
                &SnapRace {
                    phase,
                    countdown,
                    clock,
                },
            )
        };
        assert!(key(3, SNAP_PHASE_COUNTDOWN, 120, 0) < key(3, SNAP_PHASE_COUNTDOWN, 60, 0));
        assert!(key(3, SNAP_PHASE_COUNTDOWN, 1, 0) < key(3, SNAP_PHASE_RUNNING, 0, 0));
        assert!(key(3, SNAP_PHASE_RUNNING, 0, 0) < key(3, SNAP_PHASE_RUNNING, 0, 1));
        assert!(key(3, SNAP_PHASE_RUNNING, 0, 9) < key(3, SNAP_PHASE_COMPLETE, 0, 9));
        assert!(key(3, SNAP_PHASE_COMPLETE, 0, 9) < key(4, SNAP_PHASE_COUNTDOWN, 99, 0));
        assert_eq!(key(3, 99, 0, 0), None);
    }

    /// A wire seat's v14 progress tail is the predicted client's only
    /// `RaceProgress` truth (protocol v14, F25-B): the mirror lands
    /// the authority's rule counters verbatim, and a terminal edge
    /// mints the same `SessionResult` `advance_race` records on the
    /// authority — stamped on the snap's own tick, deduplicated
    /// through the `ResultLedger`. Resolving the *local* seat moves
    /// the session `Playing → Results`, the transition UI-5's own
    /// resolution runs. A redelivered identical row is idempotent
    /// state; a conflicting terminal or an unnamed discriminant
    /// drops counted rather than rewriting a recorded result.
    #[test]
    fn a_progress_tail_mirrors_the_seats_race_standing() {
        let mut session = Session::new();
        session
            .begin_generation(
                mm2_game::SessionConfig {
                    authority: mm2_game::SessionAuthority::Remote,
                    ..mm2_game::SessionConfig::default()
                },
                1,
            )
            .unwrap();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Playing).unwrap();
        let generation = session.generation();
        let def = grid_def(&[]);
        let mut race = RaceState::new(def.clone(), generation);
        race.phase = RacePhase::Running;
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(session)
            .insert_resource(race)
            .init_resource::<RemoteSnaps>()
            .init_resource::<NetDriveReport>()
            .init_resource::<crate::texel_fx::TexelDamageReport>()
            .add_message::<RemoteImpact>()
            .add_message::<RaceStarted>()
            .add_message::<BangerStateChanged>()
            .init_resource::<BangerPool>()
            .init_resource::<ResultLedger>()
            .add_systems(Update, apply_snapshots);
        // A remote wire seat and our own — both participants the
        // race tracks, so both carry `RaceProgress` to mirror into.
        let seat = |wire: u16, id: u16, control| {
            (
                NetPlayer(wire),
                Player {
                    id: mm2_game::PlayerId(id),
                    control,
                },
                ResetEpoch(0),
                Position(Vec3::ZERO),
                Rotation(Quat::IDENTITY),
                LinearVelocity(Vec3::ZERO),
                AngularVelocity(Vec3::ZERO),
                RaceProgress::new(&def),
            )
        };
        let remote = app
            .world_mut()
            .spawn(seat(7, 9, PlayerControl::Remote))
            .id();
        let own = app
            .world_mut()
            .spawn(seat(8, 11, PlayerControl::Local))
            .id();

        let push = |app: &mut App, tick: u64, entries: Vec<SnapEntry>| {
            app.world_mut().resource_mut::<RemoteSnaps>().push(
                1,
                tick,
                entries,
                Vec::new(),
                Vec::new(),
                None,
            );
        };
        let progress_of =
            |app: &App, seat: Entity| app.world().get::<RaceProgress>(seat).unwrap().clone();

        // Mid-race standing: the counters land verbatim.
        let mut racing = snap_entry();
        racing.player = 7;
        racing.prog_state = SNAP_PROG_RACING;
        racing.prog_cleared = 0b1;
        racing.prog_next = 5;
        racing.prog_lap = 2;
        racing.prog_crossings = 9;
        racing.prog_route_clears = 1;
        push(&mut app, 10, vec![racing]);
        app.update();
        let p = progress_of(&app, remote);
        assert_eq!(p.state, ParticipantState::Racing);
        assert!(p.is_cleared(0));
        assert_eq!(p.next, 5);
        assert_eq!(p.lap, 2);
        assert_eq!(p.crossings, 9);
        assert_eq!(p.route_clears, 1);
        assert_eq!(app.world().resource::<NetDriveReport>().progress_applied, 1);
        // The absent own row left the local participant untouched.
        assert_eq!(
            progress_of(&app, own).state,
            ParticipantState::AwaitingStart
        );

        // The terminal edge mints a result like `advance_race` does —
        // the snap's own tick stamps it, the ledger dedups it.
        let mut finished = racing;
        finished.prog_state = SNAP_PROG_FINISHED;
        finished.prog_ticks = 4200;
        push(&mut app, 11, vec![finished]);
        app.update();
        let result = match progress_of(&app, remote).state {
            ParticipantState::Finished { race_ticks, result } => {
                assert_eq!(race_ticks, 4200);
                result
            }
            other => panic!("expected Finished, got {other:?}"),
        };
        {
            let ledger = app.world().resource::<ResultLedger>();
            assert_eq!(ledger.len(), 1);
            let recorded = ledger.get(&result).unwrap();
            assert_eq!(recorded.tick, 11, "the snap's tick stamps the result");
            assert_eq!(
                recorded.outcome,
                SessionOutcome::Finished { race_ticks: 4200 }
            );
        }
        assert_eq!(app.world().resource::<NetDriveReport>().progress_applied, 2);
        // A *remote* seat resolving never ends the local session —
        // only the local participant's own resolution does (UI-5).
        assert_eq!(
            app.world().resource::<Session>().phase(),
            &SessionPhase::Playing
        );

        // A redelivery of the same terminal row is idempotent state,
        // not a second resolution — nothing re-mints, nothing counts.
        push(&mut app, 12, vec![finished]);
        app.update();
        assert_eq!(app.world().resource::<NetDriveReport>().progress_applied, 2);
        assert_eq!(app.world().resource::<NetDriveReport>().progress_dropped, 0);
        assert_eq!(app.world().resource::<ResultLedger>().len(), 1);

        // A terminal row disagreeing with the recorded resolution is
        // a non-conforming authority's word — refused, not believed.
        let mut rewound = finished;
        rewound.prog_state = SNAP_PROG_TIMED_OUT;
        rewound.prog_ticks = 100;
        push(&mut app, 13, vec![rewound]);
        app.update();
        assert_eq!(app.world().resource::<NetDriveReport>().progress_dropped, 1);
        assert!(matches!(
            progress_of(&app, remote).state,
            ParticipantState::Finished {
                race_ticks: 4200,
                ..
            }
        ));
        assert_eq!(app.world().resource::<ResultLedger>().len(), 1);

        // An unnamed discriminant drops counted.
        let mut nonsense = snap_entry();
        nonsense.player = 7;
        nonsense.prog_state = 9;
        push(&mut app, 14, vec![nonsense]);
        app.update();
        assert_eq!(app.world().resource::<NetDriveReport>().progress_dropped, 2);

        // The local seat's resolution on the wire's word ends the
        // local session exactly like a simulated finish does.
        let mut own_row = snap_entry();
        own_row.player = 8;
        own_row.prog_state = SNAP_PROG_FINISHED;
        own_row.prog_ticks = 4300;
        push(&mut app, 15, vec![own_row]);
        app.update();
        assert!(matches!(
            progress_of(&app, own).state,
            ParticipantState::Finished {
                race_ticks: 4300,
                ..
            }
        ));
        assert_eq!(
            app.world().resource::<Session>().phase(),
            &SessionPhase::Results,
            "UI-5's local-resolution rule, mirrored"
        );
        assert_eq!(app.world().resource::<ResultLedger>().len(), 2);
    }
}
