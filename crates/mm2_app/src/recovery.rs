//! F05-B.5 — water submersion and out-of-bounds recovery.
//!
//! The contract lives in `mm2_game::recovery` ([`VehicleRecovery`],
//! [`RecoveryEvent`]); this module is where it meets the wheel state
//! and the session lifecycle, the same split [`crate::stuck`] uses:
//!
//! - [`track_recovery`] classifies each participant's grounded wheels
//!   once per fixed step — any dry contact re-anchors the detector,
//!   an all-water contact accrues the submersion dwell, and an
//!   airborne fall past `fall_margin` below the anchor fires the
//!   out-of-bounds leg — emitting one bounded [`RecoveryEvent`] per
//!   episode. (The detector's non-finite arm is defence-in-depth for
//!   poses written *between* steps: a pose gone non-finite inside the
//!   physics step trips the wheel raycast first.)
//! - [`resolve_recovery`] answers a detection with [`ResetVehicle`]
//!   to the recorded anchor — the last pose dry ground proved
//!   recoverable — falling back to the session spawn when the car
//!   never touched dry ground. The reset marks the car `Teleported`,
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
use bevy::prelude::*;
use mm2_game::{
    DamageTier, GroundContact, ObjectId, ObjectIdentity, Player, PlayerControl, RecoveryCause,
    RecoveryEvent, RecoveryVerdict, Session, VehicleDamage, VehicleRecovery, VehicleStuck,
};
use mm2_vehicle::{ResetVehicle, VehicleState};

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
        &'static mut VehicleRecovery,
        Option<&'static VehicleDamage>,
    ),
>;

/// Fixed-step: advance every participant's [`VehicleRecovery`]
/// detector once, emitting one [`RecoveryEvent`] per fired episode.
///
/// Remote participants are skipped: a predicted client's recovery is
/// its authority's to declare (F05 req 6). A `Disabled` wreck belongs
/// to [`crate::damage::resolve_disabled`]'s outcome — its reset or
/// restart owns the pose, so the observe leg does not run a second
/// recovery underneath it.
pub fn track_recovery(
    session: Res<Session>,
    time: Res<Time<Fixed>>,
    water: Option<Res<crate::water::CityWater>>,
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
    for (.., id, player, pos, rot, state, mut recovery, damage) in &mut vehicles {
        if player.is_some_and(|p| p.control == PlayerControl::Remote) {
            continue;
        }
        // A wreck belongs to the damage outcome — it cannot drive out
        // of anything.
        if damage.is_some_and(|d| d.condition() == DamageTier::Disabled) {
            continue;
        }
        let contact = ground_contact(state, pos.0, recovery.policy.water_min_drag, water);
        let (_, yaw, _) = rot.0.to_euler(EulerRot::YXZ);
        if let RecoveryVerdict::Recover { cause, landing } =
            recovery.observe(pos.0, yaw, contact, dt)
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
/// a dry grounded wheel proved recoverable. A detector that never
/// touched dry ground (`landing: None`) falls back to the session
/// [`SpawnPoint`]; a session with no spawn at all cannot resolve and
/// the event is spent — a count in `recovered` always means a real
/// reset. Local and AI participants resolve identically; remote
/// participants belong to their own authority (F25+).
///
/// The recovery supersedes an armed [`VehicleStuck`] episode — the
/// reset moves the car far past `move_thresh`, and disarming here
/// keeps a stale episode from firing into the recovered pose (the
/// `resolve_disabled` contract). The local participant's trailers
/// re-seat behind the recovered tractor at their authored offsets.
pub fn resolve_recovery(
    mut reader: MessageReader<RecoveryEvent>,
    session: Res<Session>,
    spawn: Option<Res<SpawnPoint>>,
    identities: Query<(Entity, &ObjectIdentity, Option<&Player>)>,
    mut vehicles: Query<(&mut VehicleRecovery, Option<&mut VehicleStuck>)>,
    mut resets: MessageWriter<ResetVehicle>,
    mut report: ResMut<RecoveryReport>,
) {
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
        .map(|(entity, id, player)| (id.0, (entity, player.map(|p| p.control))))
        .collect();

    for event in reader.read() {
        if event.generation != generation {
            continue;
        }
        let Some(&(entity, control)) = index.get(&event.object) else {
            continue;
        };
        if !matches!(
            control,
            Some(PlayerControl::Local) | Some(PlayerControl::Ai)
        ) {
            continue;
        }
        let landing = event
            .landing
            .or_else(|| vehicles.get(entity).ok().and_then(|(d, _)| d.anchor()))
            .or_else(|| spawn.as_ref().map(|s| (s.position, s.yaw)));
        let Some((position, yaw)) = landing else {
            continue;
        };
        resets.write(ResetVehicle {
            entity: Some(entity),
            position,
            yaw,
        });
        if let Ok((mut detector, stuck)) = vehicles.get_mut(entity) {
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
        if control == Some(PlayerControl::Local)
            && let Some(spawn) = spawn.as_ref()
        {
            // Trailer rigs re-seat at their authored offsets behind the
            // recovered tractor — the `resolve_stuck` pattern.
            let flat = Quat::from_rotation_y(yaw);
            for (trailer, offset) in &spawn.trailers {
                resets.write(ResetVehicle {
                    entity: Some(*trailer),
                    position: position + flat * *offset,
                    yaw,
                });
            }
        }
        report.recovered += 1;
    }
}
