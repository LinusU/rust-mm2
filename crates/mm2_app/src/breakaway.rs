//! Vehicle breakaway detachment (F05-B.3) — where the authored
//! inventory meets Avian and the session lifecycle.
//!
//! [`detach_breaks`] consumes the same deduplicated [`ImpactEvent`]
//! stream the damage accumulator reads. Each attached part detaches
//! when the impact's approach speed delivers more than its authored
//! `ImpulseLimit2` measured against the part's own mass — the reading
//! the authored values are shaped for: `limit / mass` is ≈31.25 m/s
//! (~70 mph) on ordinary panels and ≈2500 m/s (never) on the heavy
//! rigs' anchors (DSN-21; what the original compares the limit against
//! stays UNK-22, and whether detachment is impact- or damage-driven
//! stays UNK-13). A detaching part's intact render node hides and a
//! fragment body spawns at the detached mesh's pose — convex hull
//! over the part's own verts, the record's physicals, the car's
//! velocity at the part plus the impact kick and spin, pooled like
//! prop debris through the shared [`BangerPool`]. One bounded
//! [`PartDetached`] fires per part per attachment.
//!
//! [`restore_rig`] is the repair side: the disabled outcome calls it
//! where it calls `damage.reset()`, so a repaired wreck gets its
//! panels back and the spawned fragments despawn (F05-AC03 — the part
//! appears once, cleanup is bounded, repair restores the rig). A
//! plain reset or stuck recovery does not repair, so detached parts
//! stay off.
//!
//! [`detach_breaks`] is authority-gated and drains while not
//! `Playing`, like every other impact consumer — but a remote seat on
//! the host is this authority's participant, so its rig sheds parts
//! like an AI's and the detached bitmask rides its `SnapEntry`
//! (protocol v11, F25-B). Predicted clients never run this system:
//! their copies' rigs reconcile off that bitmask instead
//! (`netdrive::apply_snapshots`), spawning the same pooled fragment
//! through [`spawn_break_fragment`].

use std::collections::HashMap;

use avian3d::prelude::*;
use bevy::prelude::*;
use mm2_content::model::ModelPart;
use mm2_game::{
    Banger, BangerPhase, BangerPool, BangerStateChanged, BreakPartSpec, ImpactEvent, ObjectId,
    ObjectIdentity, PartDetached, Session, SessionEntity, VehicleBreaks,
};

use crate::banger::{BangerMut, banger_bundle, claim_slot};

/// Per-session evidence counters for the breakaway pipeline — the
/// `brk=` field of the headless smoke record. Session-scoped like
/// [`crate::damage::DamageReport`]: `drive_session` resets it during
/// teardown.
#[derive(Resource, Debug, Default)]
pub struct BreakReport {
    /// Parts that left a rig this session.
    pub detached: u64,
    /// Parts a repair put back (their fragments despawned).
    pub restored: u64,
}

impl BreakReport {
    /// Forget all session-scoped counts — called on teardown, same
    /// contract as [`crate::contracts::ImpactFilter::reset`].
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// The render node of one intact `BREAK<NN>` part, tagged by
/// `car_visual` when the model carries a part the vehicle's authored
/// `breaks` inventory can detach. Detachment hides this node and
/// turns its data into the spawned fragment; repair shows it again.
#[derive(Component)]
pub struct BreakPartVisual {
    /// Model part stem (`break01`) — the key the rig's spec uses.
    pub part: String,
    /// This node's local transform under the vehicle root — the
    /// fragment's spawn pose is `car_pose * local`, read off the
    /// physics pose directly so a never-propagated `GlobalTransform`
    /// cannot lag a frame behind the impact.
    pub local: Transform,
    /// Convex hull over the part's baked mesh verts, in node space —
    /// the fragment's collider. `None` on degenerate geometry: the
    /// part still detaches visually but spawns no body.
    pub collider: Option<Collider>,
    /// Vertex centroid in node space — the fragment's centre of mass.
    /// The record's `CG` field is not consumed: on vehicle fragments
    /// it mixes car-space anchors and ~zero conventions across the
    /// retail set (UNK-13), while the measured centroid is always the
    /// part's own mass centre.
    pub centroid: Vec3,
}

impl BreakPartVisual {
    /// Tag a `BREAK<NN>` render node: hull and centroid over the same
    /// baked (recentered) verts `spawn_groups` meshes.
    pub fn of(part: &ModelPart, local: Transform) -> Self {
        let recenter = part.recenter.map(Vec3::from).unwrap_or(Vec3::ZERO);
        let mut verts: Vec<Vec3> = Vec::new();
        if let Some(groups) = part.best_nonempty_lod().or_else(|| part.best_lod()) {
            for g in groups {
                verts.extend(g.positions.iter().map(|p| Vec3::from(*p) - recenter));
            }
        }
        let centroid = if verts.is_empty() {
            Vec3::ZERO
        } else {
            verts.iter().sum::<Vec3>() / verts.len() as f32
        };
        Self {
            part: part.name.clone(),
            local,
            collider: Collider::convex_hull(verts),
            centroid,
        }
    }
}

/// A detached part's spawned fragment body, marked so diagnostics can
/// tell it from stamped prop debris (the session owns it through
/// `SessionEntity` either way).
#[derive(Component)]
pub struct BreakFragment {
    /// The vehicle entity it came off.
    pub vehicle: Entity,
    /// Index into that vehicle's [`VehicleBreaks::parts`].
    pub part: usize,
}

/// The break-node query shape [`detach_breaks`] scans and
/// [`spawn_break_fragment`] hides through — shared with the
/// replicated-detach reconcile in [`crate::netdrive`], which looks the
/// same nodes up and re-shows them on a repair.
pub(crate) type BreakVisualMut = (
    Entity,
    &'static BreakPartVisual,
    &'static mut Visibility,
    &'static ChildOf,
);

/// Spawn one detached part's fragment body — the shared half of
/// [`detach_breaks`] (authority, real impact) and the replicated-detach
/// reconcile in [`crate::netdrive`] (predicted copy). Hides the intact
/// node — dropping any `CockpitHidden` claim so leaving the cockpit
/// never re-shows the panel — claims an active-pool slot and spawns
/// the part's convex body at `car_pose * node_local`, carrying the
/// car's velocity at the part centroid. `kick` is the impact's
/// `dir`/`point`/`severity` triple — `None` on the replicated path,
/// where the wire carries the detach *state* but not the per-part
/// launch impulse: the copy's fragment inherits the replicated motion
/// and tumbles on its own (designed presentation — the bitmask, not
/// the launch, is what the authority asserts). No collider or no pool
/// slot leaves the part detached without a body — it still left the
/// rig. Returns the spawned body's `(entity, object)` id pair.
#[allow(clippy::too_many_arguments)] // the pool, the writers and the part/car geometry are genuinely distinct borrows
pub(crate) fn spawn_break_fragment(
    node: Entity,
    entity: Entity,
    part: usize,
    spec: &BreakPartSpec,
    pose: (Vec3, Quat),
    motion: (Vec3, Vec3),
    kick: Option<(Vec3, Vec3, f32)>,
    owner: SessionEntity,
    occupied: &mut usize,
    session: &mut Session,
    pool: &BangerPool,
    bangers: &mut Query<BangerMut>,
    banger_writer: &mut MessageWriter<BangerStateChanged>,
    visuals: &mut Query<BreakVisualMut, Without<Banger>>,
    render_parts: &Query<(&Mesh3d, &MeshMaterial3d<StandardMaterial>, &ChildOf)>,
    commands: &mut Commands,
) -> Option<(Entity, ObjectId)> {
    let Ok((_, visual, mut vis, _)) = visuals.get_mut(node) else {
        return None;
    };
    let (local, collider, centroid) = (visual.local, visual.collider.clone(), visual.centroid);
    *vis = Visibility::Hidden;
    commands.entity(node).remove::<crate::dash::CockpitHidden>();
    let collider = collider?;
    if !claim_slot(
        occupied,
        session.tick(),
        session.generation(),
        pool,
        bangers,
        banger_writer,
        commands,
    ) {
        return None;
    }
    let (car_pos, car_rot) = pose;
    let (car_lv, car_av) = motion;
    let (kick_dir, kick_lever_from, kick_speed) = kick.unwrap_or((Vec3::ZERO, Vec3::ZERO, 0.0));
    let node_pos = car_pos + car_rot * local.translation;
    let node_rot = car_rot * local.rotation;
    let centroid_world = node_pos + node_rot * centroid;
    let lever = kick_lever_from - centroid_world;
    let object = session.mint_object_id();
    let transform = Transform::from_translation(node_pos).with_rotation(node_rot);
    let fragment = commands
        .spawn(banger_bundle(
            Banger {
                phase: BangerPhase::Active,
                def: spec.def.clone(),
                activated: Some(session.tick()),
            },
            object,
            session.authority_role(),
            owner,
            collider,
            transform,
            format!("breakaway-{}", spec.def.name),
        ))
        .insert((
            RigidBody::Dynamic,
            LinearVelocity(car_lv + car_av.cross(centroid_world - car_pos) + kick_dir * kick_speed),
            AngularVelocity(
                car_av
                    + spec
                        .def
                        .angular_kick(lever, kick_dir * kick_speed * spec.def.mass),
            ),
            // The hull's own mass centre — the record CG's convention
            // is not consumed (see the component doc).
            CenterOfMass(centroid),
            BreakFragment {
                vehicle: entity,
                part,
            },
        ))
        .id();
    // The fragment's visuals are the part's own mesh children,
    // re-spawned under the new body.
    for (mesh, material, child) in render_parts.iter() {
        if child.parent() != node {
            continue;
        }
        let part = commands
            .spawn((
                Mesh3d(mesh.0.clone()),
                MeshMaterial3d(material.0.clone()),
                Transform::IDENTITY,
            ))
            .id();
        commands.entity(fragment).add_child(part);
    }
    Some((fragment, object))
}

/// Fixed-step: detach authored breakaway parts whose `ImpulseLimit2`
/// an impact's delivered impulse exceeds. Runs after
/// `apply_impact_damage` — a wrecking blow can shed a panel the same
/// tick it disables the car, and `resolve_disabled`'s repair puts it
/// back in the same pass.
#[allow(clippy::too_many_arguments)] // Bevy system: the detach decision, the fragment spawn and the pool all need their own handles
pub fn detach_breaks(
    mut reader: MessageReader<ImpactEvent>,
    mut session: ResMut<Session>,
    pool: Res<BangerPool>,
    identities: Query<(Entity, &ObjectIdentity, Option<&SessionEntity>)>,
    // Disjoint from `bangers` below: a vehicle is never a Banger, and
    // `BangerMut` takes `&mut LinearVelocity`/`&mut AngularVelocity`.
    motions: Query<(&Position, &Rotation, &LinearVelocity, &AngularVelocity), Without<Banger>>,
    mut rigs: Query<&mut VehicleBreaks>,
    // Disjoint from `bangers` below: a break-part node is never a
    // Banger, and both queries take `&mut Visibility`.
    mut visuals: Query<BreakVisualMut, Without<Banger>>,
    render_parts: Query<(&Mesh3d, &MeshMaterial3d<StandardMaterial>, &ChildOf)>,
    mut bangers: Query<BangerMut>,
    mut banger_writer: MessageWriter<BangerStateChanged>,
    mut writer: MessageWriter<PartDetached>,
    mut report: ResMut<BreakReport>,
    mut commands: Commands,
) {
    // No impacts is nearly every step; skip the index and the banger
    // census below (see `damage::resolve_disabled`).
    if reader.is_empty() {
        return;
    }
    // Same drain discipline as every impact consumer: a buffered
    // stale stream must never flush detaches into a pause, a loading
    // phase or a session the authority does not own.
    if !session.is_playing() || !session.authority_role().is_authority() {
        reader.read().for_each(drop);
        return;
    }
    let tick = session.tick();
    let generation = session.generation();
    let index: HashMap<ObjectId, (Entity, Option<SessionEntity>)> = identities
        .iter()
        .map(|(e, id, o)| (id.0, (e, o.copied())))
        .collect();
    let mut occupied = bangers
        .iter()
        .filter(|(_, _, b, _, _, _, _)| b.phase == BangerPhase::Active)
        .count();

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
        for (side, (object, participant)) in resolved.iter().enumerate() {
            let (object, participant) = (*object, *participant);
            let Some((entity, owner)) = participant else {
                continue;
            };
            // A remote driver's seat is this authority's participant —
            // its rig sheds parts like an AI's; the detached bitmask
            // rides its `SnapEntry` down to the copies (F25-B, v11).
            let Ok(mut rig) = rigs.get_mut(entity) else {
                continue;
            };
            // The part's own record decides: `limit / mass` is the
            // authored detach speed (≈31.25 or ≈2500 m/s on retail).
            let detachable = rig.detachable(event.severity);
            if detachable.is_empty() {
                continue;
            }
            let Some(owner) = owner else {
                continue;
            };
            // The kick points the way the striker travelled into this
            // side — the same convention the prop activations use.
            let dir = if side == 0 {
                -event.normal
            } else {
                event.normal
            };
            let (car_pos, car_rot, car_lv, car_av) = motions
                .get(entity)
                .map(|(p, r, l, a)| (p.0, r.0, l.0, a.0))
                .unwrap_or((Vec3::ZERO, Quat::IDENTITY, Vec3::ZERO, Vec3::ZERO));
            for i in detachable {
                // The part's render node: hidden on detach, shown on
                // repair. A rig part with no node cannot detach — the
                // spec was built from the same model the visuals were,
                // so a miss is an assembly inconsistency, not data.
                let Some(node) = visuals
                    .iter()
                    .find(|(_, bpv, _, child)| {
                        child.parent() == entity && bpv.part == rig.parts[i].spec.name
                    })
                    .map(|(e, _, _, _)| e)
                else {
                    continue;
                };
                let spec = rig.parts[i].spec.clone();
                let spawned = spawn_break_fragment(
                    node,
                    entity,
                    i,
                    &spec,
                    (car_pos, car_rot),
                    (car_lv, car_av),
                    Some((dir, event.point, event.severity)),
                    owner,
                    &mut occupied,
                    &mut session,
                    &pool,
                    &mut bangers,
                    &mut banger_writer,
                    &mut visuals,
                    &render_parts,
                    &mut commands,
                );
                if rig.detach(i, spawned.map(|(e, _)| e)) {
                    report.detached += 1;
                    writer.write(PartDetached {
                        object,
                        generation,
                        tick,
                        part: spec.name,
                        fragment: spawned.map(|(_, o)| o),
                        estimate: event.severity * spec.def.mass,
                    });
                }
            }
        }
    }
}

/// Restore one vehicle's rig where a repair lands (`resolve_disabled`
/// calls it next to `damage.reset()`): re-attach every part, despawn
/// the spawned fragments and show the intact nodes again. Returns the
/// number of parts that were off the rig — `0` on an untouched one,
/// so callers can count without a pre-check.
pub(crate) fn restore_rig(
    entity: Entity,
    rig: &mut VehicleBreaks,
    visuals: &mut Query<(&BreakPartVisual, &mut Visibility, &ChildOf)>,
    commands: &mut Commands,
) -> usize {
    let restored = rig.detached_count();
    for fragment in rig.restore() {
        commands.entity(fragment).despawn();
    }
    if restored > 0 {
        for (_, mut vis, child) in visuals.iter_mut() {
            if child.parent() == entity {
                *vis = Visibility::Visible;
            }
        }
    }
    restored
}
