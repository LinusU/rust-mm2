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
    AuthorityRole, DamageSignals, ImpactDedup, ImpactEvent, ImpactId, ImpactPolicy,
    MAX_BANGER_ANGULAR_SPEED, ObjectId, ObjectIdentity, Session, SurfaceMaterial, SurfaceState,
    VehicleTelemetry, WheelTelemetry,
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

/// The deepest contact of a live pair plus the solve data the banger
/// transfer needs: the manifold normal (pointing from `c1` toward
/// `c2`), the pre-solver approach speed (m/s, ≥ 0 while approaching),
/// the combined restitution the solver used and the normal impulse it
/// actually applied across the pair this step. The impact pipeline and
/// the banger activation walk the same manifold data so "the hit"
/// means the same thing to both.
///
/// All directional fields are expressed in the caller's `(c1, c2)`
/// order, not the stored pair's: the contact graph keys edges in
/// broad-phase order, so `pair.collider1` is not guaranteed to be the
/// `c1` argument.
pub(crate) struct ContactDetails {
    /// World-space contact point of the deepest manifold point.
    pub point: Vec3,
    /// The deepest manifold's normal, `c1` → `c2`.
    pub normal: Vec3,
    /// Pre-solver approach speed along the normal (m/s, ≥ 0).
    pub severity: f32,
    /// The combined restitution the deepest manifold was solved with —
    /// the pair's [`Restitution`] coefficients already combined.
    pub restitution: f32,
    /// Total normal impulse the solver applied across the pair's
    /// manifolds this step (kg·m/s) — the response a dormant prop
    /// already delivered as a static body.
    pub applied_impulse: f32,
    /// The deepest point's world lever on `c1`'s body (anchor relative
    /// to its centre of mass).
    pub anchor1: Vec3,
    /// The deepest point's world lever on `c2`'s body.
    pub anchor2: Vec3,
}

/// The deepest contact of a live pair, or `None` when the pair has no
/// live manifold. See [`ContactDetails`] for what it carries.
pub(crate) fn deepest_contact(
    collisions: &Collisions,
    c1: Entity,
    c2: Entity,
) -> Option<ContactDetails> {
    let pair = collisions.get(c1, c2)?;
    let applied_impulse: f32 = pair
        .manifolds
        .iter()
        .flat_map(|m| m.points.iter())
        .map(|p| p.normal_impulse)
        .sum();
    let (contact, manifold) = pair
        .manifolds
        .iter()
        .filter_map(|m| m.find_deepest_contact().map(|c| (c, m)))
        .max_by(|a, b| {
            a.0.penetration
                .partial_cmp(&b.0.penetration)
                .unwrap_or(std::cmp::Ordering::Equal)
        })?;
    // Re-express the directional fields in argument order. `normal`
    // points `collider1` → `collider2` and `anchor1`/`anchor2` ride on
    // those bodies; `normal_speed` needs no flip — closing speed is
    // symmetric (`(v2 − v1)·n` is invariant under swapping both).
    let flipped = pair.collider1 == c2 && pair.collider2 == c1;
    Some(ContactDetails {
        point: contact.point,
        normal: if flipped {
            -manifold.normal
        } else {
            manifold.normal
        },
        severity: (-contact.normal_speed).max(0.0),
        restitution: manifold.restitution,
        applied_impulse,
        anchor1: if flipped {
            contact.anchor2
        } else {
            contact.anchor1
        },
        anchor2: if flipped {
            contact.anchor1
        } else {
            contact.anchor2
        },
    })
}

/// The striker's resolved mass for an impact estimate: the authored
/// `ComputedMass`, else a 1 kg fallback — a light touch, not a hidden
/// force.
fn striker_mass(striker: Entity, masses: &Query<&ComputedMass>) -> f32 {
    masses
        .get(striker)
        .map(|m| m.value())
        .ok()
        .filter(|m| m.is_finite() && *m > 0.0)
        .unwrap_or(1.0)
}

/// Estimated impulse of a contact on a struck entity (kg·m/s):
/// approach speed × the striker's mass — the quantity the ambient
/// handover compares to the designed [`mm2_game::KnockPolicy`]
/// threshold.
pub(crate) fn impulse_estimate(
    striker: Entity,
    severity: f32,
    masses: &Query<&ComputedMass>,
) -> f32 {
    severity * striker_mass(striker, masses)
}

/// Kinetic energy the striker carries into a contact (joules):
/// ½·m·v² — the provisional quantity banger activation compares to
/// the authored `ImpulseLimit2` (UNK-22). A linear `m·v` estimate
/// misreads the authored data: `ImpulseLimit2 ≈ Mass × 800` on nearly
/// every retail prop (the exceptions scale the same way), which puts
/// authored-breakable trees and poles 5–50× beyond any reachable
/// impact (operator report 4 item 3). The speed-squared reading
/// restores the authored ladder — meters and cones break at a crawl,
/// poles and trees at speed, the authored-immovable records stay
/// unreachable (docs/research/banger.md).
pub(crate) fn impact_energy(striker: Entity, severity: f32, masses: &Query<&ComputedMass>) -> f32 {
    0.5 * striker_mass(striker, masses) * severity * severity
}

// ---------- post-solver momentum transfer ----------

/// The momentum-conserving split of one committed post-solver hit.
/// Both consumers — banger activation and the ambient-traffic
/// handover — run after the solver has already answered the contact
/// against a struck body that was static or kinematic during the
/// step, so the solver's own response is not the exchange this
/// carries: it is the `(1+e)·v·μ` impulse the solver *would* have
/// produced with a dynamic target, computed from the struck body's
/// authored mass and the striker's computed mass — no free tuning
/// constants.
pub(crate) struct Transfer {
    /// kg·m/s transferred striker → struck along the push direction.
    pub impulse: f32,
    /// The struck body's launch speed (`impulse / struck_mass`).
    pub launch: f32,
    /// The striker's resolved mass, kept for the correction divide.
    pub striker_mass: f32,
}

/// The two-body transfer for one committed hit, or `None` when the
/// striker's mass cannot be resolved (a static world sliver, an
/// unstamped body) — the caller then keeps the pre-transfer launch at
/// approach speed and skips the striker correction.
pub(crate) fn resolve_transfer(
    striker: Option<Entity>,
    severity: f32,
    restitution: f32,
    struck_mass: f32,
    masses: &Query<&ComputedMass>,
) -> Option<Transfer> {
    let striker = striker?;
    let m_s = masses
        .get(striker)
        .ok()
        .map(|m| m.value())
        .filter(|m| m.is_finite() && *m > 0.0)?;
    let m_p = struck_mass.max(0.001);
    let reduced = m_s * m_p / (m_s + m_p);
    let impulse = (1.0 + restitution.max(0.0)) * severity * reduced;
    Some(Transfer {
        impulse,
        launch: impulse / m_p,
        striker_mass: m_s,
    })
}

/// The mutable pieces a striker correction touches. The callsite's
/// `Without<…>` filter keeps it disjoint from the struck body's own
/// query (a dormant banger striking another dormant banger keeps the
/// solver's response unchanged; a lane-following striker's velocity
/// is owned by the ambient driver, not the correction).
pub(crate) type StruckMut = (
    &'static mut LinearVelocity,
    Option<&'static mut AngularVelocity>,
    Option<&'static ComputedAngularInertia>,
    Option<&'static Rotation>,
);

/// The Δω an impulse `impulse` applied at world lever `lever` gives a
/// body whose angular inertia resolves — `Vec3::ZERO` when it does
/// not. The contact-lever share, no tuning constants.
pub(crate) fn angular_share(
    lever: Vec3,
    impulse: Vec3,
    inertia: Option<&ComputedAngularInertia>,
    rotation: Option<&Rotation>,
) -> Vec3 {
    match (inertia, rotation) {
        (Some(cai), Some(rot)) => cai.rotated(rot.0).inverse().mul_vec3(lever.cross(impulse)),
        _ => Vec3::ZERO,
    }
}

/// The correction Δp a committed transfer gives the striker: return
/// the wall impulse the solver charged it (`applied_dir` ×
/// `applied_impulse` — zero when the pair never solved this step)
/// and charge the real exchange impulse along `dir` instead. The
/// striker therefore always pays exactly `impulse` along the push
/// direction, while a glancing touch's lateral wall response is
/// returned rather than converted into `dir`.
pub(crate) fn striker_correction(
    applied_dir: Vec3,
    applied_impulse: f32,
    dir: Vec3,
    impulse: f32,
) -> Vec3 {
    applied_dir * applied_impulse - dir * impulse
}

/// Write a correction Δp onto a resolved striker: linear Δv = Δp/m
/// plus the contact-lever angular share when inertia resolves. The
/// post-write spin clamps at `MAX_BANGER_ANGULAR_SPEED` — the same
/// bound `banger_bundle`/`spawn_ambient_car` stamp solver-side — so a
/// transient spike cannot leave the striker spinning fast enough to
/// inflate a later contact's `normal_speed` reading. A striker
/// without the solver bound (a `Player` vehicle carries none) gets
/// its only clamp here.
pub(crate) fn write_striker_correction(
    delta_p: Vec3,
    striker_mass: f32,
    lever: Vec3,
    mut linvel: Mut<LinearVelocity>,
    angvel: Option<Mut<AngularVelocity>>,
    inertia: Option<&ComputedAngularInertia>,
    rotation: Option<&Rotation>,
) {
    linvel.0 += delta_p / striker_mass;
    if let Some(mut angvel) = angvel {
        angvel.0 += angular_share(lever, delta_p, inertia, rotation);
        angvel.0 = angvel.0.clamp_length_max(MAX_BANGER_ANGULAR_SPEED);
    }
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
        let Some(deepest) = deepest_contact(&collisions, c1, c2) else {
            continue;
        };
        let (point, normal, severity) = (deepest.point, deepest.normal, deepest.severity);
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

#[cfg(test)]
mod tests {
    //! F10-B.14: the shared striker correction bounds its spin write
    //! write-side, not only solver-side — a striker without a stamped
    //! `MaxAngularSpeed` (a `Player` vehicle carries none) gets its
    //! only clamp here.

    use super::*;

    /// One striker entity with a unit angular inertia — the query
    /// tuple is exactly the [`StruckMut`] shape the callsites hand
    /// over.
    fn striker(world: &mut World, spin: Vec3) -> Entity {
        world
            .spawn((
                LinearVelocity::ZERO,
                AngularVelocity(spin),
                ComputedAngularInertia::new(Vec3::ONE),
                Rotation(Quat::IDENTITY),
            ))
            .id()
    }

    #[test]
    fn a_huge_correction_share_clamps_at_the_banger_bound() {
        let mut world = World::new();
        let striker = striker(&mut world, Vec3::ZERO);
        let mut q = world.query::<(
            &mut LinearVelocity,
            &mut AngularVelocity,
            &ComputedAngularInertia,
            &Rotation,
        )>();
        let (linvel, angvel, inertia, rotation) =
            q.get_mut(&mut world, striker).expect("the striker spawned");
        // lever x̂ × Δp ŷ·1e6 at a unit inertia → Δω ẑ·1e6 rad/s —
        // far over any physical spin.
        write_striker_correction(
            Vec3::Y * 1.0e6,
            1.0,
            Vec3::X,
            linvel,
            Some(angvel),
            Some(inertia),
            Some(rotation),
        );
        let angvel = world
            .get::<AngularVelocity>(striker)
            .expect("the striker despawned");
        assert!(
            (angvel.0.length() - MAX_BANGER_ANGULAR_SPEED).abs() < 1e-3,
            "the write left an unbounded spin: {:?}",
            angvel.0
        );
        assert!(
            angvel.0.z > 0.0,
            "the clamp lost the share's direction: {:?}",
            angvel.0
        );
        // The angular bound does not touch the linear leg.
        assert_eq!(
            world.get::<LinearVelocity>(striker).unwrap().0,
            Vec3::Y * 1.0e6
        );
    }

    #[test]
    fn an_under_bound_share_lands_verbatim_and_counts_the_prior_spin() {
        let mut world = World::new();
        let calm = striker(&mut world, Vec3::ZERO);
        // Already spinning at 55 rad/s: the same 30 rad/s share would
        // total 85 unclamped — the bound covers the post-write total,
        // matching what solver-side `MaxAngularSpeed` enforces.
        let wound = striker(&mut world, Vec3::Z * 55.0);
        let mut q = world.query::<(
            &mut LinearVelocity,
            &mut AngularVelocity,
            &ComputedAngularInertia,
            &Rotation,
        )>();
        for (entity, expected) in [(calm, 30.0f32), (wound, MAX_BANGER_ANGULAR_SPEED)] {
            let (linvel, angvel, inertia, rotation) =
                q.get_mut(&mut world, entity).expect("the striker spawned");
            // lever x̂ × Δp ŷ·30 at a unit inertia → Δω ẑ·30 rad/s.
            write_striker_correction(
                Vec3::Y * 30.0,
                1.0,
                Vec3::X,
                linvel,
                Some(angvel),
                Some(inertia),
                Some(rotation),
            );
            let spin = world.get::<AngularVelocity>(entity).unwrap().0;
            assert!(
                (spin.z - expected).abs() < 1e-3 && spin.xy().length() < 1e-3,
                "spin {spin:?} is not the expected {expected} rad/s along z"
            );
        }
    }
}
