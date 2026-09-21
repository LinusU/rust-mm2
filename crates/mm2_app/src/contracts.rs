//! Producers for the F01-B shared contracts.
//!
//! The contract types live in `mm2_game`; turning Avian's solver data
//! into them happens here, because the app is where the game contracts
//! and the physics engine are allowed to meet:
//!
//! - [`collect_impacts`] filters `CollisionStart` edges into bounded,
//!   deduplicated [`ImpactEvent`]s — one per physical impact, never raw
//!   contact spam (F01 spec requirement 5).
//! - [`publish_vehicle_telemetry`] writes the read-only
//!   [`VehicleTelemetry`] snapshot consumers read instead of the mutable
//!   simulation state.
//!
//! Both run in `FixedLast`, after the physics step in `FixedPostUpdate`,
//! and only while the session is `Playing` — telemetry freezes with the
//! session clock rather than stamping a stale tick onto moving physics.

use avian3d::prelude::*;
use bevy::prelude::*;
use mm2_game::{
    AuthorityRole, DamageSignals, ImpactDedup, ImpactEvent, ImpactId, ImpactPolicy, ObjectId,
    ObjectIdentity, Session, SurfaceMaterial, SurfaceState, VehicleTelemetry, WheelTelemetry,
};
use mm2_vehicle::surface::TireConditions;
use mm2_vehicle::vehicle::{Vehicle, VehicleState};

/// Impact reporting bookkeeping for [`collect_impacts`]. The dedup
/// window, per-tick bound and the counters that make suppression visible
/// live here so the stream consumers see is already filtered.
#[derive(Resource, Debug)]
pub struct ImpactFilter {
    dedup: ImpactDedup,
    policy: ImpactPolicy,
    next: u64,
    /// Events emitted so far — evidence the pipeline is live.
    pub emitted: u64,
    /// Candidates dropped by [`ImpactPolicy::max_per_tick`], cumulative.
    pub dropped: u64,
}

impl ImpactFilter {
    pub fn new(policy: ImpactPolicy) -> Self {
        Self {
            dedup: ImpactDedup::new(policy.pair_cooldown_ticks),
            policy,
            next: 0,
            emitted: 0,
            dropped: 0,
        }
    }
}

impl Default for ImpactFilter {
    fn default() -> Self {
        Self::new(ImpactPolicy::default())
    }
}

impl ImpactFilter {
    /// Forget all session-scoped impact bookkeeping — called on session
    /// teardown. The dedup map is keyed by `Entity`, which the next
    /// session may recycle, so a stale entry would silently suppress a
    /// new session's first impact on the recycled pair. The id counter
    /// and evidence counters restart too: `ImpactId` is per-session
    /// (events carry `generation`) and `emitted`/`dropped` describe the
    /// active session's stream.
    pub fn reset(&mut self) {
        self.dedup.clear();
        self.next = 0;
        self.emitted = 0;
        self.dropped = 0;
    }
}

/// The deepest contact of a live pair: its world point, the manifold
/// normal (pointing from `c1` toward `c2`) and the pre-solver approach
/// speed (m/s, ≥ 0 while approaching). The impact pipeline and the
/// banger activation walk the same manifold data so "the hit" means
/// the same thing to both.
pub(crate) fn deepest_contact(
    collisions: &Collisions,
    c1: Entity,
    c2: Entity,
) -> Option<(Vec3, Vec3, f32)> {
    let pair = collisions.get(c1, c2)?;
    let (contact, normal) = pair
        .manifolds
        .iter()
        .filter_map(|m| m.find_deepest_contact().map(|c| (c, m.normal)))
        .max_by(|a, b| {
            a.0.penetration
                .partial_cmp(&b.0.penetration)
                .unwrap_or(std::cmp::Ordering::Equal)
        })?;
    Some((contact.point, normal, (-contact.normal_speed).max(0.0)))
}

/// The stable identity of a contact side: its collider's
/// [`ObjectIdentity`], else its body's, else [`ObjectId::WORLD`].
fn object_of(
    collider: Entity,
    body: Option<Entity>,
    identities: &Query<&ObjectIdentity>,
) -> ObjectId {
    identities
        .get(collider)
        .ok()
        .or_else(|| body.and_then(|b| identities.get(b).ok()))
        .map(|i| i.0)
        .unwrap_or(ObjectId::WORLD)
}

/// Fixed-step: turn solver contact edges into bounded `ImpactEvent`s.
///
/// `CollisionStart` only fires when a pair *begins* touching, but that is
/// still too raw for consumers: a car settling flaps the same pair over
/// consecutive steps, and a pile-up can produce more edges in one tick
/// than any sound/damage/network consumer should see. The policy keeps
/// one event per physical impact (`ImpactDedup`), drops sub-threshold
/// touches, and caps emission per tick keeping the most severe.
// Bevy systems legitimately take one param per resource/query; this one
// joins contact events, identities, surfaces and damage.
#[allow(clippy::too_many_arguments)]
pub fn collect_impacts(
    mut reader: MessageReader<CollisionStart>,
    collisions: Collisions,
    identities: Query<&ObjectIdentity>,
    vehicles: Query<(), With<Vehicle>>,
    surfaces: Query<&SurfaceMaterial>,
    conditions: Res<TireConditions>,
    mut damage: Query<&mut DamageSignals>,
    mut filter: ResMut<ImpactFilter>,
    session: Res<Session>,
    mut writer: MessageWriter<ImpactEvent>,
) {
    // Drain regardless of phase — events buffered while Loading/Paused
    // would otherwise flush as a stale burst on resume.
    if !session.is_playing() {
        reader.read().for_each(drop);
        return;
    }
    let tick = session.tick();
    let generation = session.generation();
    let policy = filter.policy;

    let mut candidates = Vec::new();
    for event in reader.read() {
        let (c1, c2) = (event.collider1, event.collider2);
        let (a, b) = (
            object_of(c1, event.body1, &identities),
            object_of(c2, event.body2, &identities),
        );
        if a.is_world() && b.is_world() {
            continue;
        }
        // The deepest contact carries the point, normal and the
        // pre-solver approach speed — mass-independent severity.
        let Some((point, normal, severity)) = deepest_contact(&collisions, c1, c2) else {
            continue;
        };
        if severity < policy.min_severity {
            continue;
        }
        // The cooldown starts on a *reportable* contact: checking it
        // earlier would let a filtered-out touch consume the window and
        // silently discard a genuine re-impact inside it.
        if !filter.dedup.allow(c1, c2, tick) {
            continue;
        }
        // The "surface" is the passive participant's material — for a
        // vehicle hitting the world, the world's side. `traction`
        // reports the session's environment modifier, the same input
        // the tire path reads (F06-B).
        let surface_entity = if vehicles.contains(c1) && !vehicles.contains(c2) {
            c2
        } else {
            c1
        };
        let surface = SurfaceState {
            material: surfaces.get(surface_entity).copied().unwrap_or_default(),
            traction: conditions.traction,
        };
        candidates.push((event, a, b, point, normal, severity, surface));
    }

    // Bound the tick's emission: keep the most severe, count the rest.
    candidates.sort_by(|a, b| b.5.partial_cmp(&a.5).unwrap_or(std::cmp::Ordering::Equal));
    filter.dropped += candidates.len().saturating_sub(policy.max_per_tick) as u64;
    for (event, a, b, point, normal, severity, surface) in
        candidates.into_iter().take(policy.max_per_tick)
    {
        // Impact severity feeds each participant's damage signal — the
        // raw input F05's damage model will consume. Static geometry
        // carries no `DamageSignals` and is skipped.
        for entity in [
            event.body1.unwrap_or(event.collider1),
            event.body2.unwrap_or(event.collider2),
        ] {
            if let Ok(mut d) = damage.get_mut(entity) {
                d.impact_total += severity;
                d.impact_count += 1;
            }
        }
        filter.next += 1;
        filter.emitted += 1;
        writer.write(ImpactEvent {
            id: ImpactId(filter.next),
            generation,
            tick,
            participants: (a, b),
            point,
            normal,
            severity,
            surface,
        });
    }
}

/// Fixed-step: publish the read-only [`VehicleTelemetry`] snapshot for
/// every simulated vehicle. Runs only while `Playing` — a paused session
/// freezes its clock, so its snapshots freeze too.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn publish_vehicle_telemetry(
    session: Res<Session>,
    mut commands: Commands,
    vehicles: Query<(
        Entity,
        &ObjectIdentity,
        &VehicleState,
        &Position,
        &Rotation,
        &LinearVelocity,
        &AngularVelocity,
        Option<&DamageSignals>,
        Option<&AuthorityRole>,
    )>,
    surfaces: Query<&SurfaceMaterial>,
    conditions: Res<TireConditions>,
) {
    if !session.is_playing() {
        return;
    }
    for (entity, identity, state, pos, rot, linvel, angvel, damage, role) in &vehicles {
        let wheels = state
            .wheels
            .iter()
            .map(|w| WheelTelemetry {
                grounded: w.grounded,
                contact_point: w.contact_point,
                contact_normal: w.contact_normal,
                slip_angle: w.slip_angle,
                traction_demand: w.traction_demand,
                surface: SurfaceState {
                    material: w
                        .contact_entity
                        .and_then(|e| surfaces.get(e).ok())
                        .copied()
                        .unwrap_or_default(),
                    // The session's environment modifier — the same
                    // value the tire path multiplied into this wheel's
                    // limits (F06-B).
                    traction: conditions.traction,
                },
            })
            .collect();
        commands.entity(entity).insert(VehicleTelemetry {
            object: identity.0,
            tick: session.tick(),
            authority: role.copied().unwrap_or_else(|| session.authority_role()),
            position: pos.0,
            rotation: rot.0,
            linear_velocity: linvel.0,
            angular_velocity: angvel.0,
            forward_speed: state.forward_speed,
            rpm: state.rpm,
            engine_load: state.engine_load,
            gear: state.gear,
            reverse: matches!(
                state.direction,
                mm2_vehicle::vehicle::DriveDirection::Reverse
            ),
            shifting: state.shifting > 0.0,
            wheels,
            damage: damage.copied().unwrap_or_default(),
        });
    }
}
