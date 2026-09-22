//! F05-B.2 — authored `vehstuck` consumption: impact-armed stuck
//! detection plus the bounded in-place recovery.
//!
//! The contract lives in `mm2_game::stuck` ([`VehicleStuck`],
//! [`StuckEvent`]); this module is where it meets the impact stream and
//! the session lifecycle, the same split [`crate::damage`] uses:
//!
//! - [`track_stuck`] arms each participant's detector at the pose it was
//!   in when an [`ImpactEvent`] delivered — the recovered struct's
//!   `m_LastImpactPos` — then advances every armed detector once per
//!   fixed step, emitting one [`StuckEvent`] per fired episode.
//! - [`resolve_stuck`] answers a detection with the bounded recovery:
//!   [`ResetVehicle`] onto the [`upright_recovery_pose`] — heading kept,
//!   hull dropped onto the surface it already rests on, in place. In
//!   place is the safe reading of the authored bounds: `Rotation` is 0
//!   on every retail record (a yaw change is not authored) and
//!   `Translation` ≈ 0.1 m (no positional rescue is authored either —
//!   a car wedged between colliders gets righted, not teleported out;
//!   the driver's own reset key or a destruction outcome covers that).
//!   The reset marks the car [`Teleported`], so even this in-place hop
//!   can never sweep a checkpoint (F05 req 5).
//!
//! Policy: the local participant and AI opponents recover the same way;
//! a remote participant's authority owns its detector (F05 req 6), and
//! a `Disabled` wreck belongs to [`crate::damage::resolve_disabled`],
//! so the observe leg skips both. Both systems drain their input while
//! the session is not `Playing`, so a buffered event can never flush a
//! reset into the next session or a pause.

use std::collections::HashMap;

use avian3d::prelude::*;
use bevy::prelude::*;
use mm2_game::{
    DamageTier, ImpactEvent, ObjectId, ObjectIdentity, Player, PlayerControl, Session, StuckEvent,
    StuckVerdict, VehicleDamage, VehicleStuck,
};
use mm2_vehicle::{ResetVehicle, Vehicle, upright_recovery_pose};

use crate::session::SpawnPoint;

/// Per-session evidence counters for the stuck pipeline — the `vsk=`
/// field of the headless smoke record. Session-scoped like
/// [`crate::damage::DamageReport`]: `drive_session` resets it during
/// teardown so counts describe the active session's stream.
#[derive(Resource, Debug, Default)]
pub struct StuckReport {
    /// Impact deliveries that (re-)anchored a detector.
    pub armed: u64,
    /// Armed episodes that held inside `pos_thresh` for `time_thresh`.
    pub detections: u64,
    /// Detections that resolved to a recovery reset — a stale or
    /// remote-owned event does not count.
    pub recovered: u64,
}

impl StuckReport {
    /// Forget all session-scoped counts — called on teardown, same
    /// contract as [`crate::damage::DamageReport::reset`].
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

type StuckVehicles<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static ObjectIdentity,
        Option<&'static Player>,
        &'static Position,
        &'static Rotation,
        &'static mut VehicleStuck,
        Option<&'static VehicleDamage>,
    ),
>;

/// Fixed-step: arm [`VehicleStuck`] detectors off the deduplicated
/// [`ImpactEvent`] stream, then advance every detector once — one
/// [`StuckEvent`] per episode that held inside `pos_thresh` for the
/// authored `time_thresh`.
///
/// Remote participants are skipped on both legs: a predicted client's
/// stuck state is its authority's to declare (F05 req 6). A participant
/// with no `vehstuck` record has no component and is skipped like an
/// undamageable car is — authored absence, never a fabricated spec.
pub fn track_stuck(
    mut reader: MessageReader<ImpactEvent>,
    session: Res<Session>,
    time: Res<Time<Fixed>>,
    mut vehicles: StuckVehicles,
    mut writer: MessageWriter<StuckEvent>,
    mut report: ResMut<StuckReport>,
) {
    // Drain regardless of phase/authority — events buffered while
    // Loading/Paused or produced under a remote authority would
    // otherwise flush as a stale burst (same contract as
    // `apply_impact_damage`).
    if !session.is_playing() || !session.authority_role().is_authority() {
        reader.read().for_each(drop);
        return;
    }
    let generation = session.generation();
    let index: HashMap<ObjectId, Entity> = vehicles
        .iter()
        .map(|(entity, id, ..)| (id.0, entity))
        .collect();

    // Arm: every delivered impact re-anchors the participants it hit.
    for event in reader.read() {
        if event.generation != generation {
            continue;
        }
        for side in [event.participants.0, event.participants.1] {
            let Some(&entity) = index.get(&side) else {
                continue;
            };
            let Ok((.., player, pos, rot, mut stuck, damage)) = vehicles.get_mut(entity) else {
                continue;
            };
            if player.is_some_and(|p| p.control == PlayerControl::Remote) {
                continue;
            }
            // A `Disabled` wreck belongs to the damage outcome — an
            // impact into it does not start a stuck episode.
            if damage.is_some_and(|d| d.condition() == DamageTier::Disabled) {
                continue;
            }
            stuck.impact(pos.0, rot.0);
            report.armed += 1;
        }
    }

    // Observe: a `Disabled` wreck belongs to the damage outcome, not to
    // the stuck detector — a wrecked car cannot move by definition.
    let dt = time.delta_secs();
    let tick = session.tick();
    for (.., id, player, pos, rot, mut stuck, damage) in &mut vehicles {
        if player.is_some_and(|p| p.control == PlayerControl::Remote) {
            continue;
        }
        if damage.is_some_and(|d| d.condition() == DamageTier::Disabled) {
            continue;
        }
        if stuck.observe(pos.0, rot.0, dt) == StuckVerdict::Stuck {
            writer.write(StuckEvent {
                object: id.0,
                generation,
                tick,
            });
            report.detections += 1;
        }
    }
}

/// Fixed-step: answer each [`StuckEvent`] with the bounded in-place
/// recovery — [`ResetVehicle`] onto the [`upright_recovery_pose`], the
/// same landing [`vehicle_self_right`] computes. Local and AI
/// participants recover identically (designed — the original's
/// opponent stuck behavior is unverified, UNK-13); remote participants
/// resolve under their own authority. The local participant's trailers
/// re-seat behind the tractor's recovered pose — the same offsets the
/// cruise disabled outcome uses, anchored on the live pose instead of
/// the spawn point.
///
/// [`vehicle_self_right`]: mm2_vehicle::systems::vehicle_self_right
pub fn resolve_stuck(
    mut reader: MessageReader<StuckEvent>,
    session: Res<Session>,
    spawn: Option<Res<SpawnPoint>>,
    identities: Query<(Entity, &ObjectIdentity, Option<&Player>)>,
    vehicles: Query<(&Position, &Rotation, &Vehicle)>,
    mut resets: MessageWriter<ResetVehicle>,
    mut report: ResMut<StuckReport>,
) {
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
        // Only local and AI participants resolve here — a remote
        // participant's authority owns its recovery, and an unidentified
        // object has no driver to recover for (same policy as
        // `resolve_disabled`).
        if !matches!(
            control,
            Some(PlayerControl::Local) | Some(PlayerControl::Ai)
        ) {
            continue;
        }
        let Ok((pos, rot, vehicle)) = vehicles.get(entity) else {
            continue;
        };
        let (landing, yaw) = upright_recovery_pose(&vehicle.config, pos.0, rot.0);
        resets.write(ResetVehicle {
            entity: Some(entity),
            position: landing,
            yaw,
        });
        if control == Some(PlayerControl::Local)
            && let Some(spawn) = spawn.as_ref()
        {
            // Trailer rigs re-seat at their authored offsets behind the
            // recovered tractor — the FreeReset pattern, anchored on the
            // live pose instead of the spawn point.
            let flat = Quat::from_rotation_y(yaw);
            for (trailer, offset) in &spawn.trailers {
                resets.write(ResetVehicle {
                    entity: Some(*trailer),
                    position: landing + flat * *offset,
                    yaw,
                });
            }
        }
        report.recovered += 1;
    }
}
