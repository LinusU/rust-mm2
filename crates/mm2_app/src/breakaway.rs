//! Vehicle breakaway detachment (F05-B.3) — where the authored
//! inventory meets Avian and the session lifecycle.
//!
//! [`detach_breaks`] consumes the same deduplicated [`ImpactEvent`]
//! stream the damage accumulator reads, and computes the same delivered
//! impulse for each participant — approach speed × the other side's
//! mass, the struck car's own mass for a wall hit
//! ([`crate::contracts::delivered_impulse`]). The gate is recovered from
//! the retail binary (REC-2): when that impulse reaches a manager-wide
//! `10000` kg·m/s, exactly **one** part leaves the rig — the attached
//! part whose anchor is nearest the contact point — and the part's
//! authored `ImpulseLimit2` is never consulted. The reading this
//! replaces tested every part independently against `severity ×
//! part_mass > ImpulseLimit2` — the designed DSN-21 departure the
//! recovered rule retires; `docs/original-rules.md` carries the
//! recovery. A detaching part's intact render node hides and
//! a fragment body spawns at the detached mesh's pose — convex hull over
//! the part's own verts, the record's physicals, the car's velocity at
//! the part plus the impact kick and spin, pooled like prop debris
//! through the shared [`BangerPool`]. One bounded [`PartDetached`] fires
//! per part per attachment.
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
    Banger, BangerPhase, BangerPool, BangerStateChanged, BreakGate, BreakPartSpec, DamageEvent,
    DamageTier, ImpactEvent, NavRng, ObjectId, ObjectIdentity, PartDetached, Session,
    SessionEntity, VehicleBreaks,
};

use crate::banger::{BangerMut, banger_bundle, claim_slot};
use crate::contracts::delivered_impulse;
use mm2_vehicle::original::MPS_TO_MPH;

/// Determinism domain for the damage-driven wheel episode's draws —
/// XORs the object seed and the episode tick so the corner picks and the
/// fragment kick cannot coincide with another system's `NavRng` stream.
const WHEEL_EJECT_DOMAIN: u64 = 0x7768_6c5f_656a_6374;

/// Build one vehicle's breakaway rig from its authored inventory: the
/// impact-gated `BREAK<NN>` panels first, then the damage-gated
/// `whl0..3` wheels (REC-2). The order is fixed by the authored
/// definition every process shares, so a rig's part indices are the
/// replicated `SnapEntry.breaks` bit positions (protocol v11, F25-B).
/// `None` when the vehicle authored neither side — the same
/// authored-absence policy as the other damage components.
pub fn vehicle_rig(def: &mm2_content::VehicleDef) -> Option<VehicleBreaks> {
    let mut specs: Vec<BreakPartSpec> =
        Vec::with_capacity(def.breaks.len() + def.eject_wheels.len());
    specs.extend(def.breaks.iter().map(|b| BreakPartSpec {
        name: b.name.clone(),
        def: b.def.clone(),
        gate: BreakGate::Impact,
    }));
    specs.extend(def.eject_wheels.iter().map(|w| BreakPartSpec {
        name: w.name.clone(),
        def: w.def.clone(),
        gate: BreakGate::Damage,
    }));
    (!specs.is_empty()).then(|| VehicleBreaks::new(specs))
}

/// Per-session evidence counters for the breakaway pipeline — the
/// `brk=` field of the headless smoke record. Session-scoped like
/// [`crate::damage::DamageReport`]: `drive_session` resets it during
/// teardown.
#[derive(Resource, Debug, Default)]
pub struct BreakReport {
    /// Parts that left a rig this session (impact-gated panels and
    /// damage-gated wheels together).
    pub detached: u64,
    /// Wheels a damage-driven episode ejected (REC-2) — a subset of
    /// `detached`, kept apart because the trigger is different.
    pub ejected: u64,
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

/// The render node of one intact breakaway part, tagged by
/// `car_visual` when the model carries a part the vehicle's authored
/// inventory can detach (a `BREAK<NN>` panel or a `whl0..3` wheel).
/// Detachment hides this node and turns its data into the spawned
/// fragment; repair shows it again. The owning vehicle is stored rather
/// than read from `ChildOf`: a wheel's `WheelSpin` node hangs under its
/// `WheelMount`, not directly under the root.
#[derive(Component)]
pub struct BreakPartVisual {
    /// The vehicle root this node belongs to.
    pub vehicle: Entity,
    /// Model part stem (`break01`, `whl0`) — the key the rig's spec uses.
    pub part: String,
    /// This node's local transform under the vehicle root — the
    /// fragment's spawn pose is `car_pose * local`, read off the
    /// physics pose directly so a never-propagated `GlobalTransform`
    /// cannot lag a frame behind the impact. For a wheel this is the
    /// authored mount origin (the live suspension droop is presentation,
    /// not part identity).
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
    /// Tag a `BREAK<NN>`/`WHL<NN>` render node owned by `vehicle`: hull
    /// and centroid over the same baked (recentered) verts
    /// `spawn_groups` meshes.
    pub fn of(vehicle: Entity, part: &ModelPart, local: Transform) -> Self {
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
            vehicle,
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
/// same nodes up and re-shows them on a repair. The owning vehicle is a
/// field on the component, so a wheel's nested node is found the same
/// way a root-level panel is.
pub(crate) type BreakVisualMut = (Entity, &'static BreakPartVisual, &'static mut Visibility);

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
    let Ok((_, visual, mut vis)) = visuals.get_mut(node) else {
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
        // Born `Active`, so solid: only a dormant prop is a sensor.
        .remove::<Sensor>()
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

/// Fixed-step: detach the one part nearest the contact point when an
/// impact's delivered impulse reaches the recovered manager-wide gate
/// (REC-2). Runs after `apply_impact_damage` — a wrecking blow can shed
/// a panel the same tick it disables the car, and `resolve_disabled`'s
/// repair puts it back in the same pass.
#[allow(clippy::too_many_arguments)] // Bevy system: the detach decision, the fragment spawn and the pool all need their own handles
pub fn detach_breaks(
    mut reader: MessageReader<ImpactEvent>,
    mut session: ResMut<Session>,
    pool: Res<BangerPool>,
    identities: Query<(Entity, &ObjectIdentity, Option<&SessionEntity>)>,
    masses: Query<&ComputedMass>,
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
            // The recovered gate (REC-2): the same delivered impulse the
            // damage accumulator reads — approach speed × the other
            // side's mass, the car's own mass for a wall hit. The part's
            // authored `ImpulseLimit2` is not consulted.
            let impulse = delivered_impulse(
                event.severity,
                entity,
                resolved[1 - side].1.map(|(other, _)| other),
                &masses,
            );
            if !rig.detaches_on(impulse) {
                continue;
            }
            let Some(owner) = owner else {
                continue;
            };
            let (car_pos, car_rot, car_lv, car_av) = motions
                .get(entity)
                .map(|(p, r, l, a)| (p.0, r.0, l.0, a.0))
                .unwrap_or((Vec3::ZERO, Quat::IDENTITY, Vec3::ZERO, Vec3::ZERO));
            // The attached part nearest the contact point — REC-2's
            // smallest-squared-distance scan. A rig part whose render
            // node is missing has no anchor and can never be chosen, so
            // an assembly inconsistency stays visible in the rig.
            let anchors = rig.parts.iter().enumerate().filter_map(|(i, p)| {
                visuals
                    .iter()
                    .find(|(_, bpv, _)| bpv.vehicle == entity && bpv.part == p.spec.name)
                    .map(|(_, bpv, _)| (i, car_pos + car_rot * bpv.local.translation))
            });
            let Some(i) = rig.nearest_attached(event.point, anchors) else {
                continue;
            };
            // The part's render node: hidden on detach, shown on repair.
            let Some(node) = visuals
                .iter()
                .find(|(_, bpv, _)| bpv.vehicle == entity && bpv.part == rig.parts[i].spec.name)
                .map(|(e, _, _)| e)
            else {
                continue;
            };
            // The kick points the way the striker travelled into this
            // side — the same convention the prop activations use.
            let dir = if side == 0 {
                -event.normal
            } else {
                event.normal
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
                    estimate: impulse,
                });
            }
        }
    }
}

/// A uniformly-ish random unit vector off `rng` — the wheel fragment's
/// kick direction. The original's direction source (`0x4d8930`'s random
/// components scaled by the manager's ranges, `+0x14`) is unrecovered;
/// the magnitude is the recovered `Speed × 1.3`.
fn random_unit(rng: &mut NavRng) -> Vec3 {
    let z = rng.next_f32() * 2.0 - 1.0;
    let theta = rng.next_f32() * std::f32::consts::TAU;
    let r = (1.0 - z * z).max(0.0).sqrt();
    Vec3::new(r * theta.cos(), r * theta.sin(), z)
}

/// Fixed-step: the damage-driven wheel episode (REC-2). Consumes the
/// same [`DamageEvent`] stream the HUD and impairment read and, when a
/// participant's accumulated damage reaches `MaxDamage` (tier
/// `Disabled`), runs the recovered `0x4cdc90` rule once per episode:
/// `SpeedMPH` (`|forward velocity| × 2.236025`) selects the wheels —
/// `> 100` every wheel, `> 75` two random corners, `> 50` one — each
/// leaving as a pooled fragment with the recovered `Speed × 1.3` kick,
/// and the rig's one-shot flag latches until a repair restores it.
///
/// Authority-gated and drains while not `Playing`, like every other
/// damage consumer. Runs before `resolve_disabled` so a same-tick
/// repair re-attaches what the episode took (the original's reset
/// re-arms the episode flag on the same principle). A predicted client
/// never runs this: its copies reconcile the state off the replicated
/// `SnapEntry.breaks` bitmask (`netdrive::apply_snapshots`).
#[allow(clippy::too_many_arguments)] // Bevy system: the eject decision, the fragment spawn and the pool all need their own handles
pub fn eject_wheels(
    mut reader: MessageReader<DamageEvent>,
    mut session: ResMut<Session>,
    pool: Res<BangerPool>,
    identities: Query<(Entity, &ObjectIdentity, Option<&SessionEntity>)>,
    motions: Query<(&Position, &Rotation, &LinearVelocity, &AngularVelocity), Without<Banger>>,
    mut rigs: Query<&mut VehicleBreaks>,
    mut visuals: Query<BreakVisualMut, Without<Banger>>,
    render_parts: Query<(&Mesh3d, &MeshMaterial3d<StandardMaterial>, &ChildOf)>,
    mut bangers: Query<BangerMut>,
    mut banger_writer: MessageWriter<BangerStateChanged>,
    mut writer: MessageWriter<PartDetached>,
    mut report: ResMut<BreakReport>,
    mut commands: Commands,
) {
    if reader.is_empty() {
        return;
    }
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
        if event.generation != generation || event.tier != DamageTier::Disabled {
            continue;
        }
        let Some(&(entity, owner)) = index.get(&event.object) else {
            continue;
        };
        // Resolve the owner before the decision: the fragment the
        // episode spawns carries it, so a rig with no `SessionEntity`
        // cannot eject. Skipping the decision (rather than latching it)
        // leaves the one-shot episode armed for a car that later gains
        // an owner.
        let Some(owner) = owner else {
            continue;
        };
        let Ok(mut rig) = rigs.get_mut(entity) else {
            continue;
        };
        // The one-shot flag: a wreck that stays above `MaxDamage` (a
        // Breakdown episode) fires once, exactly like `0x4cdc90`'s
        // `+0xa4` gate.
        if rig.ejected {
            continue;
        }
        let Some((car_pos, car_rot, car_lv, car_av)) = motions
            .get(entity)
            .map(|(p, r, l, a)| (p.0, r.0, l.0, a.0))
            .ok()
        else {
            continue;
        };
        let forward = car_rot * Vec3::NEG_Z;
        let speed_ms = car_lv.dot(forward).abs();
        let speed_mph = speed_ms * MPS_TO_MPH;
        let seed = (event.object.generation << 32) | u64::from(event.object.slot);
        let mut rng = NavRng::new(seed ^ WHEEL_EJECT_DOMAIN ^ event.tick);
        for i in rig.eject_at(speed_mph, &mut rng) {
            // A wheel's `WheelSpin` node hangs under its `WheelMount`;
            // `BreakPartVisual` carries the owner either way.
            let Some(node) = visuals
                .iter()
                .find(|(_, bpv, _)| bpv.vehicle == entity && bpv.part == rig.parts[i].spec.name)
                .map(|(e, _, _)| e)
            else {
                continue;
            };
            let spec = rig.parts[i].spec.clone();
            let dir = random_unit(&mut rng);
            let spawned = spawn_break_fragment(
                node,
                entity,
                i,
                &spec,
                (car_pos, car_rot),
                (car_lv, car_av),
                Some((dir, car_pos, VehicleBreaks::EJECT_KICK_SCALE * speed_ms)),
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
                report.ejected += 1;
                writer.write(PartDetached {
                    object: event.object,
                    generation,
                    tick,
                    part: spec.name,
                    fragment: spawned.map(|(_, o)| o),
                    estimate: VehicleBreaks::EJECT_KICK_SCALE * speed_ms,
                });
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
    visuals: &mut Query<(&BreakPartVisual, &mut Visibility)>,
    commands: &mut Commands,
) -> usize {
    let restored = rig.detached_count();
    for fragment in rig.restore() {
        commands.entity(fragment).despawn();
    }
    if restored > 0 {
        for (visual, mut vis) in visuals.iter_mut() {
            if visual.vehicle == entity {
                *vis = Visibility::Visible;
            }
        }
    }
    restored
}
