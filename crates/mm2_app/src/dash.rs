//! Authored cockpit/dashboard view (F22-B.1, HUD-1/HUD-3).
//!
//! Stock vehicles ship a complete interior rig:
//!
//! - `geometry/<id>_dash.pkg` — flat quad parts (`dash`, `roof`,
//!   `speed_needle`, `tach_needle`, `damage_needle`, `gear_indicator`,
//!   `wheel`) authored in one shared cluster space facing +Z, plus the
//!   usual per-part `.mtx` pivots.
//! - `tune/<id>_dash.asnode` — the gauge calibration record
//!   ([`DashSpec`]): cluster/roof/wheel placements, per-gauge offsets and
//!   pivot nudges, needle sweep radians and the steering-wheel factor.
//! - `tune/camera/<id>_dash.campovcs` — the `camPovCS` cockpit camera:
//!   eye `Offset`, `TrackTo`, `Pitch`, authored FOV/near/far and a
//!   `ReverseOffset` for the back-look.
//!
//! Composition (authored-space reading; anywhere the retail binary
//! resolves differently is recorded as UNK in `docs/original-rules.md`):
//! `DashPos`/`RoofPos` anchor the cluster and the roof card relative to
//! the eye position in car space, so an anchor node sits at
//! `pov_offset + field`. Needles and the wheel live *inside* the
//! cluster: each becomes a pivot node at
//! `authored_pivot + <Field>Offset + <Field>PivotOffset` with its mesh
//! children shifted so the authored quad renders `Offset` off its file
//! position and rotates about the authored pivot. The wheel composes
//! from `WheelPos`/`WheelPivotOffset` the same way. The gear indicator
//! is a static quad: the pkg repurposes its paint-job table as gear
//! slots (shader 4 names `R`, `N`, `One`…`Six`, `D` per job), so the
//! engaged gear selects the quad's material rather than moving it.
//!
//! The cockpit camera and the dash nodes are children of the vehicle
//! rigidly — matching the recovered `mmDashView` placement under the
//! car matrix — so they survive reset/finish transforms for free.
//! Exterior children of the vehicle hide while the cockpit view is
//! active (the original draws the interior from the dash package
//! instead).

use std::fmt;

use bevy::prelude::*;

use mm2_assets::Vfs;
use mm2_content::model::{ModelPart, VehicleModel, build_model};
use mm2_formats::{
    dash::{DashSpec, PovCamSpec},
    mtx::Mtx,
    pkg::Pkg,
};
use mm2_game::{PlayerVehicle, Session, SessionEntity, SessionPhase, VehicleDamage};
use mm2_vehicle::{DriveDirection, Vehicle, VehicleState};

use crate::{
    camera::CameraMode,
    car_visual::{GlowPart, group_material, group_mesh},
    city::MaterialCache,
};

/// The authored `camPovCS` cockpit camera — a rigid child of the
/// vehicle. Numpad look keys (HUD-3) drive `look_yaw`; `Numpad2`
/// additionally swaps in `ReverseOffset` (the authored back-look seat
/// position). The look magnitudes are designed — the record carries
/// the offsets, not the glance angles (DSN-49).
#[derive(Component)]
pub struct CockpitCamera {
    /// Eye position in car space (authored `Offset`).
    pub offset: Vec3,
    /// Authored `ReverseOffset`, when the record carries one.
    pub reverse_offset: Option<Vec3>,
    /// Authored static `Pitch` (radians).
    pub pitch: f32,
    /// Current look-yaw offset applied on top of `pitch` (radians).
    pub look_yaw: f32,
}

/// A vehicle-child subtree that renders only while `CameraMode::Cockpit`
/// is active — the dash cluster, the roof card and the cockpit camera
/// itself. `sync_dash_visibility` flips these against every other
/// direct child of the vehicle.
#[derive(Component)]
pub struct CockpitPart;

/// A vehicle child `sync_dash_visibility` hid for the cockpit view.
/// The tag is the split's ownership claim: only tagged children are
/// restored when the mode leaves Cockpit — a child another system hid
/// (a detached `BreakPartVisual`, an unlit `GlowPart`) is never tagged,
/// and an owner that hides a tagged child drops the tag so the split
/// never re-shows it.
#[derive(Component)]
pub struct CockpitHidden;

/// Root of the authored dash cluster (`DashPos`-anchored).
#[derive(Component)]
pub struct DashRoot;

/// A dashboard node driven from authoritative vehicle state each frame.
#[derive(Component)]
pub struct DashNode {
    pub role: DashRole,
}

/// Which authored gauge a [`DashNode`] drives.
#[derive(Debug)]
pub enum DashRole {
    /// Speedometer needle — authored sweep radians `SpeedRotMin/Max`.
    Speed { min: f32, max: f32 },
    /// Tachometer needle — authored sweep radians `RPMRotMin/Max`.
    Tach { min: f32, max: f32 },
    /// Damage needle — authored sweep radians `DamageRotMin/Max`.
    Damage { min: f32, max: f32 },
    /// Steering wheel — the quad rolls by the fraction of steering
    /// lock times `WheelFact` (authored, ≈0.9 stock) half-turns
    /// (designed mapping — the field's unit is unrecovered, UNK-28).
    Wheel { factor: f32 },
}

/// The `gear_indicator` quad's gear-glyph materials. The dash pkg's
/// paint-job table is repurposed as a gear slot table: shader 4 of
/// paint job *n* names a per-slot texture (`R`, `N`, `One`…`Six`, `D`
/// for all stock dashes), so `drive_dash` swaps this entity's material
/// to the slot matching the engaged gear — the same mechanism the
/// original's per-paint-job shader table encodes.
#[derive(Component)]
pub struct GearGlyph {
    /// One material per gear slot, indexed by the pkg paint job.
    pub materials: Vec<Handle<StandardMaterial>>,
    /// The slot currently bound (index into `materials`).
    pub slot: usize,
}

/// Per-spawn outcome for the smoke record — same contract as
/// `HudMapReport`: bound counts on success, `absent:<why>` when no
/// authored data gates in.
#[derive(Resource, Debug, Default)]
pub struct DashReport {
    /// Renderable part nodes bound from the dash pkg.
    pub parts: usize,
    /// A cockpit camera was spawned from authored `camPovCS`.
    pub cockpit: bool,
    /// Why nothing spawned (empty when a dash exists).
    pub absent: Option<String>,
}

impl fmt::Display for DashReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.absent {
            Some(why) => write!(f, "absent:{why}"),
            None => write!(
                f,
                "{}p/{}",
                self.parts,
                if self.cockpit { "cam" } else { "nocam" }
            ),
        }
    }
}

/// Look-key yaw targets (radians): numpad 4/6 glance left/right, 8
/// faces forward, 2 looks back through `ReverseOffset`. Held, not
/// latched — releasing eases back to the authored eye pose. The
/// magnitudes are designed; only the offsets are authored (DSN-49).
const LOOK_SIDE: f32 = std::f32::consts::FRAC_PI_2;
const LOOK_LERP: f32 = 10.0;

fn read_text(vfs: &Vfs, logical: &str) -> Option<String> {
    let (bytes, _) = vfs.read_path(logical).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// Read and parse `tune/camera/<car>_dash.campovcs`, when the record
/// exists — the authored `camPovCS` eye [`spawn_dash`] binds. Resolved
/// ahead of the session cameras so `CameraMode::Cockpit` can fall back
/// before any camera spawns inactive (a dashless car, the dev car or a
/// `Cockpit` mode persisted across a reload has no camera otherwise).
pub fn load_pov_cam(vfs: &Vfs, car: &str) -> Option<PovCamSpec> {
    read_text(vfs, &format!("tune/camera/{car}_dash.campovcs"))
        .and_then(|t| PovCamSpec::parse(&t).ok())
}

fn v3(f: Option<[f32; 3]>) -> Vec3 {
    f.map(Vec3::from).unwrap_or(Vec3::ZERO)
}

/// Load and spawn the authored cockpit rig under `vehicle`.
///
/// Three independent authored gates: the camera needs the `camPovCS`
/// record (`pov`, resolved by the caller — see [`load_pov_cam`]), the
/// cluster needs both the `_dash.asnode` placement record and the
/// `_dash.pkg` geometry. Missing pieces degrade the report rather than
/// fabricating a stand-in — a car with only the camera record still
/// gets the authored eye position.
#[allow(clippy::too_many_arguments)]
pub fn spawn_dash(
    commands: &mut Commands,
    vfs: &Vfs,
    car: &str,
    paint: usize,
    pov: Option<PovCamSpec>,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
    vehicle: Entity,
    owner: SessionEntity,
    cam_mode: CameraMode,
    fog: Option<bevy::pbr::DistanceFog>,
) -> DashReport {
    let mut report = DashReport::default();

    let spec =
        read_text(vfs, &format!("tune/{car}_dash.asnode")).and_then(|t| DashSpec::parse(&t).ok());
    let model = vfs
        .read_path(&format!("geometry/{car}_dash.pkg"))
        .ok()
        .and_then(|(bytes, _)| Pkg::parse(&bytes).ok())
        .map(|pkg| {
            build_model(&pkg, |stem| {
                vfs.read_logical(&format!("geometry/{car}_dash_{stem}.mtx"))
                    .ok()
                    .and_then(|b| Mtx::parse(&b).ok())
            })
        });

    let eye = pov.as_ref().map(|p| v3(p.offset)).unwrap_or(Vec3::ZERO);
    let active = cam_mode == CameraMode::Cockpit;

    if let Some(p) = &pov {
        let pitch = p.pitch.unwrap_or(0.0);
        let cam = commands
            .spawn((
                owner,
                CockpitPart,
                CockpitCamera {
                    offset: eye,
                    reverse_offset: p.reverse_offset.map(Vec3::from),
                    pitch,
                    look_yaw: 0.0,
                },
                Camera3d::default(),
                Camera {
                    is_active: active,
                    ..default()
                },
                Projection::Perspective(PerspectiveProjection {
                    fov: p.camera_fov.unwrap_or(60.0).to_radians(),
                    near: p.camera_near.unwrap_or(0.1).max(0.01),
                    far: p.camera_far.unwrap_or(600.0).max(1.0),
                    ..default()
                }),
                Transform::from_translation(eye).with_rotation(Quat::from_rotation_x(pitch)),
            ))
            .id();
        if let Some(f) = fog {
            commands.entity(cam).insert(f);
        }
        commands.entity(vehicle).add_child(cam);
        report.cockpit = true;
    }

    match (spec, model) {
        (Some(spec), Some(model)) => {
            let mut mats = MaterialCache::new(vfs, images, materials);
            // Cluster space = authored dash space anchored `DashPos`
            // relative to the eye; the roof card anchors `RoofPos` the
            // same way (its verts are authored around an overhead
            // origin).
            let cluster = commands
                .spawn((
                    owner,
                    CockpitPart,
                    DashRoot,
                    Transform::from_translation(eye + v3(spec.dash_pos)),
                    Visibility::Visible,
                ))
                .id();
            commands.entity(vehicle).add_child(cluster);
            let roof_root = commands
                .spawn((
                    owner,
                    CockpitPart,
                    Transform::from_translation(eye + v3(spec.roof_pos)),
                    Visibility::Visible,
                ))
                .id();
            commands.entity(vehicle).add_child(roof_root);
            let rot = |r: Option<(f32, f32)>| r.unwrap_or((0.0, 0.0));
            let (speed_min, speed_max) = rot(spec.speed_rot);
            let (rpm_min, rpm_max) = rot(spec.rpm_rot);
            let (dmg_min, dmg_max) = rot(spec.damage_rot);
            for part in &model.parts {
                let pivot = part.origin.map(Vec3::from).unwrap_or(Vec3::ZERO);
                let (parent, node_pos, child_off, role) = match part.name.as_str() {
                    "speed_needle" => (
                        cluster,
                        pivot + v3(spec.speed_offset) + v3(spec.speed_pivot_offset),
                        -(pivot + v3(spec.speed_pivot_offset)),
                        Some(DashRole::Speed {
                            min: speed_min,
                            max: speed_max,
                        }),
                    ),
                    "tach_needle" => (
                        cluster,
                        pivot + v3(spec.tach_offset) + v3(spec.tach_pivot_offset),
                        -(pivot + v3(spec.tach_pivot_offset)),
                        Some(DashRole::Tach {
                            min: rpm_min,
                            max: rpm_max,
                        }),
                    ),
                    "damage_needle" => (
                        cluster,
                        pivot + v3(spec.dmg_offset) + v3(spec.dmg_pivot_offset),
                        -(pivot + v3(spec.dmg_pivot_offset)),
                        Some(DashRole::Damage {
                            min: dmg_min,
                            max: dmg_max,
                        }),
                    ),
                    "wheel" => (
                        cluster,
                        pivot + v3(spec.wheel_pos) + v3(spec.wheel_pivot_offset),
                        -(pivot + v3(spec.wheel_pivot_offset)),
                        Some(DashRole::Wheel {
                            factor: spec.wheel_fact.unwrap_or(1.0),
                        }),
                    ),
                    "roof" => (roof_root, Vec3::ZERO, Vec3::ZERO, None),
                    // `gear_indicator` is a static quad — `drive_dash`
                    // swaps its material across the pkg's repurposed
                    // paint-job gear slots (`GearGlyph`).
                    _ => (cluster, Vec3::ZERO, Vec3::ZERO, None),
                };
                let pivot_node = commands
                    .spawn((
                        owner,
                        CockpitPart,
                        Transform::from_translation(node_pos),
                        Visibility::Visible,
                    ))
                    .id();
                commands.entity(parent).add_child(pivot_node);
                if let Some(role) = role {
                    commands.entity(pivot_node).insert(DashNode { role });
                }
                report.parts += spawn_part_meshes(
                    commands,
                    pivot_node,
                    &model,
                    paint,
                    part,
                    child_off,
                    &mut mats,
                    meshes,
                    owner,
                    part.name == "gear_indicator",
                );
            }
            let missing: Vec<String> = mats.missing_textures().iter().cloned().collect();
            if !missing.is_empty() {
                warn!(car = %car, "dash missing textures: {}", missing.join(", "));
            }
        }
        (spec_r, model_r) => {
            if !report.cockpit {
                report.absent = Some(match (spec_r.is_some(), model_r.is_some()) {
                    (false, false) => "missing-spec+pkg".into(),
                    (false, true) => "missing-asnode".into(),
                    (true, false) => "missing-pkg".into(),
                    (true, true) => unreachable!(),
                });
            }
        }
    }
    report
}

/// Spawn the part's best-LOD mesh groups under `parent` at `child_off`.
/// `gear` marks the `gear_indicator` part: each mesh also gets a
/// [`GearGlyph`] carrying the material for every gear slot (the pkg's
/// paint-job index selects the glyph texture — `R`, `N`, `One`…`D`).
#[allow(clippy::too_many_arguments)]
fn spawn_part_meshes(
    commands: &mut Commands,
    parent: Entity,
    model: &VehicleModel,
    paint: usize,
    part: &ModelPart,
    child_off: Vec3,
    mats: &mut MaterialCache,
    meshes: &mut Assets<Mesh>,
    owner: SessionEntity,
    gear: bool,
) -> usize {
    let Some(groups) = part.best_lod() else {
        return 0;
    };
    let holder = commands
        .spawn((
            owner,
            CockpitPart,
            Transform::from_translation(child_off),
            Visibility::Visible,
        ))
        .id();
    commands.entity(parent).add_child(holder);
    let mut n = 0;
    for g in groups {
        if g.positions.len() < 3 {
            continue;
        }
        let slot_mat = group_material(model, paint, g.shader_offset, mats, false);
        let mesh = commands
            .spawn((
                owner,
                CockpitPart,
                Mesh3d(meshes.add(group_mesh(g, None))),
                MeshMaterial3d(slot_mat),
                Transform::default(),
            ))
            .id();
        if gear {
            let materials = (0..model.paint_jobs)
                .map(|p| group_material(model, p, g.shader_offset, mats, false))
                .collect();
            commands.entity(mesh).insert(GearGlyph {
                materials,
                slot: usize::MAX,
            });
        }
        commands.entity(holder).add_child(mesh);
        n += 1;
    }
    n
}

/// Cockpit visibility split: while `CameraMode::Cockpit` is active the
/// exterior children of the player vehicle hide and the authored
/// cockpit subtrees show; every other mode restores the split.
///
/// The restore side only touches what this system hid: a non-cockpit
/// child this sweep turns `Hidden` earns a [`CockpitHidden`] tag, and a
/// non-cockpit frame restores `Visible` on tagged children alone. A
/// child already `Hidden` when the sweep reaches it — a detached
/// `BreakPartVisual` node, an unlit `GlowPart` — belongs to its owner
/// and is left alone, so the default Chase sweep can never re-show a
/// detached panel on top of its fragment. `GlowPart` carriers are never
/// tagged at all: `update_glows` rewrites them from vehicle state every
/// frame, so they need no restore (the schedule orders this system
/// after it, so a lit lamp stays hidden inside the cockpit). A system
/// that takes over a tagged child's `Hidden` — `detach_breaks` does —
/// removes the tag, transferring ownership.
///
/// Only direct children are flipped — visibility propagates down each
/// subtree — and the check is a write-on-diff sweep so freshly spawned
/// children are caught even when the mode never changed (`--cockpit`
/// at spawn).
pub fn sync_dash_visibility(
    mut commands: Commands,
    mode: Res<CameraMode>,
    players: Query<&Children, With<PlayerVehicle>>,
    cockpits: Query<(), With<CockpitPart>>,
    glows: Query<(), With<GlowPart>>,
    hidden: Query<(), With<CockpitHidden>>,
    mut vis: Query<&mut Visibility>,
) {
    let cockpit = *mode == CameraMode::Cockpit;
    for children in &players {
        for child in children.iter() {
            let Ok(mut v) = vis.get_mut(child) else {
                continue;
            };
            if cockpits.get(child).is_ok() {
                // The dash subtree is wholly this system's.
                let next = if cockpit {
                    Visibility::Visible
                } else {
                    Visibility::Hidden
                };
                if *v != next {
                    *v = next;
                }
            } else if cockpit {
                // Exterior under the cockpit view: hide whatever is
                // showing, tagging it so leaving the mode can tell this
                // `Hidden` from one an owner wrote.
                if *v != Visibility::Hidden {
                    *v = Visibility::Hidden;
                    if glows.get(child).is_err() {
                        commands.entity(child).insert(CockpitHidden);
                    }
                }
            } else if hidden.get(child).is_ok() {
                // Restore only what this system hid.
                commands.entity(child).remove::<CockpitHidden>();
                if *v != Visibility::Visible {
                    *v = Visibility::Visible;
                }
            }
        }
    }
}

/// Drive the authored instruments from authoritative vehicle state:
/// speed needle from `forward_speed` over the authored `Trans.High`
/// top speed, tach from `rpm` over redline, damage from the authored
/// health fraction, the gear glyph by rebinding the indicator quad to
/// the engaged gear's slot material, and the wheel from the *simulated*
/// steer angle (post-smoothing) scaled to `WheelFact` half-turns at
/// full lock. Missing authored scales park the needle at `min` rather
/// than fabricating one.
pub fn drive_dash(
    player: Query<(&Vehicle, &VehicleState, Option<&VehicleDamage>), With<PlayerVehicle>>,
    mut nodes: Query<(&mut DashNode, &mut Transform)>,
    mut gears: Query<(&mut GearGlyph, &mut MeshMaterial3d<StandardMaterial>)>,
) {
    let Ok((veh, state, dmg)) = player.single() else {
        return;
    };
    for (mut node, mut xf) in &mut nodes {
        match &mut node.role {
            DashRole::Speed { min, max } => {
                let frac = veh
                    .config
                    .top_speed_mps
                    .filter(|m| *m > 0.0)
                    .map(|m| (state.forward_speed.abs() / m).clamp(0.0, 1.0))
                    .unwrap_or(0.0);
                xf.rotation = Quat::from_rotation_z(*min + (*max - *min) * frac);
            }
            DashRole::Tach { min, max } => {
                let redline = veh.config.engine.redline_rpm.max(1.0);
                let frac = (state.rpm / redline).clamp(0.0, 1.0);
                xf.rotation = Quat::from_rotation_z(*min + (*max - *min) * frac);
            }
            DashRole::Damage { min, max } => {
                let frac = dmg
                    .map(|d| 1.0 - d.health_fraction())
                    .unwrap_or(0.0)
                    .clamp(0.0, 1.0);
                xf.rotation = Quat::from_rotation_z(*min + (*max - *min) * frac);
            }
            DashRole::Wheel { factor } => {
                let lock = veh.config.steering.low_speed_max_angle.max(1e-3);
                xf.rotation = Quat::from_rotation_z(
                    -(state.steer_angle / lock).clamp(-1.0, 1.0) * *factor * std::f32::consts::PI,
                );
            }
        }
    }
    // Authored slot order is uniform across stock dashes: `R`, `N`,
    // `One`…`Six`, `D`. The sim never rests in neutral (`direction` is
    // always engaged), so `N`/`D` have no authored trigger on our
    // telemetry — forward gear g (0-based) binds slot g+2, clamped to
    // the table (UNK-27 records what stays unrecovered).
    let want = match state.direction {
        DriveDirection::Reverse => 0,
        DriveDirection::Forward => state.gear + 2,
    };
    for (mut glyph, mut mat) in &mut gears {
        let want = want.min(glyph.materials.len().saturating_sub(1));
        if glyph.slot != want {
            glyph.slot = want;
            if let Some(h) = glyph.materials.get(want) {
                mat.0 = h.clone();
            }
        }
    }
}

/// Numpad cockpit look (HUD-3): 4/6 glance sideways, 8 forward, 2
/// looks back through the authored `ReverseOffset`. Held, not latched
/// — releasing returns to the authored eye pose.
pub fn cockpit_look(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mode: Res<CameraMode>,
    session: Res<Session>,
    mut cams: Query<(&mut CockpitCamera, &mut Transform)>,
) {
    if *mode != CameraMode::Cockpit || !matches!(session.phase(), SessionPhase::Playing) {
        return;
    }
    for (mut cam, mut xf) in &mut cams {
        let (yaw, back) = if keys.pressed(KeyCode::Numpad2) {
            (std::f32::consts::PI, true)
        } else if keys.pressed(KeyCode::Numpad4) {
            (LOOK_SIDE, false)
        } else if keys.pressed(KeyCode::Numpad6) {
            (-LOOK_SIDE, false)
        } else {
            (0.0, false)
        };
        let t = 1.0 - (-LOOK_LERP * time.delta_secs()).exp();
        cam.look_yaw += (yaw - cam.look_yaw) * t;
        let pos = if back {
            cam.reverse_offset.unwrap_or(cam.offset)
        } else {
            cam.offset
        };
        xf.translation = pos;
        xf.rotation = Quat::from_rotation_y(cam.look_yaw) * Quat::from_rotation_x(cam.pitch);
    }
}
