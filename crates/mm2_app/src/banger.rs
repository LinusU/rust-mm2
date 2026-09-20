//! Banger runtime (F04-A): dormant → active → settled for stamped
//! world props with a bound `tune/banger/*.dgbangerdata` record.
//!
//! Stamping (in `city`) resolves each pathset prop name through
//! [`BangerDefs`]; a bound name spawns the placement as one entity —
//! the prop's collider, render children and [`Banger`] state on a
//! single session-owned root — instead of the loose render/collider
//! pair an unbound prop gets.
//!
//! [`activate_bangers`] is the dormant → active transition: it reads
//! the same `CollisionStart` edges the impact pipeline consumes (a
//! second, independent consumer — bangers need every approaching
//! contact, not only the deduplicated reportable ones), measures the
//! approach speed on the deepest manifold contact, estimates the
//! impulse against the striker's mass and compares it to the authored
//! `ImpulseLimit2` (provisional rule — the compared quantity is
//! UNK-22). A qualifying edge flips the prop's `RigidBody` to dynamic
//! once and applies one impulse plus the definition's spin kick.
//! `BangerPool` bounds simultaneously active props at the recovered
//! ×32, reclaiming oldest-first (reclaim order provisional).
//!
//! [`settle_bangers`] is the active → settled transition: a dynamic
//! banger that Avian puts to sleep turns back into a static collider
//! at its rest pose — the `dgHitBangerInstance` state. `Settled` is
//! terminal for the session; teardown restamps dormant placements.
//!
//! Both systems are authority-gated like the race driver: a
//! `Predicted` session never transitions banger state — replication
//! (F26) delivers authoritative `BangerStateChanged` instead.

use std::collections::HashMap;

use avian3d::prelude::*;
use bevy::prelude::*;
use mm2_assets::Vfs;
use mm2_formats::banger::BangerData;
use mm2_game::{
    AuthorityRole, Banger, BangerCause, BangerDefinition, BangerPhase, BangerPool,
    BangerStateChanged, CityEntity, ObjectId, ObjectIdentity, Session, SessionEntity,
};
use tracing::{debug, warn};

use crate::contracts::deepest_contact;

/// Cache of `tune/banger/<name>.dgbangerdata` → distilled definition,
/// filled as stamping meets each prop name. Mirrors `PropCache`'s
/// role for geometry: `None` = the name binds no record (an ordinary
/// static prop); a record that exists but fails to decode counts in
/// `failed` once per name and stamps unbound — reported, not hidden.
pub struct BangerDefs<'a> {
    vfs: &'a Vfs,
    defs: HashMap<String, Option<BangerDefinition>>,
    /// Distinct names whose record resolved but failed to decode.
    pub failed: usize,
}

impl<'a> BangerDefs<'a> {
    pub fn new(vfs: &'a Vfs) -> Self {
        Self {
            vfs,
            defs: HashMap::new(),
            failed: 0,
        }
    }

    /// The definition a stamped prop name binds, if any. The WLD-16
    /// rule is by name: `tune/banger/<name>.dgbangerdata` resolves.
    pub fn get(&mut self, name: &str) -> Option<&BangerDefinition> {
        let key = name.to_ascii_lowercase();
        if !self.defs.contains_key(&key) {
            let def = self.load(&key);
            self.defs.insert(key.clone(), def);
        }
        self.defs.get(&key).and_then(|d| d.as_ref())
    }

    fn load(&mut self, name: &str) -> Option<BangerDefinition> {
        let (bytes, resolved) = self
            .vfs
            .read_path(&format!("tune/banger/{name}.dgbangerdata"))
            .ok()?;
        match std::str::from_utf8(&bytes)
            .ok()
            .and_then(|t| BangerData::parse(t).ok())
        {
            Some(data) => {
                for issue in data.validate() {
                    debug!(record = %resolved.logical, %issue, "banger record authored issue");
                }
                Some(BangerDefinition::from_record(name, &data))
            }
            None => {
                self.failed += 1;
                warn!(record = %resolved.logical, "banger record failed to decode; stamping unbound");
                None
            }
        }
    }
}

/// The entity-level bundle of one bound placement: collider, authored
/// physicals (inert while static), the banger state machine and the
/// contract stamps — shared by city stamping and the test harness so
/// both exercise the same spawn shape. Render parts are children the
/// caller adds under this root.
#[allow(clippy::too_many_arguments)]
pub fn banger_bundle(
    def: &BangerDefinition,
    object: ObjectId,
    role: AuthorityRole,
    owner: SessionEntity,
    collider: Collider,
    transform: Transform,
    name: String,
) -> impl Bundle {
    (
        CityEntity,
        owner,
        ObjectIdentity(object),
        role,
        Banger::new(def.clone()),
        RigidBody::Static,
        collider,
        // Authored physicals ride the dormant collider already: they
        // shape resting contact the same way and the activation only
        // has to flip the body kind.
        Mass(def.mass),
        CenterOfMass(Vec3::new(
            def.cg[0],
            def.cg[1],
            if crate::city::MIRROR_Z {
                -def.cg[2]
            } else {
                def.cg[2]
            },
        )),
        Friction::new(def.friction),
        Restitution::new(def.elasticity),
        // The striker usually enables the pair's events (the vehicle
        // does), but banger-vs-banger and non-vehicle strikers need
        // the flag on this side too.
        CollisionEventsEnabled,
        transform,
        Visibility::Visible,
        Name::new(name),
    )
}

/// The mutable pieces the state machine touches. `RigidBody` is an
/// immutable component in Avian — transitions replace it through
/// `Commands`, so it is not part of this query.
type BangerMut = (
    Entity,
    &'static ObjectIdentity,
    &'static mut Banger,
    &'static Position,
    &'static mut LinearVelocity,
    &'static mut AngularVelocity,
);

/// One activation decision taken off a contact edge, before any
/// mutation: who fires, with what strength.
struct Activation {
    entity: Entity,
    object: ObjectId,
    severity: f32,
    estimate: f32,
    dir: Vec3,
    point: Vec3,
}

/// Estimated impulse of a contact on a dormant banger (kg·m/s):
/// approach speed × the striker's mass — the provisional quantity
/// `ImpulseLimit2` is compared against (UNK-22). A striker without a
/// resolvable mass counts as 1 kg — a light touch, not a hidden force.
fn impulse_estimate(striker: Entity, severity: f32, masses: &Query<&ComputedMass>) -> f32 {
    let mass = masses
        .get(striker)
        .map(|m| m.value())
        .ok()
        .filter(|m| m.is_finite() && *m > 0.0)
        .unwrap_or(1.0);
    severity * mass
}

/// Fixed-step dormant → active transition, driven by `CollisionStart`
/// edges. A banger activates at most once — the `Dormant` check is the
/// dedup: later edges against an `Active`/`Settled` prop are ordinary
/// contacts the solver owns (AC02).
#[allow(clippy::too_many_arguments)]
pub fn activate_bangers(
    mut reader: MessageReader<CollisionStart>,
    collisions: Collisions,
    session: Res<Session>,
    pool: Res<BangerPool>,
    mut bangers: Query<BangerMut>,
    masses: Query<&ComputedMass>,
    mut writer: MessageWriter<BangerStateChanged>,
    mut commands: Commands,
) {
    // Same drain discipline as collect_impacts: edges buffered outside
    // gameplay phases must not flush as a stale burst on resume — and
    // a Predicted session never transitions banger state (F26 owns it).
    if !session.is_playing() || !session.authority_role().is_authority() {
        reader.read().for_each(drop);
        return;
    }
    let tick = session.tick();
    let generation = session.generation();

    // Decide first, mutate second: the decision pass only reads.
    let mut activations: Vec<Activation> = Vec::new();
    for event in reader.read() {
        for (collider, striker, sign) in [
            (
                event.collider1,
                event.body2.unwrap_or(event.collider2),
                -1.0f32,
            ),
            (
                event.collider2,
                event.body1.unwrap_or(event.collider1),
                1.0f32,
            ),
        ] {
            let Ok((_, identity, banger, _, _, _)) = bangers.get(collider) else {
                continue;
            };
            if banger.phase != BangerPhase::Dormant {
                continue;
            }
            let Some((point, normal, severity)) =
                deepest_contact(&collisions, event.collider1, event.collider2)
            else {
                continue;
            };
            if severity <= 0.0 {
                continue;
            }
            let estimate = impulse_estimate(striker, severity, &masses);
            if !banger.def.activates_on(estimate) {
                continue;
            }
            activations.push(Activation {
                entity: collider,
                object: identity.0,
                severity,
                estimate,
                dir: normal * sign,
                point,
            });
        }
    }

    for a in activations {
        // Pool bound: at capacity the oldest activation settles in
        // place before the new one takes its slot (reclaim order
        // provisional — R4 recovers the pool size, not the order).
        if bangers
            .iter()
            .filter(|(_, _, b, _, _, _)| b.phase == BangerPhase::Active)
            .count()
            >= pool.max_active
        {
            let oldest = bangers
                .iter()
                .filter(|(_, _, b, _, _, _)| b.phase == BangerPhase::Active)
                .min_by_key(|(_, id, b, _, _, _)| (b.activated.unwrap_or(u64::MAX), id.0.slot))
                .map(|(e, _, _, _, _, _)| e);
            if let Some(oldest) = oldest {
                settle(
                    oldest,
                    tick,
                    generation,
                    BangerCause::Reclaimed,
                    &mut bangers,
                    &mut writer,
                    &mut commands,
                );
            }
        }

        let Ok((_, _, mut banger, position, mut linvel, mut angvel)) = bangers.get_mut(a.entity)
        else {
            continue;
        };
        // The reclaim above may have settled this entity already.
        if banger.phase != BangerPhase::Dormant {
            continue;
        }
        // One impulse, one transition: the body goes dynamic and
        // leaves at the striker's approach speed (bounded by the
        // impact, not scaled by it), plus the record's spin kick.
        let impulse = a.dir * a.severity * banger.def.mass;
        linvel.0 += a.dir * a.severity;
        angvel.0 += banger.def.angular_kick(a.point - position.0, impulse);
        banger.phase = BangerPhase::Active;
        banger.activated = Some(tick);
        commands.entity(a.entity).insert(RigidBody::Dynamic);
        writer.write(BangerStateChanged {
            object: a.object,
            generation,
            tick,
            phase: BangerPhase::Active,
            cause: BangerCause::Impact {
                severity: a.severity,
                estimate: a.estimate,
            },
        });
        debug!(
            prop = %banger.def.name,
            severity = a.severity,
            estimate = a.estimate,
            "banger activated"
        );
    }
}

/// Move one banger to `Settled` at its current pose: the body becomes
/// a static collider again (the `dgHitBangerInstance` state) and the
/// transition is emitted once.
fn settle<F: bevy::ecs::query::QueryFilter>(
    entity: Entity,
    tick: u64,
    generation: u64,
    cause: BangerCause,
    bangers: &mut Query<BangerMut, F>,
    writer: &mut MessageWriter<BangerStateChanged>,
    commands: &mut Commands,
) {
    let Ok((_, identity, mut banger, _, mut linvel, mut angvel)) = bangers.get_mut(entity) else {
        return;
    };
    banger.phase = BangerPhase::Settled;
    banger.activated = None;
    linvel.0 = Vec3::ZERO;
    angvel.0 = Vec3::ZERO;
    commands
        .entity(entity)
        .insert(RigidBody::Static)
        .remove::<Sleeping>();
    writer.write(BangerStateChanged {
        object: identity.0,
        generation,
        tick,
        phase: BangerPhase::Settled,
        cause,
    });
}

/// Fixed-step active → settled transition: a dynamic banger Avian has
/// put to sleep becomes static where it lies. Runs after
/// [`activate_bangers`] so a prop that activates and sleeps inside the
/// same step still settles.
#[allow(clippy::type_complexity)]
pub fn settle_bangers(
    session: Res<Session>,
    mut bangers: Query<BangerMut, With<Sleeping>>,
    mut writer: MessageWriter<BangerStateChanged>,
    mut commands: Commands,
) {
    if !session.is_playing() || !session.authority_role().is_authority() {
        return;
    }
    let tick = session.tick();
    let generation = session.generation();
    let sleepers: Vec<Entity> = bangers
        .iter()
        .filter(|(_, _, b, _, _, _)| b.phase == BangerPhase::Active)
        .map(|(e, _, _, _, _, _)| e)
        .collect();
    for entity in sleepers {
        settle(
            entity,
            tick,
            generation,
            BangerCause::Slept,
            &mut bangers,
            &mut writer,
            &mut commands,
        );
    }
}
