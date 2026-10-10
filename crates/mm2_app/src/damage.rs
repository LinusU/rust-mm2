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
//!   Crash Course's restart maps onto the session's own `restart`
//!   intent — the lesson restarts from the beginning through the
//!   production lifecycle, never a shortcut. Blitz/Checkpoint break the
//!   car down for `BREAKDOWN_SECONDS` — dead engine, smoke still
//!   pouring — and [`resolve_breakdown`] then repairs it where it
//!   stands. AI opponents reset in
//!   place and repair regardless of mode (designed — the original's
//!   opponent-destruction behavior is unverified); remote participants
//!   take the same in-place arm on the authority that simulates them
//!   (F25-A.4 — the host owns their physics truth, and the reset rides
//!   the snapshot stream down; since F25-A.5 the bump is declared on
//!   the wire by `SnapEntry::epoch`, which is also how the *owning*
//!   client reconciles the reset onto its predicted car).
//!
//! Both systems are authority-gated and drain their input while the
//! session is not `Playing`, so a buffered stale event can never flush
//! damage or a reset into the next session or a pause.

use std::collections::HashMap;

use crate::police::{PoliceCar, recovery_driver};
use avian3d::prelude::*;
use bevy::prelude::*;
use mm2_game::{
    BREAKDOWN_SECONDS, DISABLED_PENALTY_TICKS, DamageEvent, DamageTier, DamageVerdict,
    DisabledOutcome, ImpactEvent, ImpactId, ImpairmentPolicy, ObjectId, ObjectIdentity, Player,
    PlayerControl, RaceState, Session, VehicleBreakdown, VehicleBreaks, VehicleDamage,
    VehicleStuck, disabled_outcome,
};
use mm2_vehicle::{EngineImpairment, ResetVehicle, UprightLanding};

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
    /// Episodes in which an engine went fully dead (factor 0) — a
    /// Blitz/Checkpoint breakdown, on the authority from its own
    /// episode and on a predicted client from the wire's destroyed
    /// byte. A limp that deepens into a wreck counts once more.
    pub dead: u64,
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
    // No impacts is nearly every step, and the index below walks every
    // identified entity — see `resolve_disabled`.
    if reader.is_empty() {
        return;
    }
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

/// Fixed-step, authority only: `--wreck-at` destroys one car once the
/// session clock reaches the flag's tick — the local car, or the seat
/// `--wreck-seat` names — and emits the [`DamageEvent`] an impact would,
/// so the wreck takes the production [`resolve_disabled`] arm for that
/// participant (a remote human's breakdown on the host included). The
/// evidence knob for a multi-process breakdown leg: nothing has to be
/// driven into a wall, and the outcome under test is the one real
/// destruction takes. One-shot; waits for its target to exist (a seat
/// that has not joined yet is not a miss) and is inert on a predicted
/// client, whose damage is the host's to decide.
pub fn dev_wreck_at(
    session: Res<Session>,
    mut cars: Query<(
        &ObjectIdentity,
        &mut VehicleDamage,
        Option<&Player>,
        Option<&crate::netdrive::NetPlayer>,
    )>,
    mut report: ResMut<DamageReport>,
    mut writer: MessageWriter<DamageEvent>,
    mut fired: Local<bool>,
) {
    if *fired || !session.is_playing() || !session.authority_role().is_authority() {
        return;
    }
    let Some((at, seat)) = session
        .config()
        .and_then(|c| c.dev.wreck_at.map(|at| (at, c.dev.wreck_seat)))
    else {
        return;
    };
    if session.tick() < at {
        return;
    }
    let Some((id, mut damage, ..)) = cars.iter_mut().find(|(_, _, player, net)| match seat {
        Some(seat) => net.is_some_and(|n| n.0 == seat),
        None => player.is_some_and(|p| p.control == PlayerControl::Local),
    }) else {
        return;
    };
    *fired = true;
    if damage.wreck() == DamageVerdict::Rejected {
        return;
    }
    report.applied += 1;
    report.disabled += 1;
    writer.write(DamageEvent {
        object: id.0,
        generation: session.generation(),
        tick: session.tick(),
        impact: ImpactId(0),
        severity: damage.total(),
        total: damage.total(),
        tier: damage.condition(),
    });
}

/// Fixed-step: enforce the session's disabled outcome on every
/// `Disabled`-tier [`DamageEvent`].
///
/// - local participant ([`PlayerControl::Local`]): the mode's
///   [`disabled_outcome`] — Cruise resets to the spawn point (trailers
///   included, the same reset `R` performs), Circuit resets in place
///   and adds [`DISABLED_PENALTY_TICKS`] to a live race clock,
///   `Breakdown` starts a [`VehicleBreakdown`] episode that
///   [`resolve_breakdown`] ends with a repair, and `RestartEvent`
///   queues the session's own restart intent so the lesson restarts
///   through the production lifecycle. Reset outcomes
///   repair the damage with them — a reset wreck that stays wrecked
///   would just disable again next contact.
/// - AI and remote participants: reset in place and repair under every
///   mode (designed — original opponent-destruction behavior is
///   unverified, UNK-13; the alternative, a wreck ending its race
///   permanently, would silently shrink the field, and a remote driver's
///   wreck must never restart the event or tax the shared race clock
///   for everyone else on the wire). On the host the remote car is a
///   real participant this system owns; a predicted client's copies
///   never reach this code — the whole system early-returns without
///   `AuthorityRole::Authority`.
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
    identities: Query<(Entity, &ObjectIdentity, Option<&Player>, Has<PoliceCar>)>,
    landing: UprightLanding,
    mut damaged: Query<(&mut VehicleDamage, Has<VehicleBreakdown>)>,
    mut stuck: Query<&mut VehicleStuck>,
    mut resets: MessageWriter<ResetVehicle>,
    mut report: ResMut<DamageReport>,
    mut breaks: Query<&mut VehicleBreaks>,
    mut break_visuals: Query<(&BreakPartVisual, &mut Visibility, &ChildOf)>,
    mut break_report: ResMut<BreakReport>,
    mut texel: crate::texel_fx::TexelRepair,
    mut commands: Commands,
) {
    // A disabled car is rare, and the index below is a walk over every
    // identified entity with a hash insert each — measured at ~0.2 ms of
    // every 60 Hz step on a full city when built unconditionally.
    if reader.is_empty() {
        return;
    }
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
        if event.generation != generation || event.tier != DamageTier::Disabled {
            continue;
        }
        let Some(&(entity, control_kind)) = index.get(&event.object) else {
            continue;
        };
        let Ok((mut damage, broken_down)) = damaged.get_mut(entity) else {
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
        // reset-path contract).
        if control_kind.is_some()
            && let Ok(mut detector) = stuck.get_mut(entity)
        {
            detector.disarm();
        }
        match control_kind {
            Some(PlayerControl::Local) => {
                let mode = session.config().map(|c| c.mode.clone()).unwrap_or_default();
                match disabled_outcome(&mode) {
                    DisabledOutcome::Breakdown => {
                        // Further blows on a car already down change
                        // nothing — the episode owns the wreck.
                        if !broken_down {
                            info!(
                                total = damage.total(),
                                tick = event.tick,
                                seconds = BREAKDOWN_SECONDS,
                                "player vehicle destroyed — broken down"
                            );
                            commands.entity(entity).insert(VehicleBreakdown::new());
                        }
                    }
                    DisabledOutcome::RestartEvent => {
                        // The session's own restart path tears down and
                        // `begin`s the event again — "restarting from
                        // the beginning" is the production lifecycle,
                        // not a damage-specific shortcut. The wrecked
                        // state dies with the session entities.
                        info!(
                            total = damage.total(),
                            tick = event.tick,
                            "player vehicle destroyed — restarting the lesson"
                        );
                        control.restart = true;
                    }
                    DisabledOutcome::FreeReset => {
                        if let Some(spawn) = spawn.as_ref() {
                            resets.write(ResetVehicle {
                                entity: Some(entity),
                                position: spawn.position,
                                yaw: spawn.yaw,
                            });
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
                        // F05-B.9: repair also clears the skin —
                        // `fxTexelDamage::Reset` re-blits the clean
                        // texture over the car's clone.
                        texel.reset(entity);
                        report.recovered += 1;
                    }
                    DisabledOutcome::PenaltyReset => {
                        if let Some((position, yaw)) = landing.of(entity) {
                            resets.write(ResetVehicle {
                                entity: Some(entity),
                                position,
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
                        texel.reset(entity);
                        report.recovered += 1;
                    }
                }
            }
            // AI and remote wrecks share the in-place arm — on the
            // host a remote car is this authority's participant, so its
            // reset+repair rides the next snapshot down (F25-A.4).
            // ... except a remote human in a Breakdown mode, who takes
            // the same dead interval the host's own driver does
            // (networked policy, DSN-68 follow-up): the episode is
            // authority state on the remote seat's car, its engine
            // dies in the host's sim, and the wire carries it as the
            // damage byte holding at the destruction bound.
            Some(PlayerControl::Remote)
                if session
                    .config()
                    .is_some_and(|c| disabled_outcome(&c.mode) == DisabledOutcome::Breakdown) =>
            {
                if !broken_down {
                    info!(
                        total = damage.total(),
                        tick = event.tick,
                        seconds = BREAKDOWN_SECONDS,
                        "remote vehicle destroyed — broken down"
                    );
                    commands.entity(entity).insert(VehicleBreakdown::new());
                }
            }
            Some(PlayerControl::Ai) | Some(PlayerControl::Remote) => {
                if let Some((position, yaw)) = landing.of(entity) {
                    resets.write(ResetVehicle {
                        entity: Some(entity),
                        position,
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
                texel.reset(entity);
                report.recovered += 1;
            }
            // An unidentified object has no driver to resolve for (a
            // cop resolves as AI — `recovery_driver`).
            None => {}
        }
    }
}

/// Fixed-step: run each [`VehicleBreakdown`] episode down and, when it
/// ends, repair the car where it stands — the damage clears, detached
/// breakaway parts re-attach and the skin is wiped, the same repair
/// the reset outcomes perform. The race clock never stops for it; the
/// five dead seconds are the cost. Frozen while the session is not
/// `Playing`, so a pause cannot burn the episode down.
#[allow(clippy::too_many_arguments)] // Bevy system: the repair threads the same handles `resolve_disabled` does
pub fn resolve_breakdown(
    session: Res<Session>,
    time: Res<Time<Fixed>>,
    mut cars: Query<(Entity, &mut VehicleBreakdown, &mut VehicleDamage)>,
    mut breaks: Query<&mut VehicleBreaks>,
    mut break_visuals: Query<(&BreakPartVisual, &mut Visibility, &ChildOf)>,
    mut break_report: ResMut<BreakReport>,
    mut texel: crate::texel_fx::TexelRepair,
    mut report: ResMut<DamageReport>,
    mut commands: Commands,
) {
    if !session.is_playing() || !session.authority_role().is_authority() {
        return;
    }
    let dt = time.delta_secs();
    for (entity, mut episode, mut damage) in &mut cars {
        if !episode.tick(dt) {
            continue;
        }
        damage.reset();
        if let Ok(mut rig) = breaks.get_mut(entity) {
            break_report.restored +=
                crate::breakaway::restore_rig(entity, &mut rig, &mut break_visuals, &mut commands)
                    as u64;
        }
        texel.reset(entity);
        commands.entity(entity).remove::<VehicleBreakdown>();
        report.recovered += 1;
        info!(
            tick = session.tick(),
            "player vehicle repaired after its breakdown"
        );
    }
}

/// [`sync_impairment`]'s view of a car: its damage, its live impairment
/// and whether a breakdown holds the engine dead. `RemoteReplica`
/// copies are excluded — they are never stepped by the sim.
type ImpairedCars<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static VehicleDamage,
        Option<&'static mut EngineImpairment>,
        Has<VehicleBreakdown>,
    ),
    Without<mm2_vehicle::RemoteReplica>,
>;

/// Fixed-step: mirror each participant's authored damage into the
/// physics-side [`EngineImpairment`] the sim consumes — F05-B.7, the
/// designed smoke↔engine-torque coupling (DSN-25). MM2Hook's
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
/// lifts it gradually. Remote participants impair like AI (F25-A.4):
/// the host simulates their cars, so their engine power is the
/// authority's to weaken.
///
/// Under a predicted session this system still runs — the v8 snap
/// tail writes the authority's damage total onto the local seat
/// (F25-B), so the predicted car must weaken the same way the
/// authority's copy of it does, or the client drives a stronger car
/// than the wire reports. `RemoteReplica` copies are excluded: they
/// are never stepped by `vehicle_simulation`, so the component would
/// be dead weight. Entering/leaving the impaired band counts into
/// [`DamageReport::impaired`]/[`restored`], the headless record's
/// `imp=` field — on a client that count is replicated episodes.
pub fn sync_impairment(
    mut commands: Commands,
    session: Res<Session>,
    mut cars: ImpairedCars,
    mut report: ResMut<DamageReport>,
) {
    if !session.is_playing() {
        return;
    }
    let policy = ImpairmentPolicy::default();
    let predicted_breakdown = !session.authority_role().is_authority()
        && session
            .config()
            .is_some_and(|c| disabled_outcome(&c.mode) == DisabledOutcome::Breakdown);
    for (entity, damage, impairment, broken_down) in &mut cars {
        // A broken-down car's engine is dead, not merely limping.
        // A predicted client never owns the episode; the wire's damage
        // byte holding at the destruction bound is its signal, and in a
        // Breakdown mode that bound *is* the breakdown (the authority
        // repairs by dropping the byte).
        let wire_down = predicted_breakdown && damage.condition() == DamageTier::Disabled;
        let factor = if broken_down || wire_down {
            0.0
        } else {
            policy.factor(damage.total(), &damage.spec)
        };
        match impairment {
            Some(mut imp) => {
                if factor >= 1.0 {
                    commands.entity(entity).remove::<EngineImpairment>();
                    report.restored += 1;
                } else {
                    report.dead += u64::from(factor <= 0.0 && imp.0 > 0.0);
                    imp.0 = factor;
                }
            }
            None if factor < 1.0 => {
                commands.entity(entity).insert(EngineImpairment(factor));
                report.impaired += 1;
                report.dead += u64::from(factor <= 0.0);
            }
            None => {}
        }
    }
}
