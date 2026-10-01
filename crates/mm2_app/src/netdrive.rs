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
//!   devices. They are stamped `PlayerControl::Remote` — the rule
//!   systems that skip `Remote` (damage outcomes, stuck, recovery) still
//!   do so on the host, since resolving a remote driver's *outcome*
//!   needs wire coordination a later F25 slice adds; physics, collision
//!   and contract telemetry apply to them like any participant.
//! - **Client** (`SessionAuthority::Remote` → `Predicted`): remote cars
//!   are kinematic copies blended between the two newest snapshots
//!   ([`RemoteLerp`]). Our own car keeps driving on local physics —
//!   snapshot entries naming our wire id are received but not applied
//!   (reconciliation is a later slice), which is what `Predicted` means.
//!
//! Identities on the wire are the lobby's roster slots, never Bevy
//! entities: the host seat is wire id 0 (it never appears on the roster;
//! its pick rides `Start`), peers mint from 1. [`NetPlayer`] stamps that
//! identity on each participant entity — including the local car, so the
//! host's snapshots include its own pose and a client knows which entry
//! is itself.
//!
//! Everything here is loopback-scoped groundwork like the rest of F24/F25:
//! no client-side prediction, no lag compensation, no damage/result
//! replication — those are named gaps, not silent behavior.

use std::collections::BTreeMap;
use std::time::Duration;

use avian3d::prelude::{
    AngularVelocity, LinearVelocity, Position, RigidBody, Rotation, TransformInterpolation,
};
use bevy::prelude::*;
use mm2_game::{
    DamageSignals, Mm2Vfs, ObjectIdentity, Player, PlayerControl, PlayerVehicle, RaceProgress,
    RaceState, Session, SessionEntity, SessionPhase,
};
use mm2_net::{DriveInput, Message, RemoteInputs, SnapEntry, VehiclePick};
use mm2_vehicle::{VehicleInput, vehicle_bundle};

use crate::car_visual;
use crate::net::{HostLink, LobbyLink, LobbyState};
use crate::opponents::SPAWN_LIFT;
use crate::session::SpawnPoint;

/// How old the newest mailbox sample may be before a remote driver's
/// input reads as zero — a silent/stalled client should coast, not
/// keep the throttle it last sent. Sized generously for the loopback
/// scope (one missed input window is ~8 ms; this covers ~30).
pub const INPUT_STALE: Duration = Duration::from_millis(250);

/// Lateral spacing between participants' spawn slots — grid assignment
/// is an F25 follow-up; meanwhile remote cars stage beside the local
/// spawn so nobody interpenetrates.
const REMOTE_SPAWN_GAP: f32 = 4.0;

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

/// The wire id this process's own seat carries: 0 on a host, our roster
/// slot on a joined client. `None` when no link exists.
fn self_wire(link: Option<&LobbyLink>, host: Option<&HostLink>) -> Option<u16> {
    if host.is_some() {
        Some(0)
    } else {
        link.map(|l| l.player_id())
    }
}

/// A remote participant's spawn pose: the session's roam spawn shifted
/// laterally by wire id so every peer derives the same layout.
fn remote_spawn_pose(spawn: &SpawnPoint, wire: u16) -> (Vec3, f32) {
    let right = Vec3::new(spawn.yaw.cos(), 0.0, -spawn.yaw.sin());
    let pos = spawn.position + right * (REMOTE_SPAWN_GAP * (wire as f32 + 1.0));
    (pos, spawn.yaw)
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
    // snapshots carry it as seat 0; a client's copy of it is the `Local`
    // entry snapshot application skips.
    for entity in &local {
        commands.entity(entity).insert(NetPlayer(self_wire));
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
/// host it is a dynamic `Vehicle` the input mailbox drives; on a client
/// it is a kinematic copy a `RemoteLerp` blend drives. A pick that fails
/// to load is warned and skipped — the validator already gates roster
/// picks, so this is a defensive path (a dev-car pick cannot fail).
/// Returns whether the entity was spawned.
#[allow(clippy::too_many_arguments)]
fn spawn_remote(
    commands: &mut Commands,
    vfs: &mm2_assets::Vfs,
    session: &mut Session,
    race: Option<&RaceState>,
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
    let (mut pos, yaw) = remote_spawn_pose(spawn, wire);
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
        // snapshots into its pose.
        commands.entity(vehicle).insert((
            RigidBody::Kinematic,
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
                // The texel rig reads the authored damage record — the
                // remote car carries no VehicleDamage this slice, so it
                // gets no texel rig either.
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

/// Host-side: every participant's authoritative pose, broadcast once per
/// update while the session is live. `tick` is the host's session tick —
/// physics only moves inside fixed steps, so a same-tick snapshot is a
/// duplicate clients discard. Positions are `Position`/`Rotation` (the
/// solver's truth), not the render `Transform`.
pub fn publish_snapshots(
    host: Res<HostLink>,
    session: Res<Session>,
    players: Query<
        (
            &NetPlayer,
            &Position,
            &Rotation,
            &LinearVelocity,
            &AngularVelocity,
        ),
        With<Player>,
    >,
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
        .map(|(wire, pos, rot, vel, ang)| SnapEntry {
            player: wire.0,
            pos: pos.0.to_array(),
            rot: rot.0.to_array(),
            vel: vel.0.to_array(),
            angvel: ang.0.to_array(),
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
/// signature for `clippy::type_complexity`.
type SnapTargetRow<'a> = (
    &'a NetPlayer,
    &'a Player,
    &'a mut Position,
    &'a mut Rotation,
    &'a mut LinearVelocity,
    &'a mut AngularVelocity,
    Option<&'a mut RemoteLerp>,
);

/// Client-side: fold the newest staged snapshot into the remote copies'
/// [`RemoteLerp`] blend and velocities. Wrong-generation and stale-tick
/// frames drop untouched; entries without a spawned copy (a roster slot
/// whose car hasn't arrived, or our own seat) are skipped — own-seat
/// reconciliation is a later slice.
pub fn apply_snapshots(
    mut snaps: ResMut<RemoteSnaps>,
    session: Res<Session>,
    time: Res<Time>,
    mut remotes: Query<SnapTargetRow<'_>, With<RemotePick>>,
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
        for (wire, player, mut pos, mut rot, mut vel, mut ang, lerp) in &mut remotes {
            if wire.0 != entry.player {
                continue;
            }
            // A local seat never takes a snapshot pose — predicted, not
            // reconciled.
            if player.control == PlayerControl::Local {
                break;
            }
            let to_pos = Vec3::from(entry.pos);
            let to_rot = wire_quat(entry.rot);
            *vel = LinearVelocity(Vec3::from(entry.vel));
            *ang = AngularVelocity(Vec3::from(entry.angvel));
            match lerp {
                Some(mut lerp) => {
                    lerp.from_pos = pos.0;
                    lerp.from_rot = rot.0;
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
            break;
        }
    }
    report.snaps_applied += 1;
}

/// Advance remote copies along their [`RemoteLerp`] blend — one blend
/// interval behind the wire, so motion is smooth rather than
/// snap-to-pose. Kinematic bodies take their pose from `Position`.
pub fn drive_remote_lerp(
    time: Res<Time>,
    mut remotes: Query<(&mut Position, &mut Rotation, &RemoteLerp), With<RemotePick>>,
) {
    let now = time.elapsed_secs_f64();
    for (mut pos, mut rot, lerp) in &mut remotes {
        let span = (lerp.end - lerp.start).max(f64::EPSILON);
        let t = ((now - lerp.start) / span).clamp(0.0, 1.0) as f32;
        pos.0 = lerp.from_pos.lerp(lerp.to_pos, t);
        rot.0 = wire_quat(lerp.from_rot.slerp(lerp.to_rot, t).to_array());
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
}
