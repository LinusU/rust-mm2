//! F05-B.1 — runtime impact→damage application and disabled outcomes.
//!
//! The contract types live in `mm2_game::damage` ([`VehicleDamage`],
//! [`DamageEvent`]); this module is where they meet Avian and the
//! session lifecycle, the same split every other game contract uses:
//!
//! - [`apply_impact_damage`] consumes the bounded, deduplicated
//!   [`ImpactEvent`] stream [`crate::contracts::collect_impacts`]
//!   produces and accumulates it into each participant's
//!   [`VehicleDamage`]. The delivered quantity is impulse-scale —
//!   approach speed × the other participant's mass, matching the
//!   authored bounds' units (`MaxDamage` tracks vehicle mass, DMG-6) —
//!   with the vehicle's own mass standing in when the other side is the
//!   static world or has no resolvable mass (a wall is "struck" by the
//!   car itself). The conversion is a disclosed designed estimate
//!   (DSN-10/UNK-13): the authored floor and bounds are original data,
//!   the severity→impulse mapping is not verified.
//! - [`resolve_disabled`] reacts to `Disabled`-tier [`DamageEvent`]s
//!   with the session's [`disabled_outcome`] policy (RACE-5/DMG-2):
//!   Cruise resets to the spawn point, Circuit resets in place with a
//!   [`DISABLED_PENALTY_TICKS`] race-clock penalty, and the
//!   Blitz/Checkpoint/CrashCourse restart maps onto the session's own
//!   `restart` intent — the event restarts from the beginning through
//!   the production lifecycle, never a shortcut. AI opponents reset in
//!   place and repair regardless of mode (designed — the original's
//!   opponent-destruction behavior is unverified); remote participants
//!   resolve under their own authority (F25+ territory).
//!
//! Both systems are authority-gated and drain their input while the
//! session is not `Playing`, so a buffered stale event can never flush
//! damage or a reset into the next session or a pause.

use std::collections::HashMap;

use avian3d::prelude::*;
use bevy::prelude::*;
use mm2_game::{
    DISABLED_PENALTY_TICKS, DamageEvent, DamageTier, DamageVerdict, DisabledOutcome, ImpactEvent,
    ImpairmentPolicy, ObjectId, ObjectIdentity, Player, PlayerControl, RaceState, Session,
    VehicleBreaks, VehicleDamage, VehicleStuck, disabled_outcome,
};
use mm2_vehicle::{EngineImpairment, ResetVehicle};

use crate::breakaway::{BreakPartVisual, BreakReport};
use crate::session::{SessionControl, SpawnPoint};

/// Per-session evidence counters for the damage pipeline — the `dmg=`
/// field of the headless smoke record and the cheapest "is the
/// pipeline live" check. Session-scoped like [`crate::contracts::ImpactFilter`]:
/// `drive_session` resets it during teardown so counts describe the
/// active session's stream.
#[derive(Resource, Debug, Default)]
pub struct DamageReport {
    /// Impact severities that accumulated damage.
    pub applied: u64,
    /// Deliveries rejected below `ImpactThreshold` or non-finite.
    pub rejected: u64,
    /// Re-delivered/out-of-order impact ids suppressed by the
    /// [`VehicleDamage`] watermark (F05-AC06).
    pub duplicate: u64,
    /// Applications that left a vehicle `Disabled`.
    pub disabled: u64,
    /// Disabled outcomes that repaired and reset a vehicle in place or
    /// at spawn (a `RestartEvent` tears the session down instead).
    pub recovered: u64,
    /// Impairment episodes — a participant's engine factor dropped
    /// below 1.0 under the DSN-25 smoke↔torque coupling (F05-B.7).
    pub impaired: u64,
    /// Impairment episodes cleared — the factor returned to 1.0
    /// through a repair (never counted on despawn or teardown).
    pub restored: u64,
}

impl DamageReport {
    /// Forget all session-scoped counts — called on teardown, same
    /// contract as [`crate::contracts::ImpactFilter::reset`].
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// The finite positive mass of an entity, or `None` — the same filter
/// [`crate::contracts::impulse_estimate`] applies so an unresolvable
/// mass degrades to a light touch, not a hidden force.
fn mass_of(masses: &Query<&ComputedMass>, entity: Entity) -> Option<f32> {
    masses
        .get(entity)
        .ok()
        .map(|m| m.value())
        .filter(|m| m.is_finite() && *m > 0.0)
}

/// Fixed-step: accumulate the deduplicated impact stream into each
/// participant's authored [`VehicleDamage`], emitting one
/// [`DamageEvent`] per accepted application.
///
/// Per impact, each participant's delivered impulse is
/// `severity × other_mass` — for a wall hit the striker is the car
/// itself, so its own mass stands in; a missing mass anywhere degrades
/// to 1 kg (a touch, not a phantom wreck). Participants without a
/// [`VehicleDamage`] component — anything with no authored
/// `vehcardamage`, props, ambient cars — are skipped, so authored
/// absence stays undamageable rather than fabricating a spec.
pub fn apply_impact_damage(
    mut reader: MessageReader<ImpactEvent>,
    session: Res<Session>,
    identities: Query<(Entity, &ObjectIdentity)>,
    masses: Query<&ComputedMass>,
    mut damaged: Query<&mut VehicleDamage>,
    mut report: ResMut<DamageReport>,
    mut writer: MessageWriter<DamageEvent>,
) {
    // Drain regardless of phase/authority — events buffered while
    // Loading/Paused or produced under a remote authority would
    // otherwise flush as a stale burst (or self-authored damage a
    // predicted client must never apply, F05 req 6).
    if !session.is_playing() || !session.authority_role().is_authority() {
        reader.read().for_each(drop);
        return;
    }
    let tick = session.tick();
    let generation = session.generation();
    let index: HashMap<ObjectId, Entity> = identities
        .iter()
        .map(|(entity, id)| (id.0, entity))
        .collect();

    for event in reader.read() {
        if event.generation != generation {
            continue;
        }
        let resolved = [
            (
                event.participants.0,
                index.get(&event.participants.0).copied(),
            ),
            (
                event.participants.1,
                index.get(&event.participants.1).copied(),
            ),
        ];
        for side in 0..2 {
            let Some(entity) = resolved[side].1 else {
                continue;
            };
            let Ok(mut damage) = damaged.get_mut(entity) else {
                continue;
            };
            let mass = resolved[1 - side]
                .1
                .and_then(|other| mass_of(&masses, other))
                .or_else(|| mass_of(&masses, entity))
                .unwrap_or(1.0);
            let verdict = damage.apply(event.id, event.severity * mass);
            match verdict {
                DamageVerdict::Rejected => report.rejected += 1,
                DamageVerdict::Duplicate => report.duplicate += 1,
                _ => {
                    report.applied += 1;
                    let tier = damage.condition();
                    if tier == DamageTier::Disabled {
                        report.disabled += 1;
                    }
                    writer.write(DamageEvent {
                        object: resolved[side].0,
                        generation,
                        tick,
                        impact: event.id,
                        severity: event.severity * mass,
                        total: damage.total(),
                        tier,
                    });
                }
            }
        }
    }
}

/// Fixed-step: enforce the session's disabled outcome on every
/// `Disabled`-tier [`DamageEvent`].
///
/// - local participant ([`PlayerControl::Local`]): the mode's
///   [`disabled_outcome`] — Cruise resets to the spawn point (trailers
///   included, the same reset `R` performs), Circuit resets in place
///   and adds [`DISABLED_PENALTY_TICKS`] to a live race clock, and
///   `RestartEvent` queues the session's own restart intent so the
///   event restarts through the production lifecycle. Reset outcomes
///   repair the damage with them — a reset wreck that stays wrecked
///   would just disable again next contact.
/// - AI participant: resets in place and repairs under every mode
///   (designed — original opponent-destruction behavior is unverified,
///   UNK-13; the alternative, an opponent wreck ending its race
///   permanently, would silently shrink the field).
/// - remote participant: skipped — its authority resolves it (F25+).
///
/// The outcome re-checks the live tier: a `Disabled` event whose state
/// was already repaired by an earlier resolution in the same tick is a
/// no-op, so two disabling impacts in one step can never double-reset
/// or double-penalize (F05-AC06).
#[allow(clippy::too_many_arguments)] // Bevy system: outcome resolution threads the session handles it acts on
pub fn resolve_disabled(
    mut reader: MessageReader<DamageEvent>,
    session: Res<Session>,
    mut control: ResMut<SessionControl>,
    spawn: Option<Res<SpawnPoint>>,
    mut race: Option<ResMut<RaceState>>,
    identities: Query<(Entity, &ObjectIdentity, Option<&Player>)>,
    poses: Query<(&Position, &Rotation)>,
    mut damaged: Query<&mut VehicleDamage>,
    mut stuck: Query<&mut VehicleStuck>,
    mut resets: MessageWriter<ResetVehicle>,
    mut report: ResMut<DamageReport>,
    mut breaks: Query<&mut VehicleBreaks>,
    mut break_visuals: Query<(&BreakPartVisual, &mut Visibility, &ChildOf)>,
    mut break_report: ResMut<BreakReport>,
    mut commands: Commands,
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
        if event.generation != generation || event.tier != DamageTier::Disabled {
            continue;
        }
        let Some(&(entity, control_kind)) = index.get(&event.object) else {
            continue;
        };
        let Ok(mut damage) = damaged.get_mut(entity) else {
            continue;
        };
        // Idempotent: an earlier resolution in this tick already
        // repaired the state this event describes.
        if damage.condition() != DamageTier::Disabled {
            continue;
        }
        // The disabled outcome owns the wreck: its recovery (reset or
        // restart) supersedes any armed stuck episode, so the detector
        // disarms here rather than fire a second recovery into the
        // pose the outcome lands the car in (`VehicleStuck::disarm`'s
        // reset-path contract). Remote participants' detectors belong
        // to their own authority.
        if matches!(
            control_kind,
            Some(PlayerControl::Local) | Some(PlayerControl::Ai)
        ) && let Ok(mut detector) = stuck.get_mut(entity)
        {
            detector.disarm();
        }
        match control_kind {
            Some(PlayerControl::Local) => {
                let mode = session.config().map(|c| c.mode.clone()).unwrap_or_default();
                match disabled_outcome(&mode) {
                    DisabledOutcome::RestartEvent => {
                        // The session's own restart path tears down and
                        // `begin`s the event again — "restarting from
                        // the beginning" is the production lifecycle,
                        // not a damage-specific shortcut. The wrecked
                        // state dies with the session entities.
                        control.restart = true;
                    }
                    DisabledOutcome::FreeReset => {
                        if let Some(spawn) = spawn.as_ref() {
                            resets.write(ResetVehicle {
                                entity: Some(entity),
                                position: spawn.position,
                                yaw: spawn.yaw,
                            });
                            let rot = Quat::from_rotation_y(spawn.yaw);
                            for (trailer, offset) in &spawn.trailers {
                                resets.write(ResetVehicle {
                                    entity: Some(*trailer),
                                    position: spawn.position + rot * *offset,
                                    yaw: spawn.yaw,
                                });
                            }
                        }
                        damage.reset();
                        // Repair restores the rig (F05-AC03): detached
                        // breakaway parts re-attach, their fragments
                        // despawn. `restore_rig` is a no-op on a rig
                        // with nothing off it.
                        if let Ok(mut rig) = breaks.get_mut(entity) {
                            break_report.restored += crate::breakaway::restore_rig(
                                entity,
                                &mut rig,
                                &mut break_visuals,
                                &mut commands,
                            ) as u64;
                        }
                        report.recovered += 1;
                    }
                    DisabledOutcome::PenaltyReset => {
                        if let Some((position, rotation)) =
                            poses.get(entity).ok().filter(|(p, _)| p.0.is_finite())
                        {
                            let (_, yaw, _) = rotation.0.to_euler(EulerRot::YXZ);
                            resets.write(ResetVehicle {
                                entity: Some(entity),
                                position: position.0,
                                yaw,
                            });
                        }
                        if let Some(race) = race.as_mut()
                            && !race.is_stale(generation)
                        {
                            race.clock = race.clock.saturating_add(DISABLED_PENALTY_TICKS);
                        }
                        damage.reset();
                        // Same repair-restores-rig rule as FreeReset.
                        if let Ok(mut rig) = breaks.get_mut(entity) {
                            break_report.restored += crate::breakaway::restore_rig(
                                entity,
                                &mut rig,
                                &mut break_visuals,
                                &mut commands,
                            ) as u64;
                        }
                        report.recovered += 1;
                    }
                }
            }
            Some(PlayerControl::Ai) => {
                if let Some((position, rotation)) =
                    poses.get(entity).ok().filter(|(p, _)| p.0.is_finite())
                {
                    let (_, yaw, _) = rotation.0.to_euler(EulerRot::YXZ);
                    resets.write(ResetVehicle {
                        entity: Some(entity),
                        position: position.0,
                        yaw,
                    });
                }
                damage.reset();
                // Same repair-restores-rig rule as FreeReset.
                if let Ok(mut rig) = breaks.get_mut(entity) {
                    break_report.restored += crate::breakaway::restore_rig(
                        entity,
                        &mut rig,
                        &mut break_visuals,
                        &mut commands,
                    ) as u64;
                }
                report.recovered += 1;
            }
            // Remote participants resolve under their own authority —
            // the host-of-record path is F25+ territory.
            Some(PlayerControl::Remote) | None => {}
        }
    }
}

/// Fixed-step: mirror each local/AI participant's authored damage into
/// the physics-side [`EngineImpairment`] the sim consumes — F05-B.7,
/// the designed smoke↔engine-torque coupling (DSN-25). MM2Hook's
/// `PhysicalEngineDamage` option documents the original rule —
/// "when the engine spews smoke … less acceleration and less top
/// speed" — so impairment keys on the same `MedDamage` bound the
/// DSN-24 smoke tier does; the ramp and floors are designed values
/// (UNK-13 stands for the original's shape).
///
/// [`EngineImpairment`] exists exactly while the factor is below 1 —
/// an undamaged car carries no component and the sim path is
/// bit-identical. Running after [`resolve_disabled`] in the chain
/// means a wreck's repair clears the factor the same tick the state
/// returns to `Intact`; regeneration, where a session lets it run,
/// lifts it gradually. Remote participants are skipped like every
/// F05 system — their authority impairs its own sim (F25+).
/// Entering/leaving the impaired band counts into
/// [`DamageReport::impaired`]/[`restored`], the headless record's
/// `imp=` field.
pub fn sync_impairment(
    mut commands: Commands,
    session: Res<Session>,
    mut cars: Query<(
        Entity,
        &VehicleDamage,
        Option<&Player>,
        Option<&mut EngineImpairment>,
    )>,
    mut report: ResMut<DamageReport>,
) {
    if !session.is_playing() || !session.authority_role().is_authority() {
        return;
    }
    let policy = ImpairmentPolicy::default();
    for (entity, damage, player, impairment) in &mut cars {
        if player.is_some_and(|p| p.control == PlayerControl::Remote) {
            continue;
        }
        let factor = policy.factor(damage.total(), &damage.spec);
        match impairment {
            Some(mut imp) => {
                if factor >= 1.0 {
                    commands.entity(entity).remove::<EngineImpairment>();
                    report.restored += 1;
                } else {
                    imp.0 = factor;
                }
            }
            None if factor < 1.0 => {
                commands.entity(entity).insert(EngineImpairment(factor));
                report.impaired += 1;
            }
            None => {}
        }
    }
}
