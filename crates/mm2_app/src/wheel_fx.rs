//! F18-B.4: wheel surface particles — the `ptxindex`/`ptxthreshold`
//! leg of F18-B's requirement 3.
//!
//! `materials.mtl` authors two wheel-particle channels per surface
//! (`ptxindex`/`ptxthreshold`); `ptxindex` is a positional selector
//! into the retail `ptx_wheel` effect-name table the exe carries as a
//! contiguous string block (`mm2_game::PTX_RULE_NAMES`), each name
//! binding `tune/effects/<name>.asbirthrule` and sprites from the
//! `texture/ptx_wheel` atlas. `mm2_game::effects` owns the contract:
//! [`mm2_game::WheelPtx`] is the per-vehicle rig (deterministic
//! `NavRng` per wheel, [`mm2_game::WheelPtxPolicy`]) and
//! [`mm2_game::WheelPuff`] the bounded live particle. This module
//! loads the authored rules and atlas through the VFS at session
//! bind, feeds grounded-wheel contacts through [`SurfaceTables`]'s
//! `ptx_channels` lookup and renders each puff as a camera-facing
//! atlas tile.
//!
//! Authored: the field values, the index→name table, the rule files
//! and the atlas. Designed (DSN-62): the gate quantity (the wheel's
//! `tire_slippage` utilization — the same measure skid audio reads),
//! the strict-`>` comparison, the `InitialBlast`-on-edge / `SpewRate`-
//! while-held reading, the per-vehicle pool bound and the billboard
//! presentation. The original trigger quantity and emission cadence
//! are unrecovered (UNK-23) — a threshold-0 channel (retail `water`)
//! still needs nonzero tire work, so a parked wheel stays dark.

use std::collections::HashMap;

use bevy::prelude::*;
use tracing::warn;

use mm2_assets::Vfs;
use mm2_content::SurfaceTables;
use mm2_formats::banger::BirthRule;
use mm2_game::{
    ObjectIdentity, PTX_ATLAS_TILES, PTX_RULE_NAMES, ParticleSpec, Player, PlayerControl, Session,
    SessionEntity, SurfaceMaterial, WheelDraw, WheelPtx, WheelPtxPolicy, WheelPuff, tire_slippage,
};
use mm2_vehicle::{Vehicle, VehicleState};

use crate::city;

/// `texture/ptx_wheel` — the wheel-effect atlas: a measured 8×8 grid
/// of tiles ([`PTX_ATLAS_TILES`]); the authored `TexFrame*` indexes
/// select its tiles. The exe names it immediately ahead of the effect
/// table — the binding is authored (DSN-62 records the presentation
/// reading).
const WHEEL_TEXTURE: &str = "ptx_wheel";

/// Render assets for the puffs — built once per session.
pub struct WheelFxAssets {
    /// One 1×1 quad mesh per `ptx_wheel` tile, UVs baked — the same
    /// `tile_quad` n×n policy the smoke/precip atlases apply.
    pub quads: Vec<Handle<Mesh>>,
    /// Base material — cloned per puff so [`WheelPuff::alpha`]
    /// animates one sprite, not the pool.
    pub material: Handle<StandardMaterial>,
}

/// Session-scoped wheel-particle state — inserted by
/// `load_session_world`, removed on teardown. Absent in tests that
/// never load a world; the systems simply no-op.
#[derive(Resource)]
pub struct WheelFx {
    /// The authored spec per `PTX_RULE_NAMES` index — `None` where the
    /// VFS could not deliver a parseable record (each miss counted
    /// once on [`WheelFxReport::failed`]; the slot stays dark at run
    /// time, never substituted).
    pub specs: Vec<Option<ParticleSpec>>,
    pub assets: WheelFxAssets,
}

/// Session-scoped evidence the headless record reports (`wfx=`).
/// Inserted on every `load_session_world`, reset on teardown.
#[derive(Resource, Default, Debug)]
pub struct WheelFxReport {
    /// Rules the bind loaded — retail resolves all eight.
    pub loaded: usize,
    /// Rules the VFS could not deliver or could not parse — each
    /// named once in the log, counted here (F18-AC06). Never silently
    /// mapped to a substitute effect.
    pub failed: usize,
    /// Whether `texture/ptx_wheel` resolved (untextured puffs still
    /// render — the loader's missing-texture policy).
    pub texture: bool,
    /// Puffs spawned this session.
    pub emitted: u64,
    /// Puffs expired on `Life` and despawned.
    pub expired: u64,
    /// Puffs the per-vehicle pool bound discarded.
    pub dropped: u64,
}

impl WheelFxReport {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Clear the counters on teardown — the same unload-reset contract
/// the other session evidence reports hold
/// (`run_if(crate::session::unloading)`); the next load re-binds
/// `loaded`/`failed`/`texture` from its own VFS.
pub fn reset_wheel_fx_report(mut report: ResMut<WheelFxReport>) {
    report.reset();
}

/// The `tune/effects/<name>.asbirthrule` path a `PTX_RULE_NAMES`
/// entry binds — the exe's `tune/effects` string is the evidence the
/// directory is authored.
fn rule_path(name: &str) -> String {
    format!("tune/effects/{name}.asbirthrule")
}

/// Bind the session's wheel-effect table: every `PTX_RULE_NAMES`
/// entry's rule resolves through the VFS and parses into a
/// [`ParticleSpec`], then the `ptx_wheel` atlas builds the sprite
/// assets. Each rule miss warns once and marks `failed` — the slot
/// stays dark rather than borrowing another effect (F18-AC06). The
/// resource always binds: a city with no surface tables simply never
/// feeds it a channel.
pub fn wheel_fx_session(
    vfs: &Vfs,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
) -> (WheelFxReport, WheelFx) {
    let mut report = WheelFxReport::default();
    let specs = PTX_RULE_NAMES
        .iter()
        .map(|name| load_rule(vfs, name, &mut report))
        .collect();
    let assets = wheel_fx_assets(vfs, meshes, images, materials, &mut report);
    (report, WheelFx { specs, assets })
}

/// One `tune/effects/<name>.asbirthrule` → [`ParticleSpec`] — `None`
/// (counted) on any resolve/read/parse failure.
fn load_rule(vfs: &Vfs, name: &'static str, report: &mut WheelFxReport) -> Option<ParticleSpec> {
    let path = rule_path(name);
    let fail = |report: &mut WheelFxReport, why: &str| {
        warn!(rule = name, path = %path, "wheel effect {why}");
        report.failed += 1;
        None
    };
    let Some(resolved) = vfs.resolve(&path) else {
        return fail(report, "unavailable");
    };
    let bytes = match vfs.read(&resolved) {
        Ok(b) => b,
        Err(e) => {
            warn!(rule = name, error = %e, "wheel effect unreadable");
            report.failed += 1;
            return None;
        }
    };
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return fail(report, "is not text");
    };
    let parsed = match BirthRule::parse_file(text) {
        Ok(p) => p,
        Err(e) => {
            warn!(rule = name, error = %e, "wheel effect unparseable");
            report.failed += 1;
            return None;
        }
    };
    for w in &parsed.warnings {
        warn!(rule = name, note = %w, "wheel effect rule note");
    }
    report.loaded += 1;
    // `From<&StandaloneBirthRule>` keeps the effects-superset fields
    // (`Damp`/`Height`/`Intensity`/`Color`) authored.
    Some(ParticleSpec::from(&parsed))
}

/// Build the sprite assets for one session — `ptx_wheel` resolves
/// through `city::load_image` so mods can override it like any
/// texture. A missing/undecodable atlas warns and falls back to an
/// untextured material — the same missing-texture policy the rest of
/// the loader applies; it never sinks the session.
fn wheel_fx_assets(
    vfs: &Vfs,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
    report: &mut WheelFxReport,
) -> WheelFxAssets {
    let texture = city::load_image(vfs, WHEEL_TEXTURE).map(|(img, _)| images.add(img));
    report.texture = texture.is_some();
    if !report.texture {
        warn!(
            texture = WHEEL_TEXTURE,
            "wheel atlas unresolved — untextured puffs"
        );
    }
    let material = materials.add(StandardMaterial {
        base_color_texture: texture,
        // Unlit blended quads like the smoke/precip sprites — lit
        // puffs would pick up scene lighting the authored tint never
        // accounted for.
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        cull_mode: None,
        ..default()
    });
    let quads = (0..PTX_ATLAS_TILES * PTX_ATLAS_TILES)
        .map(|t| meshes.add(crate::damage_fx::tile_quad(t, PTX_ATLAS_TILES)))
        .collect();
    WheelFxAssets { quads, material }
}

/// Emit surface puffs from every rigged participant's grounded wheels
/// (F18-B.4). Per wheel the contact's [`SurfaceMaterial`] resolves
/// through [`SurfaceTables::ptx_channels`] to the authored channel
/// pair; the designed gate quantity is the wheel's `tire_slippage`
/// utilization — the same measure the skid voices read — so the
/// authored `ptxthreshold` applies on the same 0..1 domain a slip
/// trigger plausibly reads (the original quantity is unrecovered,
/// UNK-23). Remote participants are skipped like every effect system —
/// their presentation is replication scope (F25-B/F26), and a client's
/// kinematic copy runs no wheel sim to read. A vehicle without a rig
/// yet binds
/// one seeded off its object id — session-stable, so emission replays
/// identically (F18 req 5).
#[allow(clippy::too_many_arguments, clippy::type_complexity)] // Bevy system — the borrows are the contract.
pub fn emit_wheel_fx(
    mut commands: Commands,
    time: Res<Time>,
    session: Res<Session>,
    fx: Option<Res<WheelFx>>,
    tables: Option<Res<SurfaceTables>>,
    mut report: ResMut<WheelFxReport>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut cars: Query<(
        Entity,
        &Vehicle,
        &VehicleState,
        Option<&mut WheelPtx>,
        Option<&ObjectIdentity>,
        Option<&Player>,
    )>,
    collider_surfaces: Query<&SurfaceMaterial>,
    puffs: Query<&WheelPuff>,
    cameras: Query<(&Camera, &GlobalTransform), crate::hudmap::WorldCamera3d>,
) {
    if !session.is_playing() {
        return;
    }
    let (Some(fx), Some(tables)) = (fx, tables) else {
        return;
    };
    let Some(base) = materials.get(&fx.assets.material).cloned() else {
        return;
    };
    let dt = time.delta_secs();
    let owner = SessionEntity(session.generation());
    let focus = crate::precip::camera_focus(&cameras);
    let spec_of = |i: i64| fx.specs.get(i as usize).and_then(Option::as_ref);
    // Live puffs per emitter — the query cannot see this frame's
    // deferred spawns, so the count accrues locally or a burst-heavy
    // tick could overrun the designed pool bound (the spark rig's
    // pattern).
    let mut live: HashMap<Entity, usize> = HashMap::new();
    for (car, vehicle, state, rig_slot, identity, player) in &mut cars {
        if player.is_some_and(|p| p.control == PlayerControl::Remote) {
            continue;
        }
        let Some(mut rig) = rig_slot else {
            // First sight — bind the rig. Every session vehicle
            // carries an `ObjectIdentity`; without one there is no
            // session-stable seed, so the car stays dark rather than
            // emitting from an unreplicable stream.
            let Some(id) = identity else { continue };
            commands.entity(car).insert(WheelPtx::new(
                WheelPtxPolicy::default(),
                (id.0.generation << 32) | id.0.slot as u64,
                state.wheels.len(),
            ));
            continue;
        };
        let mut live_now = *live
            .entry(car)
            .or_insert_with(|| puffs.iter().filter(|p| p.emitter == car).count());
        for (i, w) in state.wheels.iter().enumerate() {
            if !w.grounded {
                // Airborne closes the gates — a reground re-fires the
                // authored `InitialBlast` even on the same surface.
                rig.release(i);
                continue;
            }
            let material = w
                .contact_entity
                .and_then(|e| collider_surfaces.get(e).ok())
                .copied()
                .unwrap_or_default();
            let channels = tables.ptx_channels(material);
            let tires = vehicle
                .config
                .wheels
                .get(i)
                .and_then(|c| c.tires.as_ref())
                .unwrap_or(&vehicle.config.tires);
            let q = tire_slippage(
                w.traction_demand,
                w.slip_angle,
                tires.peak_slip_angle,
                tires.peak_slip_ratio,
            );
            let emission = rig.draw(
                i,
                WheelDraw {
                    dt,
                    q,
                    channels,
                    spec_of: &spec_of,
                    origin: w.contact_point,
                    normal: w.contact_normal,
                    live: live_now,
                    emitter: car,
                },
            );
            live_now += emission.puffs.len();
            report.dropped += emission.dropped as u64;
            for puff in emission.puffs {
                let quad =
                    fx.assets.quads[(puff.frame() as usize).min(fx.assets.quads.len() - 1)].clone();
                let mut material = base.clone();
                let [r, g, b] = puff.rgb();
                material.base_color = Color::srgba(r, g, b, puff.alpha());
                let mut transform = Transform::from_translation(puff.position)
                    .with_scale(Vec3::splat((puff.radius * 2.0).max(0.0)));
                if let Some(f) = focus {
                    transform.look_at(f, Vec3::Y);
                }
                commands.spawn((
                    owner,
                    puff,
                    Mesh3d(quad),
                    MeshMaterial3d(materials.add(material)),
                    transform,
                ));
                report.emitted += 1;
            }
        }
        live.insert(car, live_now);
    }
}

/// Integrate live puffs and sync their sprites — position, growth,
/// camera-facing billboard plus the authored `DRotation` roll, the
/// authored flipbook tile and per-puff `Color`/`Intensity` tint+alpha.
/// Expired puffs despawn on `Life` — the bound is the authored field,
/// not a leak. Session-stamped, so teardown takes any stragglers.
#[allow(clippy::too_many_arguments, clippy::type_complexity)] // Bevy system — the borrows are the contract.
pub fn advance_wheel_fx(
    mut commands: Commands,
    time: Res<Time>,
    session: Res<Session>,
    fx: Option<Res<WheelFx>>,
    mut report: ResMut<WheelFxReport>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut puffs: Query<(
        Entity,
        &mut WheelPuff,
        &mut Transform,
        &mut Mesh3d,
        &MeshMaterial3d<StandardMaterial>,
    )>,
    cameras: Query<(&Camera, &GlobalTransform), crate::hudmap::WorldCamera3d>,
) {
    if !session.is_playing() {
        return;
    }
    let dt = time.delta_secs();
    // Billboards face the view the player sees through — the same
    // `WorldCamera3d` pick the smoke sprites make.
    let cam_rot = cameras
        .iter()
        .find(|(cam, _)| cam.is_active)
        .map(|(_, xf)| xf.compute_transform().rotation);
    for (entity, mut puff, mut xf, mut mesh, material) in &mut puffs {
        if !puff.advance(dt) {
            commands.entity(entity).despawn();
            report.expired += 1;
            continue;
        }
        xf.translation = puff.position;
        xf.scale = Vec3::splat((puff.radius * 2.0).max(0.0));
        if let Some(rot) = cam_rot {
            xf.rotation = rot * Quat::from_rotation_z(puff.rotation);
        }
        if let Some(fx) = fx.as_ref() {
            mesh.0 =
                fx.assets.quads[(puff.frame() as usize).min(fx.assets.quads.len() - 1)].clone();
        }
        if let Some(mut mat) = materials.get_mut(&material.0) {
            let [r, g, b] = puff.rgb();
            mat.base_color = Color::srgba(r, g, b, puff.alpha());
        }
    }
}
