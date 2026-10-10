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
use mm2_game::{EmergencyLights, SessionEntity};
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
    /// `SRNn` — one flare of a cop's light bar; the field is the half
    /// of the bar it sits on (`n` modulo 2 — retail's `srn0`/`srn2` are
    /// the left pair, `srn1`/`srn3` the right). Lit only while the
    /// car carries [`EmergencyLights`] and its half is the flash
    /// clock's current one (F20-B.1).
    Siren(usize),
}

/// A light-glow part whose visibility follows the owning vehicle's state.
/// The owning vehicle is the entity's parent.
#[derive(Component)]
pub struct GlowPart(pub GlowKind);

/// A flare's sprite node (a cop's `SRNn` light-bar lamps, a car's
/// `HEADLIGHTn` lenses): [`face_flares`] turns it to the view every
/// frame so the shine reads from any angle. The authored quad is flat
/// in the car's XY plane, edge-on from the side.
#[derive(Component)]
pub struct FlareSprite;

/// Edge multiplier on an authored flare quad (0.5 m on `vpcop`'s `SRNn`
/// and every `HEADLIGHTn`). A
/// glow sprite's falloff leaves only its core readable at the authored
/// size; the retail sprite size is unrecovered, so this is an enhanced
/// policy (ledger COP-10), not an original value.
pub const FLARE_SCALE: f32 = 3.0;

/// The retail glow sprite every lamp shader binds (`fxltglow`, a white
/// radial falloff, black at the edge); the `SRNn` shaders carry no
/// texture of their own and take this one.
const FLARE_TEXTURE: &str = "fxltglow";

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

/// The synthetic dev car's visuals: a cuboid body plus cylinder wheels on
/// the standard mount/spin rig. `root` is the physics vehicle entity the
/// pieces attach under. Shared by the local no-`--car` spawn and remote
/// participants whose pick is the dev car (F25-A).
pub fn spawn_dev_car(
    commands: &mut Commands,
    cfg: &mm2_vehicle::VehicleConfig,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    root: Entity,
) {
    let body_mesh = meshes.add(Cuboid::from_size(Vec3::from(cfg.chassis_size)));
    let body_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.85, 0.15, 0.1),
        metallic: 0.3,
        perceptual_roughness: 0.5,
        ..default()
    });
    commands
        .entity(root)
        .insert((Mesh3d(body_mesh), MeshMaterial3d(body_mat)));
    let wheel_mesh = meshes.add(Cylinder::new(0.34, 0.25));
    let wheel_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.1, 0.1, 0.1),
        perceptual_roughness: 0.9,
        ..default()
    });
    for (i, w) in cfg.wheels.iter().enumerate() {
        let mount = commands
            .spawn((
                WheelMount {
                    vehicle: root,
                    index: i,
                    follow_offset: Vec3::ZERO,
                },
                Transform::from_translation(Vec3::from(w.position)),
            ))
            .id();
        commands.entity(root).add_child(mount);
        let spin = commands.spawn((WheelSpin, Transform::IDENTITY)).id();
        commands.entity(mount).add_child(spin);
        commands.entity(spin).with_child((
            Mesh3d(wheel_mesh.clone()),
            MeshMaterial3d(wheel_mat.clone()),
            // Cylinder is Y-aligned: rotate onto the axle (X) and
            // scale to the configured radius.
            Transform::from_rotation(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2))
                .with_scale(Vec3::new(w.radius / 0.34, 1.0, w.radius / 0.34)),
        ));
    }
}

/// Convert a [`MeshGroup`] into a Bevy mesh, baking the optional recenter
/// offset (translation only — normals are unaffected).
pub(crate) fn group_mesh(g: &MeshGroup, recenter: Option<Vec3>) -> Mesh {
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
pub(crate) fn group_material(
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
    // Glow quads are self-lit sprites. The lamp shaders author a black,
    // fully transparent diffuse and carry the lamp colour in `emissive`
    // over a radial `fxltglow*` falloff texture, so the quad is that
    // texture tinted by the emissive colour and added to the frame —
    // the black falloff edge then vanishes instead of showing as a
    // box (rendered unlit and alpha-blended on the black diffuse it was
    // a flat grey rectangle).
    let tint = model
        .shaders
        .get(idx)
        .map(|s| [s.emissive[0], s.emissive[1], s.emissive[2]])
        .filter(|e| e.iter().any(|c| *c > 0.0))
        .unwrap_or([1.0; 3]);
    mats.adjusted(&handle, |m| {
        m.base_color = Color::srgb(tint[0], tint[1], tint[2]);
        m.alpha_mode = AlphaMode::Add;
        m.unlit = true;
        m.cull_mode = None;
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

/// Texture coordinates for a flat flare quad. Retail's `SRNn` vertices
/// carry no usable UVs (the parsed values are uninitialised memory,
/// e.g. -4.2e37) and `HEADLIGHTn`'s are all (0,0), so the glow sprite is mapped across the quad's own
/// extent in the car's XY plane — corner to corner, (0,0) top-left.
fn flare_uvs(positions: &[[f32; 3]]) -> Vec<[f32; 2]> {
    let span = |axis: usize| {
        let (lo, hi) = positions.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| {
            (lo.min(p[axis]), hi.max(p[axis]))
        });
        (lo, (hi - lo).max(f32::EPSILON))
    };
    let (x0, xs) = span(0);
    let (y0, ys) = span(1);
    positions
        .iter()
        .map(|p| [(p[0] - x0) / xs, 1.0 - (p[1] - y0) / ys])
        .collect()
}

/// The sprite material of one flare quad (`SRNn`, `HEADLIGHTn`): the
/// shader's glow texture (`fxltglow` when it names none — the `SRNn`
/// shaders carry no texture of their own) tinted by the shader's
/// colour — the diffuse for a coloured shader (blue / red / cream on
/// `vpcop`), the emissive for a lamp shader that authors a black,
/// transparent diffuse — and added to the frame so the black falloff
/// edge vanishes into the scene instead of showing as a box.
fn flare_material(
    model: &VehicleModel,
    paint: usize,
    offset: usize,
    mats: &mut MaterialCache<'_>,
) -> Handle<StandardMaterial> {
    let shader = model
        .shaders
        .get(paint * model.shaders_per_paint_job + offset);
    let tint = shader.map_or([1.0; 3], |s| {
        let c = if s.diffuse[3] < 0.01 {
            s.emissive
        } else {
            s.diffuse
        };
        if c[..3].iter().any(|v| *v > 0.0) {
            [c[0], c[1], c[2]]
        } else {
            [1.0; 3]
        }
    });
    let texture = shader
        .map(|s| s.texture.as_str())
        .filter(|t| !t.is_empty())
        .unwrap_or(FLARE_TEXTURE);
    let base = mats.get(texture);
    mats.adjusted(&base, |m| {
        m.base_color = Color::srgb(tint[0], tint[1], tint[2]);
        m.alpha_mode = AlphaMode::Add;
        m.unlit = true;
        m.cull_mode = None;
    })
}

/// Spawn one flare's sprite quads under `parent` (the flare's
/// glow node, already at the part's attach point).
fn spawn_flare(
    commands: &mut Commands,
    parent: Entity,
    model: &VehicleModel,
    paint: usize,
    part: &ModelPart,
    mats: &mut MaterialCache<'_>,
    meshes: &mut Assets<Mesh>,
) {
    let Some(groups) = part.best_nonempty_lod().or_else(|| part.best_lod()) else {
        return;
    };
    commands.entity(parent).insert(FlareSprite);
    for g in groups.iter().filter(|g| !g.indices.is_empty()) {
        let mut mesh = group_mesh(g, part.recenter.map(Vec3::from));
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, flare_uvs(&g.positions));
        let mat = flare_material(model, paint, g.shader_offset, mats);
        commands.entity(parent).with_child((
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(mat),
            bevy::light::NotShadowCaster,
        ));
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
        // `Inherited`: an unconditional `Visible` would override the
        // mount's `Hidden` — the cockpit split hides mounts under the
        // cockpit view (F22-A.3 exposed the same leak on the dash
        // subtree; every interior node follows its root's gate).
        let spin = commands
            .spawn((WheelSpin, Transform::IDENTITY, Visibility::Inherited))
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
                        // Under the mount the fender inherits its
                        // gate (a pinned `Visible` would leak past
                        // the mount's `Hidden` — same class as the
                        // dash-subtree repair in F22-A.3).
                        let node = commands
                            .spawn((
                                Transform::from_translation(attach - wheel_origin),
                                Visibility::Inherited,
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
                    // The flat `SRNn` quads are the bar's lamps; the
                    // `SIRENn` boxes (also `PartRole::Siren`) are its
                    // housing and stay solid.
                    PartRole::Siren(n) if part.name.starts_with("srn") => {
                        Some(GlowKind::Siren(n % 2))
                    }
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
                if matches!(glow, Some(GlowKind::Siren(_)) | Some(GlowKind::Headlight))
                    && matches!(role, PartRole::Siren(_) | PartRole::Headlight(_))
                {
                    commands.entity(node).insert(
                        Transform::from_translation(attach).with_scale(Vec3::splat(FLARE_SCALE)),
                    );
                    spawn_flare(commands, node, model, paint, part, &mut mats, meshes);
                    continue;
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
        // A child of the trailer so the trailer's despawn sweeps the
        // joint with it — as a standalone entity it would outlive a
        // mid-session leave/re-pick despawn, orphaned on dead bodies
        // until `SessionEntity` teardown swept it.
        ChildOf(entity),
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
        let steer = if let Some(original) = &cfg.original {
            let lock = original
                .wheels
                .iter()
                .find(|w| !w.rear)
                .map_or(0.0, |w| w.steering_limit);
            let input = if lock.abs() > 1e-6 {
                state.steer_angle / lock
            } else {
                0.0
            };
            original
                .wheels
                .get(mount.index)
                .map_or(0.0, |w| mm2_vehicle::original::wheel_steer(w, input))
        } else if wheel.steered {
            state.steer_angle * wheel.steer_scale
        } else {
            0.0
        };
        xf.rotation = Quat::from_rotation_y(-steer);
        if let Some(w) = cfg
            .original
            .as_ref()
            .and_then(|o| o.wheels.get(mount.index))
        {
            let inner_edge = w.side * w.width * 0.5;
            let pivot_shift = inner_edge * (xf.rotation * Vec3::X - Vec3::X);
            xf.translation += pivot_shift;
        }
        for child in children {
            if let Ok(mut spin) = spins.get_mut(*child) {
                spin.rotation = Quat::from_rotation_x(ws.spin);
            }
        }
    }
}

/// Toggle headlight glows (`L` as shipped, rebindable). Live phases only,
/// like the other in-session toggles: a paused rebinding page listening
/// for the new key must not also flip the lamps.
pub fn toggle_headlights(
    keys: Res<ButtonInput<KeyCode>>,
    pads: Query<&Gamepad>,
    windows: Query<&Window>,
    session: Res<mm2_game::Session>,
    controls: Option<Res<crate::controls::ControlSettings>>,
    mut on: ResMut<HeadlightsOn>,
) {
    if matches!(
        session.phase(),
        mm2_game::SessionPhase::Playing | mm2_game::SessionPhase::Countdown
    ) && crate::input::control_just_pressed(
        &keys,
        &pads,
        &windows,
        controls.as_deref(),
        crate::controls::DriveAction::Headlights,
    ) {
        on.0 = !on.0;
    }
}

/// Glow-quad visibility from the owning vehicle's state: brake lights on
/// pedal, reverse lights while reversing, head/tail lights on `L`, a
/// cop's light-bar flares alternating halves while its
/// [`EmergencyLights`] are on — or both halves held lit under the
/// reduced-flashing option, which has nothing alternate.
pub fn update_glows(
    lights: Res<HeadlightsOn>,
    settings: Option<Res<crate::settings::GraphicsSettings>>,
    vehicles: Query<(&VehicleState, &VehicleInput, Option<&EmergencyLights>)>,
    mut glows: Query<(&GlowPart, &mut Visibility, &ChildOf)>,
) {
    let steady = settings.is_some_and(|s| s.reduce_flashing);
    for (glow, mut vis, parent) in &mut glows {
        let Ok((state, input, emergency)) = vehicles.get(parent.parent()) else {
            continue;
        };
        let braking = input.brake > 0.05;
        let show = match glow.0 {
            GlowKind::Headlight | GlowKind::Taillight => lights.0,
            GlowKind::Brake => braking && state.direction == DriveDirection::Forward,
            GlowKind::Reverse => braking && state.direction == DriveDirection::Reverse,
            GlowKind::Siren(side) => emergency.is_some_and(|e| steady || e.lit_side() == side),
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

/// Turn every [`FlareSprite`] to the view the player sees through —
/// the HUD map's top-down camera is not that view — so a light-bar
/// flare reads as a shine from any angle (billboarding is render
/// policy: the authored quad is flat and its retail orientation
/// unrecovered). Only the rotation changes, so the attach offset and
/// the glow visibility toggle are untouched.
pub fn face_flares(
    cameras: Query<(&Camera, &GlobalTransform), crate::hudmap::WorldCamera3d>,
    parents: Query<&GlobalTransform, Without<FlareSprite>>,
    mut flares: Query<(&ChildOf, &mut Transform), With<FlareSprite>>,
) {
    let Some(view) = cameras
        .iter()
        .find(|(cam, _)| cam.is_active)
        .map(|(_, xf)| xf.compute_transform().rotation)
    else {
        return;
    };
    for (parent, mut xf) in &mut flares {
        let Ok(parent) = parents.get(parent.parent()) else {
            continue;
        };
        let local = parent.compute_transform().rotation.inverse() * view;
        if xf.rotation != local {
            xf.rotation = local;
        }
    }
}

/// Copy the towing vehicle's brake/handbrake onto the trailer so its
/// authored brake bias actually engages. The tractor is whatever entity
/// `Trailer::towing` names — the local player, an AI opponent or a
/// networked participant's authority-side car (F25-B); on a predicted
/// client the remote copy's snap-applied input feeds its trailer copy
/// the same way. The `Without<Trailer>` filter keeps the queries
/// provably disjoint — a trailer carries `VehicleInput` too.
pub fn trailer_input(
    cars: Query<&VehicleInput, Without<Trailer>>,
    mut trailers: Query<(&Trailer, &mut VehicleInput)>,
) {
    for (t, mut input) in &mut trailers {
        if let Ok(car_input) = cars.get(t.towing) {
            input.brake = car_input.brake;
            input.handbrake = car_input.handbrake;
        }
    }
}
