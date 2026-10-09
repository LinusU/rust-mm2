//! F18-B.2: authored precipitation — the `tune/<name>.asbirthrule` +
//! `texture/ptx_<name>` leg of F18-B's requirement 2.
//!
//! The effective weather selector binds a standalone particle spec by
//! name (`mm2_game::Weather::precipitation` — designed, DSN-60: retail
//! ships `tune/rain.asbirthrule`, `tune/snow.asbirthrule` and
//! `texture/ptx_rain.tex`, and the executable's `ptx_%s`/`asParticles`
//! strings show the shape of the original runtime without recovering
//! its semantics — UNK-40). `mm2_game::effects` owns the contract:
//! [`mm2_game::Precipitation`] is the session-owned bounded emitter
//! and [`mm2_game::PrecipDrop`] the live drop. This module resolves
//! the rule and atlas through the VFS, anchors emission on the active
//! `Camera3d` view, suppresses spawns the world shelters, and kills
//! drops on world contact so rain cannot fall through covered
//! interiors or the city floor (declared approximations — the upward
//! cover probe and the segment contact probe are designed, not
//! recovered original behavior).

use bevy::prelude::*;
use tracing::warn;

use avian3d::prelude::SpatialQuery;

use mm2_assets::Vfs;
use mm2_formats::banger::BirthRule;
use mm2_game::{ParticleSpec, PrecipDrop, Precipitation, Session, SessionEntity, Weather};

use crate::city;

/// The `asbirthrule` and particle texture bind by the same name —
/// `tune/rain.asbirthrule` pairs with `texture/ptx_rain` (the exe's
/// `ptx_%s` pattern is the evidence; DSN-60).
fn rule_path(name: &str) -> String {
    format!("tune/{name}.asbirthrule")
}

fn texture_stem(name: &str) -> String {
    format!("ptx_{name}")
}

/// Upward cover-probe distance in metres — a spawn candidate whose
/// sky is blocked within this reach counts as covered. Reads bridges,
/// tunnels and interiors; documented approximation (F18-B req 2).
/// `crate::audio` reads the same reach for the precipitation beds'
/// interior/exterior crossfade — one declared approximation serves
/// the visual and the audio leg.
pub(crate) const COVER_PROBE: f32 = 64.0;

/// Atlas tiles a spec may address before the quad set is clamped — a
/// degenerate `TexFrameEnd` bounds to an 8×8 sheet, not to memory.
const MAX_TILES: usize = 64;

/// Render assets for the drops — built once per session.
pub struct PrecipAssets {
    /// One 1×1 quad mesh per atlas tile, UVs baked to that tile — the
    /// same n×n policy the smoke atlas applies.
    pub quads: Vec<Handle<Mesh>>,
    /// Base material — cloned per drop so [`PrecipDrop::alpha`]
    /// animates one sprite, not the pool.
    pub material: Handle<StandardMaterial>,
}

/// Session-scoped precipitation state — inserted by
/// `load_session_world` when the rule parsed, removed on teardown.
/// Absent in dry sessions and in tests that never load a world; the
/// systems simply no-op.
#[derive(Resource)]
pub struct PrecipFx {
    pub assets: PrecipAssets,
}

/// Session-scoped evidence the headless record reports (`ppt=`).
/// Inserted on every `load_session_world`, reset on teardown — a dry
/// session binds `None` and formats no field.
#[derive(Resource, Default, Debug)]
pub struct PrecipReport {
    /// The bound `asbirthrule` name (`"rain"`) — `None` on a
    /// non-precipitating session.
    pub bound: Option<&'static str>,
    /// Why the bound record did not load — `Some` only when `bound`
    /// named a rule the VFS/parse could not deliver. Never a silent
    /// clear-weather fallback (F18-AC06).
    pub absent: Option<&'static str>,
    /// Whether the `ptx_<name>` atlas resolved (untextured drops
    /// still render — the same missing-texture policy as smoke).
    pub texture: bool,
    /// Drops spawned this session.
    pub emitted: u64,
    /// Drops expired on `Life` and despawned.
    pub expired: u64,
    /// Candidate spawns suppressed by the cover probe.
    pub covered: u64,
    /// Drops despawned on world contact.
    pub landed: u64,
    /// Spawns declined because the drop is undrawable: the authored
    /// `TexFrame*` window cannot fit `i64`, or a composed position/
    /// velocity or integrating `Gravity`/`Drag` is non-finite (a
    /// modded rule can author `nan`/`inf`) — counted, never overflowed,
    /// silently clamped, or fed into the cover probe's ray origin.
    pub undrawable: u64,
}

impl PrecipReport {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Clear the counters on teardown — the same unload-reset contract
/// the other session evidence reports hold
/// (`run_if(crate::session::unloading)`); the next load re-binds
/// `bound`/`absent` from its own effective weather.
pub fn reset_precip_report(mut report: ResMut<PrecipReport>) {
    report.reset();
}

/// Resolve the session's precipitation bind: the effective weather's
/// `asbirthrule` name → rule + atlas through the VFS, then the
/// deterministic [`Precipitation`] rig seeded from the session config
/// (F18 req 5's deterministic leg). Returns the evidence report plus
/// the render/fx resource and the rig — both `None` when the selector
/// names nothing (a dry session) or the rule cannot load (reported on
/// `absent`, never a silent fallback).
pub fn precip_session(
    vfs: &Vfs,
    weather: Weather,
    seed: u64,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
) -> (PrecipReport, Option<PrecipFx>, Option<Precipitation>) {
    let mut report = PrecipReport::default();
    let Some(name) = weather.precipitation() else {
        return (report, None, None);
    };
    report.bound = Some(name);
    let path = rule_path(name);
    let Some(resolved) = vfs.resolve(&path) else {
        warn!(path = %path, "precipitation rule unavailable");
        report.absent = Some("rule unavailable");
        return (report, None, None);
    };
    let bytes = match vfs.read(&resolved) {
        Ok(b) => b,
        Err(e) => {
            warn!(path = %path, error = %e, "precipitation rule unreadable");
            report.absent = Some("rule unreadable");
            return (report, None, None);
        }
    };
    let text = match std::str::from_utf8(&bytes) {
        Ok(t) => t,
        Err(_) => {
            warn!(path = %path, "precipitation rule is not text");
            report.absent = Some("rule unparseable");
            return (report, None, None);
        }
    };
    let parsed = match BirthRule::parse_file(text) {
        Ok(p) => p,
        Err(e) => {
            warn!(path = %path, error = %e, "precipitation rule unparseable");
            report.absent = Some("rule unparseable");
            return (report, None, None);
        }
    };
    for w in &parsed.warnings {
        warn!(path = %path, note = %w, "precipitation rule note");
    }
    // `From<&StandaloneBirthRule>` keeps the `tune/effects/` superset
    // fields authored — weather rules carry none, so retail rain/snow
    // convert identically; a mod that adds `Damp`/`Color` sees them.
    let spec = ParticleSpec::from(&parsed);
    let assets = precip_assets(vfs, name, &spec, meshes, images, materials, &mut report);
    let rig = Precipitation::new(spec, seed);
    (report, Some(PrecipFx { assets }), Some(rig))
}

/// Build the drop sprites for one session — the `ptx_<name>` atlas
/// resolves through `city::load_image` so mods can override it like
/// any texture. A missing atlas warns, marks the report and emits
/// untextured drops — the same missing-texture policy the loader
/// applies everywhere; it never sinks the session.
fn precip_assets(
    vfs: &Vfs,
    name: &str,
    spec: &ParticleSpec,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
    report: &mut PrecipReport,
) -> PrecipAssets {
    let stem = texture_stem(name);
    let texture = city::load_image(vfs, &stem).map(|(img, _)| images.add(img));
    report.texture = texture.is_some();
    if texture.is_none() {
        warn!(texture = %stem, "precipitation atlas unresolved — untextured drops");
    }
    let material = materials.add(StandardMaterial {
        base_color_texture: texture,
        // Unlit blended quads like the smoke sprites — lit drops would
        // pick up scene lighting the authored spec never accounted for.
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        cull_mode: None,
        ..default()
    });
    // The authored frame range selects the tile space: the smallest
    // n×n grid covering `TexFrameEnd` (retail `ptx_rain` is a measured
    // 4×4 sheet — `TexFrameEnd 15`). Designed derivation, clamped so a
    // degenerate record bounds to `MAX_TILES` quads.
    let span = spec.tex_frame_end.clamp(0, (MAX_TILES - 1) as i64) as usize + 1;
    let n = (span as f32).sqrt().ceil() as u32;
    let quads = (0..(n * n).min(MAX_TILES as u32))
        .map(|t| meshes.add(crate::damage_fx::tile_quad(t, n)))
        .collect();
    PrecipAssets { quads, material }
}

/// Anchor the emitter on the active `Camera3d` view — the same
/// `WorldCamera3d` pick the sky dome and smoke billboards follow.
/// `crate::wheel_fx` reads the same focus for its spawn billboards.
pub(crate) fn camera_focus(
    cameras: &Query<(&Camera, &GlobalTransform), crate::hudmap::WorldCamera3d>,
) -> Option<Vec3> {
    cameras
        .iter()
        .find(|(cam, _)| cam.is_active)
        .map(|(_, xf)| xf.translation())
        .filter(|p| p.is_finite())
}

/// Emit drops around the active camera while `Playing`. Each
/// candidate's position is the authored `Position`/`PositionVar`
/// jitter around the camera focus; a spawn whose upward probe hits
/// world geometry inside [`COVER_PROBE`] counts as covered and is
/// suppressed — the declared interior approximation, so rain cannot
/// appear inside tunnels, under bridges or through roofs. Remote
/// participants render their own session's emitter by construction —
/// the rig is session-local.
#[allow(clippy::too_many_arguments)] // Bevy system — the borrows are the contract.
pub fn emit_precip(
    mut commands: Commands,
    time: Res<Time>,
    session: Res<Session>,
    mut rig: Option<ResMut<Precipitation>>,
    fx: Option<Res<PrecipFx>>,
    mut report: ResMut<PrecipReport>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    spatial: Option<SpatialQuery>,
    cameras: Query<(&Camera, &GlobalTransform), crate::hudmap::WorldCamera3d>,
    drops: Query<&PrecipDrop>,
) {
    if !session.is_playing() {
        return;
    }
    let (Some(rig), Some(fx)) = (rig.as_mut(), fx) else {
        return;
    };
    let Some(focus) = camera_focus(&cameras) else {
        return;
    };
    let Some(base) = materials.get(&fx.assets.material).cloned() else {
        return;
    };
    let owner = SessionEntity(session.generation());
    let live = drops.iter().count();
    for _ in 0..rig.draw(time.delta_secs(), live) {
        // An undrawable drop — a flipbook window that cannot fit
        // `i64`, or a non-finite authored jitter/gravity/drag — is
        // declined and counted, never overflowed or fed to the cover
        // probe's ray origin.
        let Some(drop) = rig.drop(focus) else {
            report.undrawable += 1;
            continue;
        };
        // The cover probe — a spawn under world geometry reads as
        // sheltered and is suppressed. `solid: true` also catches a
        // candidate *inside* a collider (a jittered spawn buried in a
        // building reports distance 0).
        if let Some(sq) = spatial.as_ref()
            && sq
                .cast_ray(drop.position, Dir3::Y, COVER_PROBE, true, &default())
                .is_some()
        {
            report.covered += 1;
            continue;
        }
        // `rig.drop` already declined an unrepresentable window, so a
        // spawned drop's flipbook always resolves — `unreachable`
        // would be a lie if a drop were ever built by hand; keep the
        // first tile instead.
        let frame = drop.frame().unwrap_or(drop.frame_start);
        let quad = fx.assets.quads[(frame as usize).min(fx.assets.quads.len() - 1)].clone();
        let mut material = base.clone();
        material.base_color = Color::srgba(1.0, 1.0, 1.0, drop.alpha());
        let mut transform =
            Transform::from_translation(drop.position).with_scale(Vec3::splat(drop.radius * 2.0));
        transform.look_at(focus, Vec3::Y);
        transform.rotate_local_z(drop.rotation);
        commands.spawn((
            owner,
            drop,
            Mesh3d(quad),
            MeshMaterial3d(materials.add(material)),
            transform,
        ));
        report.emitted += 1;
    }
}

/// Integrate live drops and sync their sprites — position, growth,
/// camera-facing billboard plus the authored `DRotation` roll, the
/// authored flipbook tile and per-drop alpha. Drops die two ways:
/// `Life` expiry (`expired`) or world contact — the swept segment
/// from the rendered position to the integrated position probes the
/// collider scene, so a drop stops on the roof/floor/wall it struck
/// (`landed`) instead of passing through. Session-stamped, so
/// teardown takes any stragglers.
#[allow(clippy::too_many_arguments)] // Bevy system — the borrows are the contract.
pub fn advance_precip(
    mut commands: Commands,
    time: Res<Time>,
    session: Res<Session>,
    fx: Option<Res<PrecipFx>>,
    mut report: ResMut<PrecipReport>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    spatial: Option<SpatialQuery>,
    mut drops: Query<(
        Entity,
        &mut PrecipDrop,
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
    let focus = camera_focus(&cameras);
    for (entity, mut drop, mut xf, mut mesh, material) in &mut drops {
        let from = xf.translation;
        if !drop.advance(dt) {
            commands.entity(entity).despawn();
            report.expired += 1;
            continue;
        }
        // The contact probe — sweep the segment this step covered
        // (plus the sprite's radius so the visible card stops at the
        // surface, not a half-extent past it). A drop that tunnels the
        // whole segment still reads as landed.
        if let Some(sq) = spatial.as_ref() {
            let delta = drop.position - from;
            let reach = delta.length() + drop.radius;
            if reach > f32::EPSILON
                && let Ok(dir) = Dir3::new(delta)
                && sq.cast_ray(from, dir, reach, true, &default()).is_some()
            {
                commands.entity(entity).despawn();
                report.landed += 1;
                continue;
            }
        }
        xf.translation = drop.position;
        xf.scale = Vec3::splat((drop.radius * 2.0).max(0.0));
        if let Some(f) = focus {
            xf.look_at(f, Vec3::Y);
            xf.rotate_local_z(drop.rotation);
        }
        if let Some(fx) = fx.as_ref()
            && let Some(frame) = drop.frame()
        {
            mesh.0 = fx.assets.quads[(frame as usize).min(fx.assets.quads.len() - 1)].clone();
        }
        if let Some(mut mat) = materials.get_mut(&material.0) {
            mat.base_color = Color::srgba(1.0, 1.0, 1.0, drop.alpha());
        }
    }
}
