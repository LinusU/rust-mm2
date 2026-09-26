//! The authored HUD minimap (F22-A.1, HUD-4): the per-city `mmHudMap`
//! spec (`tune/<city>.mmhudmap`), the world-space tile quads
//! (`geometry/hudmap_<city>.pkg`) and the authored marker meshes
//! (`hudmap_tri` — a flat XZ triangle whose apex is vehicle-forward,
//! `hudmap_square` — a textured dot quad with nine authored paint jobs)
//! bound into a dedicated top-down orthographic camera on its own
//! render layer.
//!
//! - [`spawn_hud_map`] runs inside `load_session_world` for city
//!   sessions: it parses the spec, converts the tile PKG through the
//!   same `pkg_paint_parts`/`MaterialCache` path the city uses (VFS
//!   texture resolution and authored shader tints included), spawns
//!   tiles + markers + the map camera, and inserts the session-scoped
//!   [`mm2_game::HudMap`] state and [`HudMapReport`]. Everything
//!   carries the session's `SessionEntity` stamp — teardown removes it
//!   wholesale, and `drive_session` drops both resources.
//! - [`hudmap_input`] owns the documented controls (HUD-4): TAB cycles
//!   the two inset views and off, E toggles zoom, F toggles rotation,
//!   and Q opens the full-screen *pause* map — it queues the session's
//!   pause intent under the same `allows_pause` authority gate as Esc
//!   (single player only). While that pause is up, Q/Esc close the map
//!   straight back to play and the pause-menu overlay stays hidden and
//!   input-dead (`pause_input` gates on the open map). The map only
//!   lives inside its pause: leaving `Paused` by any path — or a queued
//!   pause intent that never landed — clears `fullscreen`, so it can
//!   never sit over live gameplay with no key that closes it.
//!   [`dev_pause_map_once`] is the quarantined `--pause-map` dev
//!   override for capture runs (input is frozen there).
//! - [`drive_hud_map`] tracks the local player under the camera, eases
//!   the zoom at the authored `Approach Rate`, recomputes the viewport
//!   from the authored `Pos`/`Size` window fractions, scales markers to
//!   the authored `IconScale` extents and repaints gate dots by the
//!   local participant's progress — un-cleared `GREEN_DOT`, cleared
//!   `GREY_DOT`, the navigation target `YELLOW_DOT`, the armed finish
//!   `FINISH_DOT` (designed paint picks over the authored palette,
//!   matching HUD-4's bright/dark/highlight rule).
//!
//! Missing content is explicit: a city without `tune/<stem>.mmhudmap`
//! or `geometry/hudmap_<stem>.pkg` records `absent` on the report and
//! binds nothing — never a substitute map. The dev world gets no
//! report at all, keeping its records bit-identical.
//!
//! Designed edges this slice owns: the TAB cycle order and the second
//! inset's size (the original's two "smaller views" geometry is
//! unrecovered), the marker paint assignments, the `IconScale` →
//! world-extent reading, and the pause-map close path (HUD-4 names
//! only the open).

use bevy::camera::ScalingMode;
use bevy::camera::Viewport;
use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use mm2_assets::Vfs;
use mm2_formats::hudmap::HudMapSpec;
use mm2_formats::pkg::Pkg;
use mm2_game::{
    HudMap, MapView, NavTarget, Player, PlayerControl, RaceDefinition, RacePhase, RaceProgress,
    RaceState, Session, SessionEntity, SessionPhase, TargetSelection, navigation_target,
};
use std::path::Path;
use tracing::{info, warn};

use crate::camera::CameraMode;
use crate::city::{MaterialCache, pkg_paint_parts};
use crate::session::SessionControl;

/// Render layer the whole map lives on: the chase/free cameras stay on
/// the default layer and never see it; the map camera sees only this.
const MAP_LAYER: usize = 1;
/// Map camera height above the marker plane — the authored tile/marker
/// Y range sits far inside the ortho volume either side.
const CAMERA_LIFT: f32 = 500.0;
/// Lift markers this far above the tallest tile vertex so no authored
/// tile pixel overdraws a marker.
const MARKER_LIFT: f32 = 5.0;
/// Second TAB view's size multiplier over the authored inset
/// (designed — the original's two "smaller views" geometry is
/// unrecovered; the larger inset keeps the same corner anchor).
const LARGE_VIEW_SCALE: f32 = 1.9;
/// `hudmap_tri` paint job for the local player (yellow — designed pick
/// over the authored palette).
const TRI_PAINT_PLAYER: usize = 5;
/// Pool of `hudmap_tri` paint jobs opponents cycle through — every
/// authored paint except the player's and the near-black navy.
const TRI_PAINT_OPPONENTS: &[usize] = &[4, 1, 3, 6, 7, 8, 2, 9];
/// `hudmap_square` paint jobs (authored order red/blue/green/grey/
/// yellow/finish/gold/bank/hideout) bound to marker roles — designed
/// picks matching HUD-4's bright/dark/highlight vocabulary.
const DOT_GATE: usize = 2;
const DOT_CLEARED: usize = 3;
const DOT_TARGET: usize = 4;
const DOT_FINISH: usize = 5;

/// Marker for the map's own camera — `drive_hud_map` moves, aims and
/// frames it. Session-owned like everything else it serves.
#[derive(Component)]
pub struct HudMapCamera;

/// Query filter for systems that pick "the" world camera
/// (`audio_listener`, `apply_city_pvs`, damage billboards, …): the map
/// camera and the F22-B.2 mirror strip are *active* `Camera3d`s while
/// up, so every such pick must exclude them or the secondary view
/// becomes the listener / PVS source / billboard-facing view.
pub type WorldCamera3d = (
    With<Camera3d>,
    Without<HudMapCamera>,
    Without<crate::camera::MirrorCamera>,
);

/// One world-space map tile (`hudmap_<city>.pkg` section) — spawned at
/// the authored pose and never moved.
#[derive(Component)]
pub struct HudMapTile;

/// What one marker entity tracks.
#[derive(Debug, Clone, Copy)]
pub enum MarkerRole {
    /// The local participant's heading triangle.
    Player,
    /// Opponent triangle pool slot — participants are bound to pool
    /// slots in entity order each frame.
    Opponent,
    /// Dot over `RaceDefinition::checkpoints[i]`.
    Gate(usize),
    /// Dot over `RaceDefinition::finish`.
    Finish,
}

/// A session-owned map marker entity.
#[derive(Component, Debug, Clone, Copy)]
pub struct HudMapMarker {
    /// What it tracks.
    pub role: MarkerRole,
    /// The mesh's authored XZ extent (metres) — the `IconScale` values
    /// are read as the marker's target world extent, so the transform
    /// scale is `icon_scale / extent` (inferred unit).
    pub extent: f32,
}

/// Session-scoped load report for the HUD map — the `map=` record
/// field's source and [`drive_hud_map`]'s material/plane table.
/// Inserted for every city session (map bound or not — `absent` says
/// why not); never inserted on the dev world.
#[derive(Resource)]
pub struct HudMapReport {
    /// `tune/<stem>.mmhudmap` logical path attempted.
    pub spec_path: String,
    /// `geometry/hudmap_<stem>.pkg` logical path attempted.
    pub pkg_path: String,
    /// Tile sections spawned (0 when nothing bound).
    pub tiles: usize,
    /// Marker entities spawned (player + opponent pool + gates/finish).
    pub markers: usize,
    /// Why no [`HudMap`] state was bound (`None` = bound):
    /// `missing-tune`, `unparseable-tune`, `missing-pkg`,
    /// `unparseable-pkg`.
    pub absent: Option<&'static str>,
    /// Square paint-job materials for gate-dot repaints, indexed by
    /// authored paint (`*_DOT` order).
    pub dot_materials: Vec<Handle<StandardMaterial>>,
    /// The height markers ride at — tallest tile vertex + margin.
    pub marker_y: f32,
}

impl HudMapReport {
    /// The `map=` record field body: `<view>/<orient>/z<zoom>` state
    /// with tile/marker counts when bound, or `absent:<why>` when the
    /// city had no usable authored map content.
    pub fn smoke_detail(&self, map: Option<&HudMap>) -> String {
        match map {
            Some(m) => format!(
                "{}/{}/{}t/{}m",
                m.smoke_detail(),
                self.pkg_path.rsplit('/').next().unwrap_or(&self.pkg_path),
                self.tiles,
                self.markers,
            ),
            None => match self.absent {
                Some(why) => format!("absent:{why}"),
                None => "absent:state-removed".into(),
            },
        }
    }
}

/// The marker mesh's authored XZ extent — the bounding square's side —
/// so `IconScale` can scale it to a world extent.
fn authored_extent(pkg: &Pkg) -> f32 {
    let mut r = 0.0f32;
    for (_name, geo) in pkg.geometries() {
        for section in &geo.sections {
            for strip in &section.strips {
                for v in &strip.vertices {
                    r = r.max(v.position[0].abs()).max(v.position[2].abs());
                }
            }
        }
    }
    r * 2.0
}

/// Tallest authored vertex Y in the package (map tiles are flat and
/// near y=0, but measured rather than assumed).
fn top_vertex_y(pkg: &Pkg) -> f32 {
    let mut y = 0.0f32;
    for (_name, geo) in pkg.geometries() {
        for section in &geo.sections {
            for strip in &section.strips {
                for v in &strip.vertices {
                    y = y.max(v.position[1]);
                }
            }
        }
    }
    y
}

/// One paint job's material out of a marker PKG, flattened for the
/// top-down map read ([`MaterialCache::unlit_copy`]).
fn paint_material(
    pkg: &Pkg,
    paint: usize,
    mats: &mut MaterialCache<'_>,
) -> Option<Handle<StandardMaterial>> {
    let s = pkg.shaders()?;
    let shader = s
        .shaders
        .get(paint.saturating_mul(s.shaders_per_paint_job.max(1) as usize))?;
    let base = mats.shader_material(shader);
    Some(mats.unlit_copy(&base))
}

fn read_pkg(vfs: &Vfs, logical: &str) -> Option<Pkg> {
    vfs.read_path(logical)
        .ok()
        .map(|(bytes, _)| bytes)
        .and_then(|bytes| match Pkg::parse(&bytes) {
            Ok(p) => Some(p),
            Err(e) => {
                warn!(path = %logical, error = %e, "pkg failed to parse");
                None
            }
        })
}

/// Spawn the session's HUD map for `psdl_path`'s city. `race` carries
/// the event definition when an event session loaded (its gates/finish
/// get dots); `opponent_count` sizes the opponent marker pool. Returns
/// the report plus the bound [`HudMap`] state — `None` when no usable
/// authored content exists (the report's `absent` says why).
#[allow(clippy::too_many_arguments)]
pub fn spawn_hud_map(
    commands: &mut Commands,
    vfs: &Vfs,
    psdl_path: &str,
    race: Option<&RaceDefinition>,
    opponent_count: usize,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
    owner: SessionEntity,
    generation: u64,
) -> (HudMapReport, Option<HudMap>) {
    let stem = Path::new(psdl_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(psdl_path);
    let mut report = HudMapReport {
        spec_path: format!("tune/{stem}.mmhudmap"),
        pkg_path: format!("geometry/hudmap_{stem}.pkg"),
        tiles: 0,
        markers: 0,
        absent: None,
        dot_materials: Vec::new(),
        marker_y: MARKER_LIFT,
    };

    let spec = match vfs.read_path(&report.spec_path) {
        Ok((bytes, _)) => match HudMapSpec::parse(&String::from_utf8_lossy(&bytes)) {
            Ok(s) => Some(s),
            Err(e) => {
                warn!(path = %report.spec_path, error = %e, "hud map spec failed to parse");
                report.absent = Some("unparseable-tune");
                None
            }
        },
        Err(e) => {
            warn!(path = %report.spec_path, error = %e, "hud map spec unavailable");
            report.absent = Some("missing-tune");
            None
        }
    };
    let Some(spec) = spec else {
        return (report, None);
    };
    for issue in spec.validate() {
        warn!(path = %report.spec_path, issue = ?issue, "hud map spec issue");
    }

    let tiles_pkg = match read_pkg(vfs, &report.pkg_path) {
        Some(p) => p,
        None => {
            report.absent = Some(if vfs.resolve(&report.pkg_path).is_some() {
                "unparseable-pkg"
            } else {
                "missing-pkg"
            });
            return (report, None);
        }
    };

    let mut mats = MaterialCache::new(vfs, images, materials);
    let mut missing_prims = 0usize;
    let spawn_marker = |commands: &mut Commands,
                        report: &mut HudMapReport,
                        mesh: &Handle<Mesh>,
                        material: Handle<StandardMaterial>,
                        role: MarkerRole,
                        extent: f32,
                        pos: Vec3| {
        commands.spawn((
            owner,
            HudMapMarker { role, extent },
            Mesh3d(mesh.clone()),
            MeshMaterial3d(material),
            RenderLayers::layer(MAP_LAYER),
            Transform::from_xyz(pos.x, report.marker_y, pos.z),
            Visibility::Hidden,
        ));
        report.markers += 1;
    };

    // World-space tiles — spawned at the authored pose verbatim, so the
    // map's world→map projection is identity in XZ (measured: the tile
    // verts span the city bounds).
    report.marker_y = top_vertex_y(&tiles_pkg) + MARKER_LIFT;
    for (mesh, mat) in pkg_paint_parts(&tiles_pkg, &mut mats, meshes, &mut missing_prims, 0) {
        commands.spawn((
            owner,
            HudMapTile,
            Mesh3d(mesh),
            MeshMaterial3d(mats.unlit_copy(&mat)),
            RenderLayers::layer(MAP_LAYER),
            Transform::IDENTITY,
        ));
        report.tiles += 1;
    }
    if report.tiles == 0 {
        warn!(path = %report.pkg_path, "hud map pkg produced no tiles");
        report.absent = Some("empty-pkg");
        return (report, None);
    }

    // Marker meshes — authored flat XZ shapes shared by every city.
    // Each missing piece degrades its own marker family only: a map
    // without gate dots still draws tiles and the player tri.
    let tri_pkg = read_pkg(vfs, "geometry/hudmap_tri.pkg");
    let square_pkg = read_pkg(vfs, "geometry/hudmap_square.pkg");

    if let Some(tri) = &tri_pkg {
        let extent = authored_extent(tri).max(1.0);
        let mesh = pkg_paint_parts(tri, &mut mats, meshes, &mut missing_prims, 0)
            .into_iter()
            .next()
            .map(|(m, _)| m);
        if let Some(tri_mesh) = mesh {
            // The player's heading tri.
            if let Some(mat) = paint_material(tri, TRI_PAINT_PLAYER, &mut mats) {
                spawn_marker(
                    commands,
                    &mut report,
                    &tri_mesh,
                    mat,
                    MarkerRole::Player,
                    extent,
                    Vec3::ZERO,
                );
            }
            // The opponent pool — paint cycled over the authored jobs.
            for slot in 0..opponent_count {
                let paint = TRI_PAINT_OPPONENTS[slot % TRI_PAINT_OPPONENTS.len()];
                if let Some(mat) = paint_material(tri, paint, &mut mats) {
                    spawn_marker(
                        commands,
                        &mut report,
                        &tri_mesh,
                        mat,
                        MarkerRole::Opponent,
                        extent,
                        Vec3::ZERO,
                    );
                }
            }
        }
    } else {
        warn!("geometry/hudmap_tri.pkg unavailable — no player/opponent map markers");
    }

    if let Some(square) = &square_pkg
        && let Some(def) = race
    {
        let extent = authored_extent(square).max(1.0);
        let mesh = pkg_paint_parts(square, &mut mats, meshes, &mut missing_prims, 0)
            .into_iter()
            .next()
            .map(|(m, _)| m);
        if let Some(dot_mesh) = mesh {
            for paint in 0..square.shaders().map(|s| s.paint_jobs).unwrap_or(0) {
                if let Some(mat) = paint_material(square, paint as usize, &mut mats) {
                    report.dot_materials.push(mat);
                }
            }
            let gate_mat = report
                .dot_materials
                .get(DOT_GATE)
                .cloned()
                .unwrap_or_default();
            let finish_mat = report
                .dot_materials
                .get(DOT_FINISH)
                .cloned()
                .unwrap_or_default();
            for (i, cp) in def.checkpoints.iter().enumerate() {
                spawn_marker(
                    commands,
                    &mut report,
                    &dot_mesh,
                    gate_mat.clone(),
                    MarkerRole::Gate(i),
                    extent,
                    cp.center,
                );
            }
            if let Some(finish) = &def.finish {
                spawn_marker(
                    commands,
                    &mut report,
                    &dot_mesh,
                    finish_mat,
                    MarkerRole::Finish,
                    extent,
                    finish.center,
                );
            }
        }
    } else if race.is_some() {
        warn!("geometry/hudmap_square.pkg unavailable — no gate map markers");
    }

    // The map camera: top-down orthographic on the map layer, drawn over
    // the main camera (order 1) into the authored `Pos`/`Size` viewport
    // (`drive_hud_map` recomputes it per frame — window size is only
    // known there). `Ocean Color` is its clear colour — the authored
    // background for tile-free water areas.
    commands.spawn((
        owner,
        HudMapCamera,
        Camera3d::default(),
        Camera {
            order: 1,
            clear_color: ClearColorConfig::Custom(Color::srgb(
                spec.ocean_color[0],
                spec.ocean_color[1],
                spec.ocean_color[2],
            )),
            ..default()
        },
        Projection::Orthographic(OrthographicProjection {
            scaling_mode: ScalingMode::Fixed {
                width: 2.0 * spec.zoom_out_dist,
                height: 2.0 * spec.zoom_out_dist,
            },
            near: 0.0,
            far: CAMERA_LIFT * 4.0,
            ..OrthographicProjection::default_3d()
        }),
        RenderLayers::layer(MAP_LAYER),
        Transform::from_translation(Vec3::Y * (report.marker_y + CAMERA_LIFT))
            .looking_at(Vec3::new(0.0, report.marker_y, 0.0), Vec3::NEG_Z),
    ));
    info!(
        spec = %report.spec_path,
        pkg = %report.pkg_path,
        tiles = report.tiles,
        markers = report.markers,
        "hud map bound"
    );
    (report, Some(HudMap::new(spec, generation)))
}

/// The map's control surface (HUD-4). Runs between
/// `session_control_input` and `pause_input`: while `Playing`, TAB/E/F
/// drive the corner map and Q opens the full-screen pause map under the
/// same `allows_pause` authority gate as Esc-pause (single player
/// only); while `Paused` with the map up, Q/Esc close it straight back
/// to play — ahead of `pause_input`, so the closing press is never
/// double-read as a menu resume.
pub fn hudmap_input(
    keys: Res<ButtonInput<KeyCode>>,
    cam_mode: Res<CameraMode>,
    mut session: ResMut<Session>,
    mut control: ResMut<SessionControl>,
    map: Option<ResMut<HudMap>>,
) {
    let Some(mut map) = map else { return };
    if map.is_stale(session.generation()) {
        return;
    }
    // In the dev free camera `E`/`Q` are the vertical axes
    // (`camera::free_fly`) — the map yields them there rather than
    // double-binding both meanings on one key. The chase camera, where
    // the documented controls live, keeps them.
    let free_cam = *cam_mode == CameraMode::Free;
    match *session.phase() {
        SessionPhase::Playing => {
            if keys.just_pressed(KeyCode::Tab) {
                map.cycle_view();
            }
            if !free_cam && keys.just_pressed(KeyCode::KeyE) {
                map.toggle_zoom();
            }
            if keys.just_pressed(KeyCode::KeyF) {
                map.toggle_orientation();
            }
            if !free_cam
                && keys.just_pressed(KeyCode::KeyQ)
                && session.config().is_none_or(|c| c.authority.allows_pause())
            {
                map.fullscreen = true;
                control.pause = true;
            }
        }
        SessionPhase::Paused
            if map.fullscreen
                && (keys.just_pressed(KeyCode::KeyQ) || keys.just_pressed(KeyCode::Escape)) =>
        {
            map.fullscreen = false;
            session
                .transition(SessionPhase::Playing)
                .expect("Paused → Playing is a legal transition");
        }
        _ => {}
    }
    // The full-screen map only lives inside its pause — the Q/Esc arm
    // above clears it on the way out, and any other exit from `Paused`
    // (restart, teardown, a queued pause intent `drive_session`
    // rejected) drops it here so the order-1 map camera can never sit
    // over live gameplay with no key that closes it. A `control.pause`
    // still queued this update is the one exception: the `Playing` arm
    // and `dev_pause_map_once` set the flag the same frame they queue
    // the intent, and `drive_session` consumes it later in the update.
    if map.fullscreen && !matches!(session.phase(), SessionPhase::Paused) && !control.pause {
        map.fullscreen = false;
    }
}

/// `--pause-map` (quarantined `DevOverrides`, evidence runs only): open
/// the full-screen pause map on the first `Playing` frame — how a
/// `--frames`/`--screenshot` capture renders the pause map while live
/// input is frozen. Render-only like `--pause`: out of
/// `record_eligibility`. Same MP-6 gate as the Q key — on an authority
/// that cannot pause it never fires, so the map can never be left
/// full-screen over a session that is still `Playing`. One-shot; a
/// session with no bound (or stale) map keeps it unfired.
pub fn dev_pause_map_once(
    session: Res<Session>,
    map: Option<ResMut<HudMap>>,
    mut control: ResMut<SessionControl>,
    mut fired: Local<bool>,
) {
    if *fired {
        return;
    }
    if session.is_playing()
        && session
            .config()
            .is_some_and(|c| c.dev.pause_map && c.authority.allows_pause())
        && let Some(mut map) = map
        && !map.is_stale(session.generation())
    {
        *fired = true;
        map.fullscreen = true;
        control.pause = true;
    }
}

/// The authored `Pos`/`Size` viewport in physical pixels. `Inset` uses
/// the authored fractions verbatim (top-left origin — the corner the
/// original draws the map in); `Large` is the same corner anchor at the
/// designed `LARGE_VIEW_SCALE`. Returns `(position, size)`.
fn inset_rect(view: MapView, spec: &HudMapSpec, window: Vec2) -> (Vec2, Vec2) {
    let scale = if view == MapView::Large {
        LARGE_VIEW_SCALE
    } else {
        1.0
    };
    let size = Vec2::from(spec.size) * scale;
    // Grow toward the window's top-left so the tuned corner anchor
    // (bottom-right on both cities) is preserved.
    let size_px = (size * window).min(window * 0.95);
    let origin_px =
        (((Vec2::from(spec.pos) + Vec2::from(spec.size)) * window) - size_px).max(Vec2::ZERO);
    (origin_px, size_px)
}

/// The participants the map reads: entity (opponent ordering), control
/// (local vs opponents), pose, and the local course view.
type MapParticipants<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Player,
        &'static GlobalTransform,
        Option<&'static RaceProgress>,
        Option<&'static TargetSelection>,
    ),
    Without<HudMapMarker>,
>;

/// The map camera entity.
type MapCam<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut Transform,
        &'static mut Camera,
        &'static mut Projection,
    ),
    (With<HudMapCamera>, Without<HudMapMarker>),
>;

/// The marker entities.
type MapMarkers<'w, 's> = Query<
    'w,
    's,
    (
        &'static HudMapMarker,
        &'static mut Transform,
        &'static mut Visibility,
        &'static mut MeshMaterial3d<StandardMaterial>,
    ),
>;

/// Per-frame map driver: camera placement/framing, zoom easing, marker
/// tracking and gate-dot colouring. Runs on every update — the eased
/// zoom and a rotating map should track through pause and countdown
/// alike.
#[allow(clippy::too_many_arguments)]
pub fn drive_hud_map(
    time: Res<Time>,
    session: Res<Session>,
    map: Option<ResMut<HudMap>>,
    report: Option<Res<HudMapReport>>,
    race: Option<Res<RaceState>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    participants: MapParticipants,
    mut camera: MapCam,
    mut markers: MapMarkers,
) {
    let (Some(mut map), Some(report)) = (map, report) else {
        return;
    };
    if map.is_stale(session.generation()) {
        return;
    }
    map.advance(time.delta_secs());

    let yaw_of = |gt: &GlobalTransform| {
        let f = gt.forward();
        (-f.x).atan2(-f.z)
    };
    let local = participants
        .iter()
        .find(|(_, p, ..)| p.control == PlayerControl::Local);
    // Opponent tris bind pool slots in entity order — stable for the
    // roster's fixed membership.
    let opponents: Vec<(Vec3, f32)> = {
        let mut v: Vec<(Entity, Vec3, f32)> = participants
            .iter()
            .filter(|(_, p, ..)| p.control != PlayerControl::Local)
            .map(|(e, _, gt, ..)| (e, gt.translation(), yaw_of(gt)))
            .collect();
        v.sort_by_key(|(e, ..)| *e);
        v.into_iter().map(|(_, pos, yaw)| (pos, yaw)).collect()
    };

    // Camera: parked above the local player, north up or heading up.
    let player_pose = local.map(|(_, _, gt, ..)| (gt.translation(), yaw_of(gt)));
    for (mut xf, mut cam, mut proj) in &mut camera {
        let Some((pos, yaw)) = player_pose else {
            cam.is_active = false;
            continue;
        };
        cam.is_active = map.visible();
        if !cam.is_active {
            continue;
        }
        let up = match map.orientation {
            mm2_game::MapOrientation::NorthUp => Vec3::NEG_Z,
            mm2_game::MapOrientation::Rotating => {
                Vec3::new(-yaw.sin(), 0.0, -yaw.cos()).normalize_or(Vec3::NEG_Z)
            }
        };
        *xf = Transform::from_xyz(pos.x, report.marker_y + CAMERA_LIFT, pos.z)
            .looking_at(Vec3::new(pos.x, report.marker_y, pos.z), up);
        // The viewport: authored `Pos`/`Size` fractions of the window
        // for the insets, the whole window while the full-screen map is
        // up. `zoom` is the view half-extent — the projection rectangle
        // follows the viewport's aspect so the map does not stretch.
        let win = windows.iter().next().map(|w| {
            Vec2::new(
                w.resolution.physical_width() as f32,
                w.resolution.physical_height() as f32,
            )
        });
        let (vp_pos, vp_size) = if map.fullscreen {
            (Vec2::ZERO, win.unwrap_or(Vec2::ONE))
        } else {
            inset_rect(map.view, &map.spec, win.unwrap_or(Vec2::new(1280.0, 960.0)))
        };
        cam.viewport = if map.fullscreen {
            None
        } else {
            win.map(|_| Viewport {
                physical_position: UVec2::new(vp_pos.x as u32, vp_pos.y as u32),
                physical_size: UVec2::new(vp_size.x.max(1.0) as u32, vp_size.y.max(1.0) as u32),
                depth: 0.0..1.0,
            })
        };
        if let Projection::Orthographic(o) = &mut *proj {
            let height = 2.0 * map.zoom;
            let aspect = (vp_size.x / vp_size.y.max(1.0)).max(0.01);
            o.scaling_mode = ScalingMode::Fixed {
                width: height * aspect,
                height,
            };
        }
    }

    // Markers: scale every icon to the authored `IconScale` extent at
    // the eased zoom, then bind the tracked poses.
    let icon_scale = map.icon_scale();
    let mut opp_iter = opponents.iter();
    // The local participant's view of the course (the same
    // disambiguation `update_checkpoint_markers` uses).
    let progress = local.and_then(|(.., prog, _)| prog);
    let race_def = race.filter(|r| !r.is_stale(session.generation()));
    // The highlighted dot is the arrow's target verbatim (HUD-4) —
    // `navigation_target` already scopes the compass to the
    // Blitz/Checkpoint rule (HUD-2), so a Circuit race highlights
    // nothing rather than inventing a second instrument.
    let target: Option<NavTarget> = match (race_def.as_deref(), local) {
        (Some(r), Some((_, _, gt, Some(prog), sel))) => navigation_target(
            &r.definition,
            prog,
            sel.and_then(|t| t.picked),
            gt.translation(),
        ),
        _ => None,
    };
    for (marker, mut xf, mut vis, mut mat) in &mut markers {
        match marker.role {
            MarkerRole::Player => match player_pose {
                Some((pos, yaw)) => {
                    xf.translation = Vec3::new(pos.x, report.marker_y, pos.z);
                    xf.rotation = Quat::from_rotation_y(yaw);
                    *vis = Visibility::Visible;
                }
                None => *vis = Visibility::Hidden,
            },
            MarkerRole::Opponent => match opp_iter.next() {
                Some((pos, yaw)) => {
                    xf.translation = Vec3::new(pos.x, report.marker_y, pos.z);
                    xf.rotation = Quat::from_rotation_y(*yaw);
                    *vis = Visibility::Visible;
                }
                None => *vis = Visibility::Hidden,
            },
            MarkerRole::Gate(i) => {
                *vis = Visibility::Visible;
                if race_def.is_some() {
                    let paint = if progress.is_some_and(|p| p.is_cleared(i)) {
                        DOT_CLEARED
                    } else if target == Some(NavTarget::Gate(i)) {
                        DOT_TARGET
                    } else {
                        DOT_GATE
                    };
                    if let Some(m) = report.dot_materials.get(paint) {
                        *mat = MeshMaterial3d(m.clone());
                    }
                }
            }
            MarkerRole::Finish => {
                // The finish dot only appears once every gate is
                // cleared — the RACE-7 rule the world-space marker
                // already draws. As the arrow's target it takes the
                // highlight paint (HUD-4).
                let unlocked = race_def.as_deref().is_some_and(|r| {
                    matches!(r.phase, RacePhase::Complete)
                        || progress
                            .is_some_and(|p| p.cleared_count() >= r.definition.checkpoints.len())
                });
                *vis = if unlocked {
                    Visibility::Visible
                } else {
                    Visibility::Hidden
                };
                let paint = if target == Some(NavTarget::Finish) {
                    DOT_TARGET
                } else {
                    DOT_FINISH
                };
                if let Some(m) = report.dot_materials.get(paint) {
                    *mat = MeshMaterial3d(m.clone());
                }
            }
        }
        let s = (icon_scale / marker.extent).max(0.0);
        xf.scale = Vec3::splat(s);
    }
}
