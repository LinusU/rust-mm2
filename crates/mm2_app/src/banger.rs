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
//! A placement whose PKG carries authored `BREAK<NN>` chunks (attached
//! to the entity as [`BangerPieces`]) does not tip over on activation:
//! it goes `Broken` — the unified collider and mesh are replaced by
//! one fragment body per collidable piece, each spawned directly in
//! the `Active` phase with its own `<name>_break<NN>` record (the
//! parent's def when the piece has none). Fragments are ordinary
//! actives from then on: they count against the pool and settle
//! through [`settle_bangers`]. One logical break event fires for the
//! placement itself (F04-AC03). Fragment spawn timing is provisional
//! (UNK-22): this slice breaks on the activation edge.
//!
//! Both systems are authority-gated like the race driver: a
//! `Predicted` session never transitions banger state — replication
//! (F26) delivers authoritative `BangerStateChanged` instead.

use std::collections::{HashMap, HashSet};

use avian3d::prelude::*;
use bevy::prelude::*;
use mm2_assets::Vfs;
use mm2_formats::banger::BangerData;
use mm2_game::{
    AuthorityRole, Banger, BangerCause, BangerDefinition, BangerPhase, BangerPool,
    BangerStateChanged, CityEntity, ObjectId, ObjectIdentity, Session, SessionEntity,
};
use mm2_vehicle::StrikeBound;
use tracing::{debug, warn};

use crate::contracts::{deepest_contact, impulse_estimate};

/// One authored `BREAK<NN>` piece of a breakable prop — the parts the
/// break transition turns into a fragment body: the chunk's render
/// parts, a convex collider and its runtime parameters.
#[derive(Clone)]
pub struct FragmentPiece {
    /// The authored BREAK index digits (e.g. `01`); the piece's own
    /// record stem is `<prop>_break<index>`.
    pub index: String,
    /// The piece's own `<prop>_break<index>` record, or the parent
    /// prop's distilled def when the piece has no record — a
    /// documented provisional fallback (254 retail fragment records
    /// exist; pieces without one are rare authored gaps).
    pub def: BangerDefinition,
    /// Best-LOD render parts of the piece, shared handles out of the
    /// prop cache.
    pub parts: Vec<(Handle<Mesh>, Handle<StandardMaterial>)>,
    /// Convex-hull collider over the piece's triangles (dynamic
    /// fragments get convex collision, not the prop's trimesh — small
    /// debris doesn't need authored concavity). `None` means the piece
    /// never spawns as a body.
    pub collider: Option<Collider>,
}

/// The authored break pieces of a stamped placement (F04-B) — present
/// only when the prop's PKG carries `BREAK<NN>` chunks. On activation
/// the placement goes [`BangerPhase::Broken`] and its pieces spawn as
/// fragment bodies; a placement without pieces tips over instead.
#[derive(Component)]
pub struct BangerPieces {
    /// Pieces in PKG chunk order.
    pub fragments: Vec<FragmentPiece>,
}

impl BangerPieces {
    /// Pieces that can spawn as bodies — a fragment without usable
    /// collision is never a physical participant.
    pub fn collidable(&self) -> impl Iterator<Item = &FragmentPiece> {
        self.fragments.iter().filter(|f| f.collider.is_some())
    }
}

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

/// The authored `CG` re-expressed in the mirrored world frame —
/// prop-local, before the placement's own rotation. `CG` is the bound
/// box's centre (measured on retail: `cg.y = size.y/2`, so the bound
/// rests its base on the instance origin): it is both the body's
/// centre of mass and the offset centred PKG geometry needs to sit
/// inside the bound — `city` stamping bakes the same vector into the
/// prop's vertices.
pub(crate) fn mirrored_cg(def: &BangerDefinition) -> Vec3 {
    Vec3::new(
        def.cg[0],
        def.cg[1],
        if crate::city::MIRROR_Z {
            -def.cg[2]
        } else {
            def.cg[2]
        },
    )
}

/// The entity-level bundle of one bound placement (or break
/// fragment): collider, authored physicals (inert while static), the
/// given [`Banger`] state and the contract stamps — shared by city
/// stamping, fragment spawning and the test harness so all exercise
/// the same spawn shape. Render parts are children the caller adds
/// under this root.
#[allow(clippy::too_many_arguments)]
pub fn banger_bundle(
    banger: Banger,
    object: ObjectId,
    role: AuthorityRole,
    owner: SessionEntity,
    collider: Collider,
    transform: Transform,
    name: String,
) -> impl Bundle {
    let def = banger.def.clone();
    (
        CityEntity,
        owner,
        ObjectIdentity(object),
        role,
        banger,
        RigidBody::Static,
        collider,
        // Authored physicals ride the dormant collider already: they
        // shape resting contact the same way and the activation only
        // has to flip the body kind.
        Mass(def.mass),
        CenterOfMass(mirrored_cg(&def)),
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
/// `Commands`, so it is not part of this query. `pub(crate)` for the
/// breakaway pipeline, which claims pool slots through the same
/// `claim_slot` path.
pub(crate) type BangerMut = (
    Entity,
    &'static ObjectIdentity,
    &'static mut Banger,
    &'static Position,
    &'static Rotation,
    &'static mut LinearVelocity,
    &'static mut AngularVelocity,
);

/// The read-only pieces a bound-strike query needs: the striker's
/// `StrikeBound` shape plus the pose/velocity the overlap test reads.
type StrikerRef = (
    Entity,
    &'static StrikeBound,
    &'static Position,
    &'static Rotation,
    &'static LinearVelocity,
    &'static AngularVelocity,
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

/// Claim one active-pool slot for a transition or fragment spawn.
/// `occupied` tracks `Active` bangers in the query plus bodies spawned
/// this tick (they only land in the world on the next flush). At
/// capacity the oldest `Active` settles in place first (reclaim order
/// provisional — R4 recovers the pool size, not the order). Returns
/// `false` when the pool is full and nothing is reclaimable — e.g.
/// `max_active = 0`, or every occupied slot is a pending spawn — so a
/// degenerate cap can never be exceeded.
#[allow(clippy::too_many_arguments)]
pub(crate) fn claim_slot(
    occupied: &mut usize,
    tick: u64,
    generation: u64,
    pool: &BangerPool,
    bangers: &mut Query<BangerMut>,
    writer: &mut MessageWriter<BangerStateChanged>,
    commands: &mut Commands,
) -> bool {
    if *occupied >= pool.max_active {
        let oldest = bangers
            .iter()
            .filter(|(_, _, b, _, _, _, _)| b.phase == BangerPhase::Active)
            .min_by_key(|(_, id, b, _, _, _, _)| (b.activated.unwrap_or(u64::MAX), id.0.slot))
            .map(|(e, _, _, _, _, _, _)| e);
        match oldest {
            Some(oldest) => {
                settle(
                    oldest,
                    tick,
                    generation,
                    BangerCause::Reclaimed,
                    bangers,
                    writer,
                    commands,
                );
                *occupied -= 1;
            }
            None => return false,
        }
    }
    *occupied += 1;
    true
}

/// The break transition (F04-B): the placement's unified collider and
/// mesh children are replaced by one fragment body per collidable
/// [`BangerPieces`] piece — each spawned already `Active` at the
/// parent's pose with the impact's velocity and its own spin kick. One
/// logical [`BangerStateChanged`] fires for the placement (`Broken`);
/// the pieces then live inside the shared pool like any active.
#[allow(clippy::too_many_arguments)]
fn break_banger(
    a: &Activation,
    pieces: &BangerPieces,
    owner: SessionEntity,
    parent_gt: &GlobalTransform,
    session: &mut Session,
    occupied: &mut usize,
    pool: &BangerPool,
    bangers: &mut Query<BangerMut>,
    writer: &mut MessageWriter<BangerStateChanged>,
    commands: &mut Commands,
) {
    let entity = a.entity;
    let tick = session.tick();
    let generation = session.generation();
    let role = session.authority_role();
    let parent_pos = parent_gt.translation();
    let parent_rot = parent_gt.rotation();

    // The placement keeps its entity/identity — only the collider and
    // the unified mesh are replaced by the spawned pieces.
    let parent_name = banger_name(bangers, entity);
    if let Ok((_, _, mut banger, _, _, mut linvel, mut angvel)) = bangers.get_mut(entity) {
        banger.phase = BangerPhase::Broken;
        banger.activated = None;
        linvel.0 = Vec3::ZERO;
        angvel.0 = Vec3::ZERO;
    }
    commands
        .entity(entity)
        .remove::<(Collider, RigidBody, CollisionEventsEnabled, Sleeping)>()
        .despawn_related::<Children>();
    writer.write(BangerStateChanged {
        object: a.object,
        generation,
        tick,
        phase: BangerPhase::Broken,
        cause: BangerCause::Impact {
            severity: a.severity,
            estimate: a.estimate,
        },
    });

    let base_vel = a.dir * a.severity;
    for piece in pieces.collidable() {
        if !claim_slot(occupied, tick, generation, pool, bangers, writer, commands) {
            debug!(
                piece = %piece.index,
                "banger piece skipped: active pool full"
            );
            continue;
        }
        let object = session.mint_object_id();
        // Each piece spins off its own lever arm: the contact point
        // relative to the piece's authored CG in world space.
        let lever = a.point - (parent_pos + parent_rot * mirrored_cg(&piece.def));
        let ang = piece
            .def
            .angular_kick(lever, a.dir * a.severity * piece.def.mass);
        let transform = Transform::from_translation(parent_pos).with_rotation(parent_rot);
        let fragment = commands
            .spawn(banger_bundle(
                Banger {
                    phase: BangerPhase::Active,
                    def: piece.def.clone(),
                    activated: Some(tick),
                },
                object,
                role,
                owner,
                piece.collider.clone().unwrap(),
                transform,
                format!("{parent_name}-break{}", piece.index),
            ))
            .insert((
                RigidBody::Dynamic,
                LinearVelocity(base_vel),
                AngularVelocity(ang),
            ))
            .id();
        for (mesh, material) in &piece.parts {
            let part = commands
                .spawn((
                    Mesh3d(mesh.clone()),
                    MeshMaterial3d(material.clone()),
                    Transform::IDENTITY,
                ))
                .id();
            commands.entity(fragment).add_child(part);
        }
    }
    debug!(
        severity = a.severity,
        estimate = a.estimate,
        "banger shattered"
    );
}

/// The prop name a banger entity's def carries — for fragment names.
fn banger_name(bangers: &Query<BangerMut>, entity: Entity) -> String {
    bangers
        .get(entity)
        .map(|(_, _, b, _, _, _, _)| b.def.name.clone())
        .unwrap_or_default()
}

/// Fixed-step dormant → active transition, driven by `CollisionStart`
/// edges. A banger activates at most once — the `Dormant` check is the
/// dedup: later edges against an `Active`/`Settled` prop are ordinary
/// contacts the solver owns (AC02). A placement carrying
/// [`BangerPieces`] goes [`BangerPhase::Broken`] instead — its pieces
/// become the active bodies.
///
/// Contacts are not the only strike source: a moving vehicle also
/// activates any dormant banger its [`StrikeBound`] overlaps — the
/// *unmodified* authored bound, the shape the original bound-vs-bound
/// prop test used. The snag-safe chassis hull's raised floor can ride
/// over kerb-height props (a bus over a cone) that bound would have
/// touched; the overlap restores those strikes without re-introducing
/// the road snags the reshape removed. The overlap carries no
/// manifold: approach speed is the bound's surface velocity at the
/// prop's centre, and the "contact point" is the prop's upwind face —
/// the lever the spin kick needs.
// Threads the same query the contact path mutates plus the read-only
// striker/spatial-query params — the signature stays flat because the
// decision list and pool accounting below must see one merged set.
#[allow(clippy::too_many_arguments)]
pub fn activate_bangers(
    mut reader: MessageReader<CollisionStart>,
    collisions: Collisions,
    spatial: SpatialQuery,
    strikers: Query<StrikerRef, Without<Banger>>,
    mut session: ResMut<Session>,
    pool: Res<BangerPool>,
    mut bangers: Query<BangerMut>,
    pieces: Query<(&BangerPieces, &SessionEntity, &GlobalTransform)>,
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
            let Ok((_, identity, banger, _, _, _, _)) = bangers.get(collider) else {
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

    // Authored-bound strikes: any dormant banger a striker's
    // `StrikeBound` overlaps this tick is a candidate, deduplicated
    // against the contact activations above and against itself (two
    // strikers can overlap one prop). A below-threshold overlap leaves
    // the prop dormant and — since the bound is not a world collider —
    // the vehicle passes through it visually; the authored limits make
    // that a corner case (a cone's 8 500 vs a bus's ~5 000 kg striker
    // fires at walking pace).
    let mut claimed: HashSet<Entity> = activations.iter().map(|a| a.entity).collect();
    for (entity, bound, position, rotation, linvel, angvel) in &strikers {
        if linvel.0 == Vec3::ZERO && angvel.0 == Vec3::ZERO {
            continue;
        }
        let filter = SpatialQueryFilter::from_excluded_entities([entity]);
        for hit in spatial.shape_intersections(&bound.0, position.0, rotation.0, &filter) {
            let Ok((_, identity, banger, bpos, brot, _, _)) = bangers.get(hit) else {
                continue;
            };
            if banger.phase != BangerPhase::Dormant || claimed.contains(&hit) {
                continue;
            }
            // Surface velocity of the bound at the prop's centre — the
            // authored `CG` (the bound's centre, verified on retail:
            // `cg.y = size.y/2`), not the bound-base origin.
            let centre = bpos.0 + brot.0 * mirrored_cg(&banger.def);
            let surface = linvel.0 + angvel.0.cross(centre - position.0);
            let severity = surface.length();
            if severity <= 0.0 {
                continue;
            }
            let estimate = impulse_estimate(entity, severity, &masses);
            if !banger.def.activates_on(estimate) {
                continue;
            }
            claimed.insert(hit);
            let dir = surface.try_normalize().unwrap_or(Vec3::X);
            // `Size` is the bound's full extents (verified): half its
            // largest axis reaches from the bound's centre to a face.
            let reach = banger.def.size.iter().fold(0.0f32, |m, h| m.max(h.abs())) * 0.5;
            activations.push(Activation {
                entity: hit,
                object: identity.0,
                severity,
                estimate,
                dir,
                point: centre - dir * reach,
            });
        }
    }

    // Pool occupancy counts `Active` bangers in the query plus the
    // fragment bodies this tick spawns — those only land in the world
    // on the next flush, so they must be accounted as pending.
    let mut occupied = bangers
        .iter()
        .filter(|(_, _, b, _, _, _, _)| b.phase == BangerPhase::Active)
        .count();

    for a in activations {
        // A placement that carries break pieces shatters; everything
        // else takes the ordinary dormant → active transition. The
        // `Dormant` re-check still holds: a reclaim cannot touch a
        // dormant prop, but a `CollisionStart` pair can name the same
        // entity twice.
        if bangers
            .get(a.entity)
            .map(|(_, _, b, _, _, _, _)| b.phase != BangerPhase::Dormant)
            .unwrap_or(true)
        {
            continue;
        }
        match pieces.get(a.entity) {
            Ok((pieces, owner, gt)) if pieces.collidable().next().is_some() => {
                break_banger(
                    &a,
                    pieces,
                    *owner,
                    gt,
                    &mut session,
                    &mut occupied,
                    &pool,
                    &mut bangers,
                    &mut writer,
                    &mut commands,
                );
                continue;
            }
            _ => {}
        }

        if !claim_slot(
            &mut occupied,
            tick,
            generation,
            &pool,
            &mut bangers,
            &mut writer,
            &mut commands,
        ) {
            // The pool is full of pending spawns (or capped at zero):
            // the prop stays dormant rather than exceed the bound.
            continue;
        }
        let Ok((_, _, mut banger, position, rotation, mut linvel, mut angvel)) =
            bangers.get_mut(a.entity)
        else {
            continue;
        };
        // The reclaim above may have settled this entity already.
        if banger.phase != BangerPhase::Dormant {
            occupied -= 1;
            continue;
        }
        // One impulse, one transition: the body goes dynamic and
        // leaves at the striker's approach speed (bounded by the
        // impact, not scaled by it), plus the record's spin kick —
        // the lever runs from the body's centre of mass (the authored
        // `CG`, the bound's centre), not the bound-base origin.
        let impulse = a.dir * a.severity * banger.def.mass;
        linvel.0 += a.dir * a.severity;
        angvel.0 += banger.def.angular_kick(
            a.point - (position.0 + rotation.0 * mirrored_cg(&banger.def)),
            impulse,
        );
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
    let Ok((_, identity, mut banger, _, _, mut linvel, mut angvel)) = bangers.get_mut(entity)
    else {
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
        .filter(|(_, _, b, _, _, _, _)| b.phase == BangerPhase::Active)
        .map(|(e, _, _, _, _, _, _)| e)
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
