//! F05-B.5 — water submersion and out-of-bounds recovery.
//!
//! The contract lives in `mm2_game::recovery` ([`VehicleRecovery`],
//! [`RecoveryEvent`]); this module is where it meets the wheel state
//! and the session lifecycle, the same split [`crate::stuck`] uses:
//!
//! - [`track_recovery`] classifies each participant's grounded wheels
//!   once per fixed step — a dry contact re-anchors the detector when
//!   static dry ground holds the car up ([`SupportProbe`]), an
//!   all-water contact accrues the submersion dwell, and an
//!   airborne fall past `fall_margin` below the anchor *and* under the
//!   city's [`WorldFloor`] fires the out-of-bounds leg — emitting one
//!   bounded [`RecoveryEvent`] per episode. (The detector's non-finite arm is defence-in-depth for
//!   poses written *between* steps: a pose gone non-finite inside the
//!   physics step trips the wheel raycast first.)
//! - [`resolve_recovery`] answers a detection with [`ResetVehicle`]
//!   to the recorded anchor — the last pose dry ground proved
//!   recoverable — set down level on the ground under it, falling back
//!   to the session spawn when the car never touched dry ground or no
//!   ground lies under the landing. The reset marks the car `Teleported`,
//!   so it can never sweep a checkpoint (F05 req 5). Local
//!   participants' trailers re-seat at their authored offsets, the
//!   same rule `resolve_stuck` follows.
//!
//! Recovery is **not a repair**: a damaged car recovered from the
//! water keeps its damage and detached parts — only
//! [`crate::damage::resolve_disabled`]'s outcomes heal. Policy: the
//! local participant and AI opponents recover identically (designed —
//! the original's water/OOB rules are unverified, UNK-13/DSN-23); a
//! remote participant's authority owns its detector (F05 req 6); a
//! `Disabled` wreck belongs to the damage outcome, so the observe leg
//! skips it. Both systems idle while the session is not `Playing`, so
//! a pause can neither accrue dwell nor flush a buffered event into
//! the next session.

use std::collections::HashMap;

use avian3d::prelude::*;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use mm2_game::{
    DamageTier, GroundContact, ObjectId, ObjectIdentity, Player, PlayerControl, RecoveryCause,
    RecoveryEvent, RecoveryVerdict, Session, VehicleDamage, VehicleRecovery, VehicleStuck,
};
use mm2_vehicle::{
    HandlingMetrics, ResetPending, ResetVehicle, TireSurface, Vehicle, VehicleConfig, VehicleState,
    hull_points, seat_level,
};

use crate::city::WorldFloor;
use crate::police::{PoliceCar, recovery_driver};
use crate::session::SpawnPoint;

/// Per-session evidence counters for the recovery pipeline — the
/// `rcv=` field of the headless smoke record. Session-scoped like
/// [`crate::stuck::StuckReport`]: `drive_session` resets it during
/// teardown so counts describe the active session's stream.
#[derive(Resource, Debug, Default)]
pub struct RecoveryReport {
    /// Submersion episodes that fired (`RecoveryCause::Submerged`).
    pub submerged: u64,
    /// Out-of-bounds detections that fired
    /// (`RecoveryCause::OutOfBounds`).
    pub out_of_bounds: u64,
    /// Events that resolved to a recovery reset — a stale or
    /// remote-owned event does not count.
    pub recovered: u64,
}

impl RecoveryReport {
    /// Forget all session-scoped counts — called on teardown, same
    /// contract as [`crate::damage::DamageReport::reset`].
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Classify the surface under the car this step from the wheels'
/// recorded `surface_drag`: any grounded wheel under `water_min_drag`
/// is dry purchase; every grounded wheel at or above it is water; no
/// grounded wheels is air. Unmarked colliders report `0.0` — ordinary
/// ground by definition.
///
/// The authored `.water` record plus SDL water-surface marks
/// (F18-A.6/.7, [`crate::water::CityWater`]) overlay both arms: a
/// wheel contact inside a listed deadly room at/below its bound is
/// water whatever the collider's `drag` says — the room is marked
/// deadly, not the material — and a car under a listed room's bound
/// with no contact at all (clipped through the plane) drowns rather
/// than free-falls.
fn ground_contact(
    state: &VehicleState,
    pos: Vec3,
    water_min_drag: f32,
    water: Option<&crate::water::CityWater>,
) -> GroundContact {
    let mut grounded = false;
    let mut dry = false;
    for w in &state.wheels {
        if w.grounded {
            grounded = true;
            let deadly = water.is_some_and(|water| water.is_deadly(w.contact_point));
            dry |= w.surface_drag < water_min_drag && !deadly;
        }
    }
    if !grounded {
        if water.is_some_and(|water| water.is_deadly(pos)) {
            GroundContact::Submerged
        } else {
            GroundContact::Airborne
        }
    } else if dry {
        GroundContact::Dry
    } else {
        GroundContact::Submerged
    }
}

/// Least `up.y` — the cosine of the car's tilt — a pose may have and
/// still anchor the recovery: 30° off level. Streets stay well inside it
/// (SF's steepest are ~17°); a car pitched further is up a grass bank
/// or a ramp face, where the level landing a recovery makes would need
/// ground probed metres above the anchor.
const ANCHOR_MIN_UPRIGHT: f32 = 0.866;

/// How far below the car's centre of mass the ground that holds it up
/// may lie, metres — a settled car's centre of mass is under two metres
/// above its wheels' contacts.
const ANCHOR_SUPPORT_REACH: f32 = 3.0;

/// Head start the landing probes get above the highest ground an
/// anchor can have under its level footprint, metres.
const LANDING_PROBE_MARGIN: f32 = 0.5;

/// The anchor test on top of a dry wheel contact: whether static dry
/// ground holds the car up.
///
/// A dry wheel says something solid is under that wheel, not that the
/// car stands on it. A car righted level into a 37° bank had one rear
/// wheel still above the grass while the rest of it lay inside the
/// one-sided slope and was falling through: that pose became the anchor,
/// and every out-of-bounds recovery set the car back down inside the
/// bank to fall again. The probe asks the chassis instead — straight down
/// from the centre of mass the first thing hit must be dry ground that
/// cannot move away (no ferry deck or drawbridge leaf), and the car
/// must be close enough to upright to be set down level where it is.
#[derive(SystemParam)]
pub struct SupportProbe<'w, 's> {
    spatial: SpatialQuery<'w, 's>,
    colliders: Query<'w, 's, (Option<&'static ColliderOf>, Option<&'static TireSurface>)>,
    bodies: Query<'w, 's, &'static RigidBody>,
}

impl SupportProbe<'_, '_> {
    /// Whether `collider` belongs to no moving body — city ground and
    /// props, not a car, a loose prop or a ferry deck.
    fn is_static(&self, collider: Entity) -> bool {
        let attached = self.colliders.get(collider).ok().and_then(|(c, _)| c);
        let body = attached.map_or(collider, |c| c.body);
        !self
            .bodies
            .get(body)
            .is_ok_and(|b| b.is_dynamic() || b.is_kinematic())
    }

    /// Whether `car`, at `pos`/`rot` with `vehicle`'s centre of mass,
    /// stands on static dry ground: upright within
    /// [`ANCHOR_MIN_UPRIGHT`], and the first collider straight below its
    /// centre of mass within [`ANCHOR_SUPPORT_REACH`] belongs to no
    /// moving body and is not water by the same rule the wheels use.
    fn holds_up(
        &self,
        car: Entity,
        pos: Vec3,
        rot: Quat,
        vehicle: &Vehicle,
        water_min_drag: f32,
        water: Option<&crate::water::CityWater>,
    ) -> bool {
        if (rot * Vec3::Y).y < ANCHOR_MIN_UPRIGHT {
            return false;
        }
        let com = pos + rot * Vec3::from(vehicle.config.center_of_mass);
        let filter = SpatialQueryFilter::default().with_excluded_entities([car]);
        // `solid: false` — a centre of mass already inside the ground
        // must find nothing below it, not a hit at its own origin.
        let Some(hit) =
            self.spatial
                .cast_ray(com, Dir3::NEG_Y, ANCHOR_SUPPORT_REACH, false, &filter)
        else {
            return false;
        };
        let surface = self.colliders.get(hit.entity).ok().and_then(|(_, s)| s);
        let point = com - Vec3::Y * hit.distance;
        let deadly = water.is_some_and(|water| water.is_deadly(point));
        self.is_static(hit.entity) && surface.map_or(0.0, |s| s.drag) < water_min_drag && !deadly
    }

    /// [`seat_on_static_ground`] with this probe's notion of static.
    /// Only ground that anchored the car counts: another car or a raised
    /// drawbridge leaf beside the anchor must not lift the landing onto
    /// its roof or deck.
    fn seat_landing(
        &self,
        car: Entity,
        config: &VehicleConfig,
        position: Vec3,
        yaw: f32,
    ) -> Option<Vec3> {
        seat_on_static_ground(
            &self.spatial,
            &|collider| self.is_static(collider),
            car,
            config,
            position,
            yaw,
        )
    }
}

/// `car` set down level at `position` facing `yaw` on the static ground
/// under its footprint ([`seat_level`]) — `None` when none lies under
/// it. `is_static` says which colliders belong to no moving body.
///
/// The probes are banded around `position.y`: a pose is a place the
/// car was (or is to be) at about this height, not a promise of ground
/// below it. An anchor's car stood within 30° of level
/// ([`ANCHOR_MIN_UPRIGHT`]), so the ground under its level footprint
/// lies at most the footprint's reach × tan 30° above the ground under
/// the anchor: the probes start that far up, plus
/// [`LANDING_PROBE_MARGIN`] — and never above whatever is overhead,
/// since a probe that starts over a bridge deck or a tunnel roof would
/// land the car on top of it.
pub(crate) fn seat_on_static_ground(
    spatial: &SpatialQuery,
    is_static: &dyn Fn(Entity) -> bool,
    car: Entity,
    config: &VehicleConfig,
    position: Vec3,
    yaw: f32,
) -> Option<Vec3> {
    let reach = hull_points(config)
        .into_iter()
        .chain(config.wheels.iter().map(|w| w.position))
        .map(|p| Vec2::new(p[0], p[2]).length())
        .fold(0.0, f32::max);
    let ground = position.y + HandlingMetrics::of(config).ground_y;
    let mut top = ground + reach * ANCHOR_MIN_UPRIGHT.acos().tan() + LANDING_PROBE_MARGIN;
    let com = position + Quat::from_rotation_y(yaw) * Vec3::from(config.center_of_mass);
    let filter = SpatialQueryFilter::default().with_excluded_entities([car]);
    if top > com.y
        && let Some(hit) = spatial.cast_ray(com, Dir3::Y, top - com.y, false, &filter)
    {
        top = com.y + hit.distance - 0.05;
    }
    seat_level(
        spatial,
        car,
        config,
        position,
        yaw,
        top,
        ground - ANCHOR_SUPPORT_REACH,
        is_static,
    )
}

type RecoveryVehicles<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static ObjectIdentity,
        Option<&'static Player>,
        &'static Position,
        &'static Rotation,
        &'static VehicleState,
        &'static Vehicle,
        &'static mut VehicleRecovery,
        Option<&'static VehicleDamage>,
        Has<ResetPending>,
    ),
>;

/// Fixed-step: advance every participant's [`VehicleRecovery`]
/// detector once, emitting one [`RecoveryEvent`] per fired episode.
///
/// A dry contact only re-anchors the detector when the
/// [`SupportProbe`] finds static dry ground holding the car up; a dry
/// wheel on anything else observes as airborne — the anchor stays on
/// the last ground that did, and the fall leg keeps watching.
///
/// Remote drivers observe like AI: the authority simulating their car
/// owns their detector (F25-A.4; a predicted client's copies never
/// reach this system — it early-returns without
/// `AuthorityRole::Authority`, F05 req 6). A `Disabled` wreck belongs
/// to [`crate::damage::resolve_disabled`]'s outcome — its reset or
/// restart owns the pose, so the observe leg does not run a second
/// recovery underneath it.
#[allow(clippy::too_many_arguments)] // Bevy system: the observe leg threads the session handles it reads
pub fn track_recovery(
    session: Res<Session>,
    time: Res<Time<Fixed>>,
    water: Option<Res<crate::water::CityWater>>,
    floor: Option<Res<WorldFloor>>,
    support: SupportProbe,
    mut vehicles: RecoveryVehicles,
    mut writer: MessageWriter<RecoveryEvent>,
    mut report: ResMut<RecoveryReport>,
) {
    // Nothing buffers here — observe-only — so pausing simply freezes
    // the dwell instead of draining a queue.
    if !session.is_playing() || !session.authority_role().is_authority() {
        return;
    }
    let dt = time.delta_secs();
    let generation = session.generation();
    let tick = session.tick();
    let water = water.as_deref();
    let floor = floor.map(|floor| floor.0);
    for (entity, id, _player, pos, rot, state, vehicle, mut recovery, damage, reset_pending) in
        &mut vehicles
    {
        // A reset is on its way (fixed steps outrun the frame that
        // applies it): the pose here is the one it replaces, and
        // observing it would re-fire the episode just resolved or
        // overwrite the landing the resolver just recorded.
        if reset_pending {
            continue;
        }
        // A wreck belongs to the damage outcome — it cannot drive out
        // of anything.
        if damage.is_some_and(|d| d.condition() == DamageTier::Disabled) {
            continue;
        }
        let water_min_drag = recovery.policy.water_min_drag;
        let mut contact = ground_contact(state, pos.0, water_min_drag, water);
        if contact == GroundContact::Dry
            && !support.holds_up(entity, pos.0, rot.0, vehicle, water_min_drag, water)
        {
            contact = GroundContact::Airborne;
        }
        let (_, yaw, _) = rot.0.to_euler(EulerRot::YXZ);
        if let RecoveryVerdict::Recover { cause, landing } =
            recovery.observe_in_world(pos.0, yaw, contact, dt, floor)
        {
            match cause {
                RecoveryCause::Submerged => report.submerged += 1,
                RecoveryCause::OutOfBounds => report.out_of_bounds += 1,
            }
            writer.write(RecoveryEvent {
                object: id.0,
                generation,
                tick,
                cause,
                landing,
            });
        }
    }
}

/// Fixed-step: answer each [`RecoveryEvent`] with the bounded
/// recovery — [`ResetVehicle`] to the detector's anchor, the last pose
/// a dry grounded wheel proved recoverable, set down level on the
/// ground under it (`SupportProbe::seat_landing`): every reset lands a car level,
/// and level at the height it stood on a slope its uphill end would be
/// inside the slope. A detector that never touched dry ground
/// (`landing: None`) falls back to the session [`SpawnPoint`], and so
/// does a landing with no ground under it — set down there the car
/// would only fall again, recovering onto the same spot for ever. A
/// session with no spawn at all cannot resolve and the event is spent —
/// a count in `recovered` always means a real reset. Local, AI and
/// remote participants resolve identically — on
/// the authority a remote car's recovery is this process's to declare,
/// and the reset reaches its copies through the snapshot stream
/// (F25-A.4).
///
/// The recovery supersedes an armed [`VehicleStuck`] episode — the
/// reset moves the car far past `move_thresh`, and disarming here
/// keeps a stale episode from firing into the recovered pose (the
/// `resolve_disabled` contract). Towed trailers re-seat behind the
/// recovered tractor through [`crate::session::reseat_towed_trailers`],
/// the stream follower — for a remote rig exactly as for the local
/// one.
#[allow(clippy::too_many_arguments)] // Bevy system: the resolver threads the session handles it acts on
pub fn resolve_recovery(
    mut commands: Commands,
    mut reader: MessageReader<RecoveryEvent>,
    session: Res<Session>,
    spawn: Option<Res<SpawnPoint>>,
    support: SupportProbe,
    identities: Query<(Entity, &ObjectIdentity, Option<&Player>, Has<PoliceCar>)>,
    mut vehicles: Query<(
        &mut VehicleRecovery,
        Option<&mut VehicleStuck>,
        Option<&Vehicle>,
    )>,
    cops: Query<(), With<PoliceCar>>,
    mut resets: MessageWriter<ResetVehicle>,
    mut report: ResMut<RecoveryReport>,
) {
    // Recoveries are rare; skip the index walk when there is none (see
    // `damage::resolve_disabled`).
    if reader.is_empty() {
        return;
    }
    // Drain regardless of phase/authority — events buffered while
    // Loading/Paused or produced under a remote authority would
    // otherwise flush as a stale burst (same contract as
    // `resolve_stuck`).
    if !session.is_playing() || !session.authority_role().is_authority() {
        reader.read().for_each(drop);
        return;
    }
    let generation = session.generation();
    let index: HashMap<ObjectId, (Entity, Option<PlayerControl>)> = identities
        .iter()
        .map(|(entity, id, player, cop)| (id.0, (entity, recovery_driver(player, cop))))
        .collect();

    for event in reader.read() {
        if event.generation != generation {
            continue;
        }
        let Some(&(entity, control)) = index.get(&event.object) else {
            continue;
        };
        // Every identified participant resolves — a remote driver is
        // this authority's simulated car (F25-A.4), a cop an AI-driven
        // one (`recovery_driver`); an unidentified object has no driver
        // to recover for.
        if control.is_none() {
            continue;
        }
        let anchor = event
            .landing
            .or_else(|| vehicles.get(entity).ok().and_then(|(d, ..)| d.anchor()));
        let seated = anchor.and_then(|(position, yaw)| {
            match vehicles.get(entity).ok().and_then(|(.., vehicle)| vehicle) {
                Some(vehicle) => support
                    .seat_landing(entity, &vehicle.config, position, yaw)
                    .map(|seated| (seated, yaw)),
                None => Some((position, yaw)),
            }
        });
        if anchor.is_some() && seated.is_none() {
            debug!(tick = event.tick, "no ground under the recovery landing");
        }
        // No ground under the landing: the spawn is the landing of last
        // resort, as for a car that never stood anywhere — and only
        // without one does the bare anchor stand.
        // A cop never falls back to the session spawn — that is the
        // player's start, and a cop set down there would be teleported
        // onto its target; it keeps the bare anchor (its own post).
        let spawn_pose = if cops.contains(entity) {
            None
        } else {
            spawn.as_ref().map(|s| (s.position, s.yaw))
        };
        let Some((position, yaw)) = seated.or(spawn_pose).or(anchor) else {
            continue;
        };
        // The local driver's reset is the one a player notices — say
        // why it happened; opponents' stay at debug.
        if control == Some(PlayerControl::Local) {
            info!(cause = ?event.cause, tick = event.tick, to = ?position, "player vehicle recovered");
        } else {
            debug!(cause = ?event.cause, tick = event.tick, to = ?position, "vehicle recovered");
        }
        resets.write(ResetVehicle {
            entity: Some(entity),
            position,
            yaw,
        });
        if let Ok((mut detector, stuck, vehicle)) = vehicles.get_mut(entity) {
            // Until `vehicle_reset` lands the teleport the car is not
            // observed again (`track_recovery`, `track_stuck`).
            if vehicle.is_some() {
                commands.entity(entity).insert(ResetPending);
            }
            // Re-anchor on the landing and clear the episode — a
            // landing back on water starts a fresh dwell, not a
            // carried-over one.
            detector.recovered(position, yaw);
            // The recovery owns the pose now: a stale stuck episode
            // must not fire a second reset into it.
            if let Some(mut detector) = stuck {
                detector.disarm();
            }
        }
        report.recovered += 1;
    }
}
