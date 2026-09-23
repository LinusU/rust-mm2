//! Rendering rig for imported stock vehicles: builds child entities for
//! every [`VehicleModel`] part under the physics root, keeps wheel mounts
//! (suspension + steer) separate from spin nodes, and drives light-glow
//! visibility from vehicle state.
//!
//! The physics body itself is spawned by `main` via
//! `mm2_vehicle::vehicle_bundle`; this module only owns presentation.

use avian3d::prelude::*;
use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use mm2_assets::Vfs;
use mm2_content::TrailerDef;
use mm2_content::model::{MeshGroup, ModelPart, PartRole, VehicleModel};
use mm2_game::SessionEntity;
use mm2_vehicle::vehicle::{DriveDirection, Vehicle, VehicleInput, VehicleState};

use crate::city::MaterialCache;

/// A wheel suspension/steer node: child of the vehicle body. Its local
/// transform follows the physics wheel's suspension drop and steer angle;
/// the [`WheelSpin`] child carries the rolling rotation and fender
/// siblings move with the mount but never spin.
#[derive(Component)]
pub struct WheelMount {
    /// Entity carrying the [`Vehicle`] component this wheel belongs to.
    pub vehicle: Entity,
    /// Index into `VehicleConfig::wheels`.
    pub index: usize,
    /// Car-space offset added to the mount's position — nonzero on
    /// "back-back" follower wheels (`whl4`/`whl5`), which copy the
    /// referenced physics wheel's suspension/steer/spin shifted by the
    /// authored offset between the two wheels.
    pub follow_offset: Vec3,
}

/// Rolling-rotation node, child of a [`WheelMount`].
#[derive(Component)]
pub struct WheelSpin;

/// Which state lights a glow-quad part (`HLIGHT`, `BLIGHT`, ...).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlowKind {
    /// `HLIGHT` — headlight beams.
    Headlight,
    /// `TLIGHT` — tail lights.
    Taillight,
    /// `BLIGHT` — brake lights.
    Brake,
    /// `RLIGHT` — reverse lights.
    Reverse,
}

/// A light-glow part whose visibility follows the owning vehicle's state.
/// The owning vehicle is the entity's parent.
#[derive(Component)]
pub struct GlowPart(pub GlowKind);

/// `L`-toggled headlight state.
#[derive(Resource, Default)]
pub struct HeadlightsOn(pub bool);

/// A trailer body attached to a towing vehicle by a hitch joint. A system
/// mirrors the towing vehicle's brake input so the trailer's authored
/// brakes engage.
#[derive(Component)]
pub struct Trailer {
    /// The vehicle entity pulling this trailer.
    pub towing: Entity,
    /// Car-space offset from the car origin to the trailer origin at rest
    /// — used to re-place the trailer on reset.
    pub rest_offset: Vec3,
}

/// Convert a [`MeshGroup`] into a Bevy mesh, baking the optional recenter
/// offset (translation only — normals are unaffected).
fn group_mesh(g: &MeshGroup, recenter: Option<Vec3>) -> Mesh {
    let mut positions = g.positions.clone();
    if let Some(c) = recenter {
        for p in &mut positions {
            p[0] -= c.x;
            p[1] -= c.y;
            p[2] -= c.z;
        }
    }
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    if g.normals.len() == g.positions.len() {
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, g.normals.clone());
    } else {
        mesh.compute_normals();
    }
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, g.uvs.clone());
    mesh.insert_indices(Indices::U32(g.indices.clone()));
    mesh
}

/// Material for a mesh group under the selected paint job.
fn group_material(
    model: &VehicleModel,
    paint: usize,
    offset: usize,
    mats: &mut MaterialCache<'_>,
    glow: bool,
) -> Handle<StandardMaterial> {
    let idx = paint * model.shaders_per_paint_job + offset;
    let Some(shader) = model.shaders.get(idx) else {
        return mats.fallback();
    };
    let handle = mats.shader_material(shader);
    if !glow {
        return handle;
    }
    // Glow quads are self-lit: unlit + emissive so they read as lit lamps
    // without paying for real lights.
    mats.adjusted(&handle, |m| {
        m.unlit = true;
        m.emissive = LinearRgba::rgb(4.0, 4.0, 4.0);
    })
}

/// Spawn one mesh entity per `MeshGroup` of the part's best LOD under
/// `parent`, baking `recenter` into the geometry. `texel` is the
/// accumulating damage rig — passed for `Body` parts only, matching
/// the recovered `fxTexelDamage::Init` binding over the high-LOD body.
// Bevy spawn helpers thread `Commands` plus the several `Assets<T>`
// stores the meshes, images and materials live in; bundling them behind
// a context struct would only move the same borrows one level down.
#[allow(clippy::too_many_arguments)]
fn spawn_groups(
    commands: &mut Commands,
    parent: Entity,
    model: &VehicleModel,
    paint: usize,
    part: &ModelPart,
    mats: &mut MaterialCache<'_>,
    meshes: &mut Assets<Mesh>,
    glow: bool,
    mut texel: Option<&mut crate::texel_fx::TexelDamageBuilder>,
) {
    let recenter = part.recenter.map(Vec3::from);
    // The transform a raw vertex rides to reach car space: the group
    // bakes `−recenter` into its positions and the node sits at
    // `attach` — the offset `TexelDamageTri` positions carry.
    let car_offset =
        part.origin.map(Vec3::from).unwrap_or(Vec3::ZERO) - recenter.unwrap_or(Vec3::ZERO);
    let Some(groups) = part.best_nonempty_lod().or_else(|| part.best_lod()) else {
        return;
    };
    for g in groups {
        if g.indices.is_empty() {
            continue;
        }
        let mesh = meshes.add(group_mesh(g, recenter));
        let mat = group_material(model, paint, g.shader_offset, mats, glow);
        let mat = match texel.as_deref_mut() {
            Some(builder) => builder.bind_group(mats, model, paint, g, car_offset, mat),
            None => mat,
        };
        commands
            .entity(parent)
            .with_child((Mesh3d(mesh), MeshMaterial3d(mat)));
    }
}

/// Spawn all renderable parts of `model` under `root` (the physics body).
/// Returns texture stems that failed to resolve.
///
/// `texel_damage` carries the authored `vehcardamage` record and the
/// spawn's deterministic seed: with it, `Body` parts bound to
/// `_dmg`-paired shader slots render through a per-vehicle cloned
/// texture and `root` gains a [`crate::texel_fx::TexelDamageRig`]
/// (F05-B.9). `None` — ambient traffic, trailers — spawns the shared
/// bindings only.
// Bevy spawn helpers thread `Commands` plus the several `Assets<T>`
// stores the meshes, images and materials live in; bundling them behind
// a context struct would only move the same borrows one level down.
#[allow(clippy::too_many_arguments)]
pub fn spawn_vehicle_model(
    commands: &mut Commands,
    vfs: &Vfs,
    model: &VehicleModel,
    paint: usize,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
    root: Entity,
    texel_damage: Option<(&mm2_formats::veh::VehCarDamage, u64)>,
) -> Vec<String> {
    let mut mats = MaterialCache::new(vfs, images, materials);
    let mut texel = texel_damage.map(|(d, seed)| crate::texel_fx::TexelDamageBuilder::new(d, seed));

    // Physics wheel index within this model: ordinal among `simulated`
    // wheels of the same class, matching the order `assemble` consumed
    // them into `WheelGeom`/`WheelConfig`. `!simulated` wheels have no
    // physics wheel: a follower (`whl4`/`whl5` back-back wheels) mounts
    // onto its reference wheel's physics index plus the authored offset
    // between the pair, and a parked decorative part gets `usize::MAX`
    // so it just stays at the authored origin.
    let mut car_ord = 0usize;
    let mut trailer_ord = 0usize;
    let mut wheel_phys = vec![usize::MAX; model.wheels.len()];
    for (wi, w) in model.wheels.iter().enumerate() {
        if w.simulated {
            let ord = if w.trailer {
                let o = trailer_ord;
                trailer_ord += 1;
                o
            } else {
                let o = car_ord;
                car_ord += 1;
                o
            };
            wheel_phys[wi] = ord;
        }
    }
    let mut follow_offsets = vec![Vec3::ZERO; model.wheels.len()];
    for (wi, w) in model.wheels.iter().enumerate() {
        let Some(reference) = w.follows else { continue };
        let Some((ri, ref_wheel)) = model
            .wheels
            .iter()
            .enumerate()
            .find(|(_, r)| r.trailer == w.trailer && r.index == reference && r.simulated)
        else {
            continue;
        };
        wheel_phys[wi] = wheel_phys[ri];
        follow_offsets[wi] = Vec3::from(w.origin) - Vec3::from(ref_wheel.origin);
    }

    // Pass 1: wheel mounts + spin nodes. Fenders may precede their wheel
    // in part order, so mounts must exist before anything links to them.
    // `mounts` is indexed by model-part index.
    let mut mounts: Vec<Option<Entity>> = vec![None; model.parts.len()];
    // model-part index → (physics wheel index, follow offset)
    let mut part_wheel: Vec<Option<(usize, Vec3)>> = vec![None; model.parts.len()];
    for (wi, w) in model.wheels.iter().enumerate() {
        for &pi in &w.parts {
            part_wheel[pi] = Some((wheel_phys[wi], follow_offsets[wi]));
        }
    }
    for (pi, part) in model.parts.iter().enumerate() {
        if !matches!(part.role, PartRole::Wheel(_) | PartRole::TrailerWheel(_)) {
            continue;
        }
        let attach = part.origin.map(Vec3::from).unwrap_or(Vec3::ZERO);
        let (phys_idx, follow_offset) = part_wheel[pi].unwrap_or((0, Vec3::ZERO));
        let mount = commands
            .spawn((
                WheelMount {
                    vehicle: root,
                    index: phys_idx,
                    follow_offset,
                },
                Transform::from_translation(attach),
                Visibility::Visible,
            ))
            .id();
        commands.entity(root).add_child(mount);
        let spin = commands
            .spawn((WheelSpin, Transform::IDENTITY, Visibility::Visible))
            .id();
        commands.entity(mount).add_child(spin);
        spawn_groups(
            commands, spin, model, paint, part, &mut mats, meshes, false, None,
        );
        mounts[pi] = Some(mount);
    }

    // Pass 2: everything else. Fenders parent to their wheel's mount, with
    // their transform expressed relative to the wheel's authored centre
    // (the mount sits at the live centre, which equals the authored origin
    // at rest).
    for part in &model.parts {
        let attach = part.origin.map(Vec3::from).unwrap_or(Vec3::ZERO);
        match part.role {
            PartRole::Shadow | PartRole::Wheel(_) | PartRole::TrailerWheel(_) => continue,
            PartRole::Fender(n) => {
                let parent = model
                    .wheels
                    .iter()
                    .enumerate()
                    .find(|(_, w)| !w.trailer && w.index == n)
                    .and_then(|(_, w)| w.parts.first().and_then(|pi| mounts[*pi]));
                match parent {
                    Some(mount) => {
                        let wheel_origin = model
                            .wheels
                            .iter()
                            .find(|w| !w.trailer && w.index == n)
                            .map(|w| Vec3::from(w.origin))
                            .unwrap_or(Vec3::ZERO);
                        let node = commands
                            .spawn((
                                Transform::from_translation(attach - wheel_origin),
                                Visibility::Visible,
                            ))
                            .id();
                        commands.entity(mount).add_child(node);
                        spawn_groups(
                            commands, node, model, paint, part, &mut mats, meshes, false, None,
                        );
                    }
                    None => {
                        let node = commands
                            .spawn((Transform::from_translation(attach), Visibility::Visible))
                            .id();
                        commands.entity(root).add_child(node);
                        spawn_groups(
                            commands, node, model, paint, part, &mut mats, meshes, false, None,
                        );
                    }
                }
            }
            PartRole::Break => {
                // Intact breakaway panel (F05-B.3): an ordinary part
                // node plus the `BreakPartVisual` tag the detach
                // system hides and turns into the fragment body.
                let local = Transform::from_translation(attach);
                let node = commands.spawn((local, Visibility::Visible)).id();
                commands.entity(root).add_child(node);
                commands
                    .entity(node)
                    .insert(crate::breakaway::BreakPartVisual::of(part, local));
                spawn_groups(
                    commands, node, model, paint, part, &mut mats, meshes, false, None,
                );
            }
            role => {
                let glow = match role {
                    PartRole::HeadlightGlow => Some(GlowKind::Headlight),
                    // HEADLIGHTn are flat lit-lens quads shown only when
                    // the headlights are on — the housing lives in BODY.
                    PartRole::Headlight(_) => Some(GlowKind::Headlight),
                    PartRole::TaillightGlow => Some(GlowKind::Taillight),
                    PartRole::BrakeGlow => Some(GlowKind::Brake),
                    PartRole::ReverseGlow => Some(GlowKind::Reverse),
                    _ => None,
                };
                let node = commands
                    .spawn((Transform::from_translation(attach), Visibility::Visible))
                    .id();
                commands.entity(root).add_child(node);
                if let Some(kind) = glow {
                    commands
                        .entity(node)
                        .insert((GlowPart(kind), Visibility::Hidden));
                }
                spawn_groups(
                    commands,
                    node,
                    model,
                    paint,
                    part,
                    &mut mats,
                    meshes,
                    glow.is_some(),
                    // `fxTexelDamage` binds the high-LOD body only —
                    // lights/glows and other parts keep shared bindings.
                    matches!(role, PartRole::Body)
                        .then(|| texel.as_mut())
                        .flatten(),
                );
            }
        }
    }

    // The rig lands only when at least one body slot paired a `_dmg`
    // texture — the same authored-presence policy as the other damage
    // components, one level down.
    if let Some(rig) = texel.and_then(|b| b.finish()) {
        commands.entity(root).insert(rig);
    }

    mats.missing_textures().iter().cloned().collect()
}

/// Spawn a trailer body joined to `car` at the authored hitch anchors and
/// build its model under it. Returns the trailer entity and any missing
/// texture stems. The trailer body and its joint are stamped with `owner`
/// (its model children cascade through it).
// Bevy spawn helpers thread `Commands` plus the several `Assets<T>`
// stores the meshes, images and materials live in; bundling them behind
// a context struct would only move the same borrows one level down.
#[allow(clippy::too_many_arguments)]
pub fn spawn_trailer(
    commands: &mut Commands,
    vfs: &Vfs,
    trailer: &TrailerDef,
    paint: usize,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
    car: Entity,
    car_transform: Transform,
    owner: SessionEntity,
) -> (Entity, Vec<String>) {
    // Trailer spawn pose: both hitch anchors coincide in world space while
    // the trailer shares the car's heading. `rest_offset` is the car-space
    // vector from the car origin to the trailer origin, reused on reset.
    let rest_offset = Vec3::from(trailer.car_hitch) - Vec3::from(trailer.trailer_hitch);
    let pos = car_transform.translation + car_transform.rotation * rest_offset;
    let entity = commands
        .spawn((
            owner,
            Trailer {
                towing: car,
                rest_offset,
            },
            mm2_vehicle::vehicle_bundle(&trailer.config),
            Transform::from_translation(pos).with_rotation(car_transform.rotation),
            TransformInterpolation,
            Visibility::Visible,
        ))
        .id();
    commands.spawn((
        owner,
        SphericalJoint::new(car, entity)
            .with_local_anchor1(Vec3::from(trailer.car_hitch))
            .with_local_anchor2(Vec3::from(trailer.trailer_hitch))
            .with_swing_limits(-0.6, 0.6)
            .with_twist_limits(-0.15, 0.15),
        // The hitched hulls touch at the anchor; the joint holds the rig
        // together, so the contact would only fight it.
        JointCollisionDisabled,
    ));
    let missing = spawn_vehicle_model(
        commands,
        vfs,
        &trailer.model,
        paint,
        meshes,
        images,
        materials,
        entity,
        // Trailers carry no `vehcardamage` — no texel rig.
        None,
    );
    (entity, missing)
}

/// Position/steer wheel mounts and spin nodes from physics state. Runs in
/// `Update` so it tracks the interpolated physics pose; mounts are children
/// of the body so everything is local-space.
pub fn update_wheel_visuals(
    vehicles: Query<(&Vehicle, &VehicleState)>,
    mut mounts: Query<(&WheelMount, &mut Transform, &Children)>,
    mut spins: Query<&mut Transform, (With<WheelSpin>, Without<WheelMount>)>,
) {
    for (mount, mut xf, children) in &mut mounts {
        let Ok((veh, state)) = vehicles.get(mount.vehicle) else {
            continue;
        };
        let cfg = &veh.config;
        let (Some(wheel), Some(ws)) = (cfg.wheels.get(mount.index), state.wheels.get(mount.index))
        else {
            continue;
        };
        let suspension = wheel.suspension.as_ref().unwrap_or(&cfg.suspension);
        // Wheel centre = physics hardpoint minus the live droop. At rest
        // (compression = sag) this lands on the authored wheel origin.
        let drop = if ws.grounded {
            suspension.travel - ws.compression
        } else {
            suspension.travel
        };
        xf.translation = Vec3::from(wheel.position) + mount.follow_offset + Vec3::NEG_Y * drop;
        let steer = if wheel.steered {
            state.steer_angle * wheel.steer_scale
        } else {
            0.0
        };
        xf.rotation = Quat::from_rotation_y(-steer);
        for child in children {
            if let Ok(mut spin) = spins.get_mut(*child) {
                spin.rotation = Quat::from_rotation_x(ws.spin);
            }
        }
    }
}

/// Toggle headlight glows (`L`).
pub fn toggle_headlights(keys: Res<ButtonInput<KeyCode>>, mut on: ResMut<HeadlightsOn>) {
    if keys.just_pressed(KeyCode::KeyL) {
        on.0 = !on.0;
    }
}

/// Glow-quad visibility from the owning vehicle's state: brake lights on
/// pedal, reverse lights while reversing, head/tail lights on `L`.
pub fn update_glows(
    lights: Res<HeadlightsOn>,
    vehicles: Query<(&VehicleState, &VehicleInput)>,
    mut glows: Query<(&GlowPart, &mut Visibility, &ChildOf)>,
) {
    for (glow, mut vis, parent) in &mut glows {
        let Ok((state, input)) = vehicles.get(parent.parent()) else {
            continue;
        };
        let braking = input.brake > 0.05;
        let show = match glow.0 {
            GlowKind::Headlight | GlowKind::Taillight => lights.0,
            GlowKind::Brake => braking && state.direction == DriveDirection::Forward,
            GlowKind::Reverse => braking && state.direction == DriveDirection::Reverse,
        };
        let want = if show {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        if *vis != want {
            *vis = want;
        }
    }
}

/// Copy the towing vehicle's brake/handbrake onto the trailer so its
/// authored brake bias actually engages.
pub fn trailer_input(
    cars: Query<&VehicleInput, With<mm2_game::PlayerVehicle>>,
    mut trailers: Query<(&Trailer, &mut VehicleInput), Without<mm2_game::PlayerVehicle>>,
) {
    for (t, mut input) in &mut trailers {
        if let Ok(car_input) = cars.get(t.towing) {
            input.brake = car_input.brake;
            input.handbrake = car_input.handbrake;
        }
    }
}
