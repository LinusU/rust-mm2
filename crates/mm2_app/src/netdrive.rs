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
//!   What the wire still does not carry is the replicated damage
//!   presentation (smoke/sparks/texel/breakaway stay unrigged on remote
//!   cars — F25-B/F26 scope).
//! - **Client** (`SessionAuthority::Remote` → `Predicted`): remote cars
//!   are kinematic copies blended between the two newest snapshots
//!   ([`RemoteLerp`]), marked [`RemoteReplica`] so the local sim never
//!   steps them — their `VehicleState`/`VehicleInput` carry the snapshot
//!   tail's replicated drive state for the wheel/glow visuals instead.
//!   They carry the same damage/stuck/recovery
//!   components inertly, since the rule systems never run under a
//!   predicted session. Our own car keeps driving on local physics —
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
//! no client-side prediction, no lag compensation, no damage/result
//! replication down to clients' presentation — those are named gaps, not
//! silent behavior.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use avian3d::prelude::{
    AngularVelocity, LinearVelocity, Position, RigidBody, Rotation, TransformInterpolation,
};
use bevy::prelude::*;
use mm2_game::{
    DamageSignals, DamageSpec, Mm2Vfs, ObjectIdentity, Player, PlayerControl, PlayerVehicle,
    RaceDefinition, RaceProgress, RaceState, RecoveryPolicy, Session, SessionEntity, SessionPhase,
    StuckSpec, VehicleDamage, VehicleRecovery, VehicleStuck,
};
use mm2_net::{
    DriveInput, Message, RemoteInputs, SNAP_FLAG_BRAKE, SNAP_FLAG_GROUNDED, SNAP_FLAG_REVERSE,
    SnapEntry, VehiclePick,
};
use mm2_vehicle::{
    DriveDirection, RemoteReplica, ResetVehicle, Teleported, Vehicle, VehicleConfig, VehicleInput,
    VehicleState, vehicle_bundle,
};

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

/// The wire roster id this participant entity carries. `0` is the host
/// seat — the roster never lists it, but its `Start`-carried pick and
/// snapshot entries use it. Stamped on the local car too: the host's
/// snapshots include seat 0, and a client recognizes its own entry.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetPlayer(pub u16);

/// The roster pick a spawned remote participant was built from — a
/// mid-session `SetVehicle` rebroadcast that changes it respawns the
/// entity rather than leaving a stale shell.
#[derive(Component, Debug, Clone, PartialEq)]
pub struct RemotePick(pub VehiclePick);

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

/// The client-side snapshot inbox: the newest `Snap` the lobby pump
/// drained, staged for [`apply_snapshots`]. Latest-wins like the host's
/// input mailbox — a backlog of poses is strictly worse than the newest.
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
}

/// A staged snapshot frame.
struct Snap {
    generation: u64,
    tick: u64,
    entries: Vec<SnapEntry>,
}

impl RemoteSnaps {
    /// Queue a received snapshot frame.
    pub fn push(&mut self, generation: u64, tick: u64, entries: Vec<SnapEntry>) {
        self.latest = Some(Snap {
            generation,
            tick,
            entries,
        });
    }

    /// Newest snapshot tick applied so far — for the record/tests.
    pub fn applied(&self) -> Option<(u64, u64)> {
        self.applied
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
    local: Query<Entity, (With<PlayerVehicle>, Without<NetPlayer>)>,
    mut report: ResMut<NetDriveReport>,
) {
    let Some(self_wire) = self_wire(link.as_deref(), host.as_deref()) else {
        return;
    };
    // Only reconcile inside the session the lobby minted — parked at
    // `Menu` nothing exists, and mid-teardown nothing should spawn.
    let live = lobby.generation == Some(session.generation())
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
    // components are inert — the systems that read them never run
    // without `AuthorityRole::Authority` — but carrying them keeps one
    // spawn shape and gives damage-state replication a place to land.
    if let Some(d) = def.as_ref().and_then(|d| d.damage.as_ref()) {
        commands
            .entity(vehicle)
            .insert(VehicleDamage::new(DamageSpec::from(d)));
    }
    if let Some(s) = def.as_ref().and_then(|d| d.stuck.as_ref()) {
        commands
            .entity(vehicle)
            .insert(VehicleStuck::new(StuckSpec::from(s)));
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
    if let Some(race) = race {
        commands
            .entity(vehicle)
            .insert(RaceProgress::new(&race.definition));
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
                // The texel rig reads the authored damage record — a
                // remote car carries `VehicleDamage` for the authority's
                // pipeline, but its *skin* is replicated presentation
                // (F25-B/F26), so no rig is bound.
                None,
            );
            if !missing.is_empty() {
                warn!(car = %def.id, "remote vehicle missing textures: {}", missing.join(", "));
            }
        }
        None => car_visual::spawn_dev_car(commands, &cfg, meshes, materials, vehicle),
    }
    info!(player = wire, "remote participant spawned");
    true
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
        .send_input(encode_input(input, session.generation(), seq.0))
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
            s.input.generation == session.generation() && s.received.elapsed() <= INPUT_STALE
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
    if link.ctl().request_reset(session.generation()).is_ok() {
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
    let generation = session.generation();
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
/// tail encodes. The drive row is `Option`: a participant without a
/// vehicle bundle still publishes its pose rather than vanishing.
type SnapSourceRow<'a> = (
    &'a NetPlayer,
    &'a ResetEpoch,
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
/// grounded flags — what a remote copy needs to *look* like the car the
/// authority is simulating.
pub fn publish_snapshots(
    host: Res<HostLink>,
    session: Res<Session>,
    players: Query<SnapSourceRow<'_>, With<Player>>,
    mut report: ResMut<NetDriveReport>,
) {
    if !matches!(
        session.phase(),
        SessionPhase::Ready | SessionPhase::Countdown | SessionPhase::Playing
    ) {
        return;
    }
    let mut entries: Vec<SnapEntry> = players
        .iter()
        .map(|(wire, epoch, pos, rot, vel, ang, vehicle, state, input)| {
            let (steer, spin, compression, flags) = match (vehicle, state, input) {
                (Some(v), Some(s), Some(i)) => encode_present(&v.config, s, i),
                _ => (0, 0, 0, 0),
            };
            SnapEntry {
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
            }
        })
        .collect();
    entries.sort_by_key(|e| e.player);
    if host
        .ctl()
        .broadcast(&Message::Snap {
            generation: session.generation(),
            tick: session.tick(),
            entries,
        })
        .is_ok()
    {
        report.snaps_sent += 1;
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
);

/// Client-side: fold the newest staged snapshot into the remote copies'
/// [`RemoteLerp`] blend and velocities, and reconcile the own seat on
/// an epoch advance (F25-A.5). Wrong-generation and stale-tick frames
/// drop untouched; entries without a spawned entity are skipped.
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
pub fn apply_snapshots(
    mut commands: Commands,
    mut snaps: ResMut<RemoteSnaps>,
    session: Res<Session>,
    time: Res<Time>,
    mut players: Query<SnapTargetRow<'_>, With<NetPlayer>>,
    mut report: ResMut<NetDriveReport>,
) {
    let Some(snap) = snaps.latest.take() else {
        return;
    };
    // A frame from another session is never applied — and a stale tick
    // inside this generation isn't either (physics only moves on fixed
    // steps, so a same-tick snap carries a duplicate pose).
    if snap.generation != session.generation() {
        return;
    }
    let stale = snaps
        .applied
        .is_some_and(|(g, t)| snap.generation == g && snap.tick <= t);
    if stale {
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
        ) in &mut players
        {
            if wire.0 != entry.player {
                continue;
            }
            let to_pos = Vec3::from(entry.pos);
            let to_rot = wire_quat(entry.rot);
            let authority_reset = entry.epoch != epoch.0;
            epoch.0 = entry.epoch;
            // The own seat: only an authority reset may move it — the
            // host teleported our car (its copy of us is the truth),
            // so the predicted pose yields to the asserted one. Its
            // presentation fields stay ignored too — the local sim's
            // `VehicleState` is already the truth here.
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
    report.snaps_applied += 1;
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
}
