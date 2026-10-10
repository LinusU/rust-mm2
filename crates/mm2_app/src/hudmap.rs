//! The authored HUD minimap (F22-A.1, HUD-4): the per-city `mmHudMap`
//! spec (`tune/<city>.mmhudmap`), the world-space tile quads
//! (`geometry/hudmap_<city>.pkg`) and the authored marker meshes
//! (`hudmap_tri` — a flat XZ triangle whose apex is vehicle-forward,
//! `hudmap_square` — a textured dot quad with nine authored paint jobs)
//! bound into a dedicated top-down orthographic camera on its own
//! render layer. The local player's own marker is a designed
//! black-outlined yellow arrowhead ([`player_marker_mesh`]) rather
//! than the authored tri: the authored one reads as a speck against
//! the busy tile artwork.
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

use bevy::camera::Viewport;
use bevy::camera::visibility::RenderLayers;
use bevy::camera::{MsaaWriteback, ScalingMode};
use bevy::core_pipeline::tonemapping::Tonemapping;
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
/// The player arrowhead's fill (designed — the HUD-4 highlight yellow).
const PLAYER_FILL: Color = Color::srgb(1.0, 0.85, 0.0);
/// Outline width around the player arrowhead, as a fraction of its
/// extent — a thin dark rim that keeps it readable over light tiles.
const PLAYER_OUTLINE: f32 = 0.08;
/// The player arrowhead's size over the authored `IconScale` extent
/// the other markers use — it is the one marker the driver must find at
/// a glance.
const PLAYER_ICON_SCALE: f32 = 1.5;
/// The player arrowhead rides this far above the other markers so it
/// always draws over an opponent tri it overlaps.
const PLAYER_LIFT: f32 = 1.0;
/// Pool of `hudmap_tri` paint jobs opponents cycle through — every
/// authored paint except the player's yellow (5) and the near-black
/// navy. Shared
/// with `oppind`'s in-world arrows so an opponent's indicator and map
/// tri agree on colour (both pools bind in entity order).
pub(crate) const TRI_PAINT_OPPONENTS: &[usize] = &[4, 1, 3, 6, 7, 8, 2, 9];
/// `hudmap_square` paint jobs (authored order red/blue/green/grey/
/// yellow/finish/gold/bank/hideout) bound to marker roles — designed
/// picks matching HUD-4's bright/dark/highlight vocabulary.
const DOT_GATE: usize = 2;
const DOT_CLEARED: usize = 3;
const DOT_TARGET: usize = 4;
const DOT_FINISH: usize = 5;
/// The Cops & Robbers dots — authored paint jobs 6/7/8 of
/// `hudmap_square.pkg` bind the `GOLD_DOT`/`BANK_DOT`/`HIDEOUT_DOT`
/// textures (read from the package's shader records).
const DOT_GOLD: usize = 6;
const DOT_BANK: usize = 7;
const DOT_HIDEOUT: usize = 8;

/// Marker for the map's own camera — `drive_hud_map` moves, aims and
/// frames it. Session-owned like everything else it serves.
#[derive(Component)]
pub struct HudMapCamera;

/// Designed bezel surrounding the corner map viewport.
#[derive(Component)]
pub struct HudMapFrame;

/// Query filter for systems that pick "the" world camera
/// (`audio_listener`, `apply_city_pvs`, damage billboards, …): the map
/// camera and the F22-B.2 mirror strip are *active* `Camera3d`s while
/// up, so every such pick must exclude them or the secondary view
/// becomes the listener / PVS source / billboard-facing view.
pub type WorldCamera3d = (
    With<Camera3d>,
    Without<HudMapCamera>,
    Without<crate::camera::MirrorCamera>,
    Without<crate::navarrow3d::NavArrowCamera>,
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
    /// Cops & Robbers: the gold — at its site, or on the car carrying it.
    Gold,
    /// Cops & Robbers: the hideout.
    Hideout,
    /// Cops & Robbers: the bank.
    Bank,
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
/// so `IconScale` can scale it to a world extent. `oppind` measures
/// the same extent for its in-world arrow scale.
pub(crate) fn authored_extent(pkg: &Pkg) -> f32 {
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
/// top-down map read ([`MaterialCache::unlit_copy`] — unlit and
/// double-sided, which is also what `oppind`'s camera-facing arrows
/// need).
pub(crate) fn paint_material(
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

/// The local player's map marker: a yellow arrowhead with a thin black
/// rim, apex −Z (vehicle-forward, like the authored `hudmap_tri`), in
/// the XZ plane with a bounding extent of 1 so `IconScale` scales it
/// like every other marker. One mesh with vertex colours — the rim is
/// a larger copy of the fill grown about its incentre (a homothety
/// about the incentre moves every edge out by the same distance), set
/// just below it so the fill always wins the depth test.
pub fn player_marker_mesh() -> Mesh {
    use bevy::asset::RenderAssetUsages;
    use bevy::mesh::{Indices, PrimitiveTopology};

    // Apex forward, centroid on the origin so the car sits at the
    // arrowhead's middle and heading-up rotation pivots in place.
    let fill = [
        Vec2::new(0.0, -0.6),
        Vec2::new(-0.36, 0.3),
        Vec2::new(0.36, 0.3),
    ];
    let side = |a: usize, b: usize| fill[a].distance(fill[b]);
    let (la, lb, lc) = (side(1, 2), side(2, 0), side(0, 1));
    let perimeter = la + lb + lc;
    let incentre = (fill[0] * la + fill[1] * lb + fill[2] * lc) / perimeter;
    let area = (fill[1] - fill[0]).perp_dot(fill[2] - fill[0]).abs() / 2.0;
    let inradius = 2.0 * area / perimeter;
    let grow = (inradius + PLAYER_OUTLINE) / inradius;
    let rim = fill.map(|v| incentre + (v - incentre) * grow);

    // Normalise the outer outline to the unit bounding extent
    // `authored_extent` would measure.
    let half = rim
        .iter()
        .map(|v| v.x.abs().max(v.y.abs()))
        .fold(0.0f32, f32::max);
    let k = 0.5 / half;
    let black = LinearRgba::BLACK.to_f32_array();
    let yellow = LinearRgba::from(PLAYER_FILL).to_f32_array();
    let mut positions = Vec::with_capacity(6);
    let mut colors = Vec::with_capacity(6);
    for v in rim {
        positions.push([v.x * k, -0.05, v.y * k]);
        colors.push(black);
    }
    for v in fill {
        positions.push([v.x * k, 0.0, v.y * k]);
        colors.push(yellow);
    }
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 1.0, 0.0]; 6])
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
    .with_inserted_indices(Indices::U16(vec![0, 2, 1, 3, 5, 4]))
}

/// Read-and-parse one authored PKG through the VFS; `None` on either
/// failure with the cause logged — shared with `oppind`, which binds
/// the same marker package in-world.
pub(crate) fn read_pkg(vfs: &Vfs, logical: &str) -> Option<Pkg> {
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

/// Render authored artwork verbatim over an opaque, city-authored background.
fn map_camera_presentation(ocean_color: [f32; 3]) -> impl Bundle {
    (
        Tonemapping::None,
        Camera {
            order: 1,
            // Automatic MSAA writeback copies the previous camera into every
            // later view, consuming its clear operation. This view owns its
            // opaque background; its output viewport preserves the world outside.
            msaa_writeback: MsaaWriteback::Off,
            clear_color: ClearColorConfig::Custom(Color::srgb(
                ocean_color[0],
                ocean_color[1],
                ocean_color[2],
            )),
            ..default()
        },
    )
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
    cnr: bool,
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

    // Designed player arrowhead — vertex-coloured, so the material is
    // plain white, unlit and double-sided like the authored markers.
    let player_mesh = meshes.add(player_marker_mesh());
    let player_mat = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        unlit: true,
        cull_mode: None,
        ..default()
    });

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

    // The player's heading arrowhead.
    spawn_marker(
        commands,
        &mut report,
        &player_mesh,
        player_mat,
        MarkerRole::Player,
        // Unit mesh extent, enlarged over the shared `IconScale`.
        1.0 / PLAYER_ICON_SCALE,
        Vec3::ZERO,
    );

    // Marker meshes — authored flat XZ shapes shared by every city.
    // Each missing piece degrades its own marker family only: a map
    // without gate dots still draws tiles and the player arrowhead.
    let tri_pkg = read_pkg(vfs, "geometry/hudmap_tri.pkg");
    let square_pkg = read_pkg(vfs, "geometry/hudmap_square.pkg");

    if let Some(tri) = &tri_pkg {
        let extent = authored_extent(tri).max(1.0);
        let mesh = pkg_paint_parts(tri, &mut mats, meshes, &mut missing_prims, 0)
            .into_iter()
            .next()
            .map(|(m, _)| m);
        if let Some(tri_mesh) = mesh {
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
        warn!("geometry/hudmap_tri.pkg unavailable — no opponent map markers");
    }

    if let Some(square) = &square_pkg
        && (race.is_some() || cnr)
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
            if let Some(def) = race {
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
            // The match's three sites; `drive_hud_map` places them
            // from the live match every frame.
            if cnr {
                for (role, paint) in [
                    (MarkerRole::Gold, DOT_GOLD),
                    (MarkerRole::Hideout, DOT_HIDEOUT),
                    (MarkerRole::Bank, DOT_BANK),
                ] {
                    match report.dot_materials.get(paint).cloned() {
                        Some(mat) => spawn_marker(
                            commands,
                            &mut report,
                            &dot_mesh,
                            mat,
                            role,
                            extent,
                            Vec3::ZERO,
                        ),
                        None => warn!(?role, "hudmap_square.pkg has no paint job for this dot"),
                    }
                }
            }
        }
    } else if race.is_some() || cnr {
        warn!("geometry/hudmap_square.pkg unavailable — no gate map markers");
    }

    // The map camera: top-down orthographic on the map layer, drawn over
    // the main camera (order 1) into the authored `Pos`/`Size` viewport
    // (`drive_hud_map` recomputes it per frame — window size is only
    // known there). `Ocean Color` is its clear colour — the authored
    // background for tile-free water areas. No tonemapping: the map is
    // flat unlit artwork, and the world's filmic curve would mute the
    // authored tile colours and marker paints alike.
    commands.spawn((
        owner,
        HudMapCamera,
        Camera3d::default(),
        map_camera_presentation(spec.ocean_color),
        Projection::Orthographic(OrthographicProjection {
            scaling_mode: ScalingMode::Fixed {
                width: 2.0 * spec.zoom_out_dist * mm2_game::hudmap::INSET_VIEW_SCALE,
                height: 2.0 * spec.zoom_out_dist * mm2_game::hudmap::INSET_VIEW_SCALE,
            },
            near: 0.0,
            far: CAMERA_LIFT * 4.0,
            ..OrthographicProjection::default_3d()
        }),
        RenderLayers::layer(MAP_LAYER),
        Transform::from_translation(Vec3::Y * (report.marker_y + CAMERA_LIFT))
            .looking_at(Vec3::new(0.0, report.marker_y, 0.0), Vec3::NEG_Z),
    ));
    let frame = commands
        .spawn((
            owner,
            HudMapFrame,
            Visibility::Hidden,
            Node {
                position_type: PositionType::Absolute,
                border: UiRect::all(Val::Px(6.0)),
                ..default()
            },
            BorderColor::all(Color::srgb(0.055, 0.065, 0.08)),
        ))
        .id();
    commands.spawn((
        owner,
        ChildOf(frame),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(-5.0),
            top: Val::Px(-5.0),
            right: Val::Px(-5.0),
            bottom: Val::Px(-5.0),
            border: UiRect::all(Val::Px(1.0)),
            ..default()
        },
        BorderColor::all(Color::srgb(0.38, 0.42, 0.46)),
    ));
    commands.spawn((
        owner,
        ChildOf(frame),
        BackgroundColor(Color::srgb(1.0, 0.64, 0.15)),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(10.0),
            bottom: Val::Px(-4.0),
            width: Val::Px(36.0),
            height: Val::Px(2.0),
            ..default()
        },
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
// A Bevy system: each parameter is one injected resource or query, and the
// pad map is one more of them.
#[allow(clippy::too_many_arguments)]
pub fn hudmap_input(
    keys: Res<ButtonInput<KeyCode>>,
    pads: Query<&Gamepad>,
    windows: Query<&Window>,
    cam_mode: Res<CameraMode>,
    mut session: ResMut<Session>,
    mut control: ResMut<SessionControl>,
    controls: Option<Res<crate::controls::ControlSettings>>,
    map: Option<ResMut<HudMap>>,
) {
    use crate::controls::DriveAction;
    use crate::input::control_just_pressed;
    let pressed =
        |action| control_just_pressed(&keys, &pads, &windows, controls.as_deref(), action);
    let Some(mut map) = map else { return };
    if map.is_stale(session.generation()) {
        return;
    }
    // In the dev free camera `E`/`Q` are the vertical axes
    // (`camera::free_fly`) — the map yields them there rather than
    // double-binding both meanings on one key. The chase camera, where
    // the documented controls live, keeps them. The pad's zoom
    // (DPadLeft) yields under the same gate: free-fly keeps its own
    // meaning for the key's device family either way.
    let free_cam = *cam_mode == CameraMode::Free;
    match *session.phase() {
        SessionPhase::Playing => {
            if pressed(DriveAction::MapView) {
                map.cycle_view();
            }
            if !free_cam && pressed(DriveAction::MapZoom) {
                map.toggle_zoom();
            }
            if pressed(DriveAction::MapRotate) {
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
/// the authored size fractions with the designed lower-left anchor;
/// `Large` is the same corner anchor at the
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
    // Mirror the tuned horizontal anchor so the new driving dial owns
    // the lower-right corner; large maps grow inward from the lower-left.
    (
        Vec2::new((window.x - origin_px.x - size_px.x).max(0.0), origin_px.y),
        size_px,
    )
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

type MapFrames<'w, 's> = Query<
    'w,
    's,
    (&'static mut Node, &'static mut Visibility),
    (With<HudMapFrame>, Without<HudMapMarker>),
>;

/// Width of the bezel's border in layout pixels at a UI scale of 1.
const FRAME_BORDER: f32 = 6.0;

/// The bezel's `(left, top, width, height)` in layout pixels so that, once
/// layout multiplies by `physical_per_px` (window scale factor times
/// [`UiScale`]), it covers the physical `vp_pos`/`vp_size` viewport plus a
/// border of `FRAME_BORDER` logical pixels (`ui_scale` undoes the border's
/// own growth).
fn frame_rect(vp_pos: Vec2, vp_size: Vec2, physical_per_px: f32, ui_scale: f32) -> Vec4 {
    let border = FRAME_BORDER / ui_scale;
    Vec4::new(
        vp_pos.x / physical_per_px - border,
        vp_pos.y / physical_per_px - border,
        vp_size.x / physical_per_px + 2.0 * border,
        vp_size.y / physical_per_px + 2.0 * border,
    )
}

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
/// alike. The `H` HUD gate suppresses the corner views with the rest
/// of the layer (F22-A.3 — `mmHudMap` is an `mmHUD` member); the
/// full-screen *pause* map is a menu surface, not a driving
/// instrument, so it stays up under `Paused` even then.
#[allow(clippy::too_many_arguments)]
pub fn drive_hud_map(
    time: Res<Time>,
    session: Res<Session>,
    hud: Res<crate::hud::HudVisible>,
    map: Option<ResMut<HudMap>>,
    report: Option<Res<HudMapReport>>,
    race: Option<Res<RaceState>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    ui_scale: Option<Res<UiScale>>,
    cnr: crate::cnrhud::CnrScene,
    participants: MapParticipants,
    mut camera: MapCam,
    mut markers: MapMarkers,
    mut frames: MapFrames,
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

    for (_, mut visibility) in &mut frames {
        *visibility = Visibility::Hidden;
    }
    // Camera: parked above the local player, north up or heading up.
    let player_pose = local.map(|(_, _, gt, ..)| (gt.translation(), yaw_of(gt)));
    for (mut xf, mut cam, mut proj) in &mut camera {
        let Some((pos, yaw)) = player_pose else {
            cam.is_active = false;
            continue;
        };
        // The corner views are HUD instruments — off with the layer —
        // while `fullscreen` is the pause overlay's surface and exempt.
        // A minimized client has a 0x0 window: no inset fits, and the
        // 1-pixel viewport floor below would overrun the target.
        let minimized = windows
            .iter()
            .next()
            .is_some_and(crate::input::window_minimized);
        cam.is_active = map.visible() && (hud.0 || map.fullscreen) && !minimized;
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
        // up. The view half-extent is the eased zoom (scaled down for
        // the corner views) — the projection rectangle follows the
        // viewport's aspect so the map does not stretch.
        let win = windows.iter().next().map(|w| {
            Vec2::new(
                w.resolution.physical_width() as f32,
                w.resolution.physical_height() as f32,
            )
        });
        let scale_factor = windows.iter().next().map_or(1.0, Window::scale_factor);
        let (mut vp_pos, vp_size) = if map.fullscreen {
            (Vec2::ZERO, win.unwrap_or(Vec2::ONE))
        } else {
            inset_rect(map.view, &map.spec, win.unwrap_or(Vec2::new(1280.0, 960.0)))
        };
        if !map.fullscreen {
            // Leave breathing room for the bezel, in logical pixels even on Retina.
            vp_pos.x = (vp_pos.x + 12.0 * scale_factor)
                .min(win.unwrap_or(Vec2::new(1280.0, 960.0)).x - vp_size.x);
            vp_pos.y = (vp_pos.y - 18.0 * scale_factor).max(0.0);
            // The camera viewport is physical pixels and ignores `UiScale`,
            // while every `Val::Px` of the frame is multiplied by it (the
            // text-size setting), so divide it back out to keep the bezel
            // on the map.
            let ui = ui_scale.as_ref().map_or(1.0, |s| s.0).max(0.01);
            let rect = frame_rect(vp_pos, vp_size, scale_factor * ui, ui);
            for (mut node, mut visibility) in &mut frames {
                node.left = Val::Px(rect.x);
                node.top = Val::Px(rect.y);
                node.width = Val::Px(rect.z);
                node.height = Val::Px(rect.w);
                node.border = UiRect::all(Val::Px(FRAME_BORDER / ui));
                *visibility = Visibility::Visible;
            }
        }
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
            let height = 2.0 * map.view_half_extent();
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
    let cnr_objective = cnr.objective();
    for (marker, mut xf, mut vis, mut mat) in &mut markers {
        match marker.role {
            MarkerRole::Player => match player_pose {
                Some((pos, yaw)) => {
                    xf.translation = Vec3::new(pos.x, report.marker_y + PLAYER_LIFT, pos.z);
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
            MarkerRole::Gold | MarkerRole::Hideout | MarkerRole::Bank => {
                // The match's sites, off the same objective the arrow
                // reads; the gold has no dot while the local car holds it
                // (and none for a carrier out of view).
                let at = cnr_objective.and_then(|o| match marker.role {
                    MarkerRole::Gold => o.gold,
                    MarkerRole::Hideout => Some(o.hideout),
                    _ => Some(o.bank),
                });
                match at {
                    Some(at) => {
                        xf.translation = Vec3::new(at.x, report.marker_y, at.z);
                        *vis = Visibility::Visible;
                    }
                    None => *vis = Visibility::Hidden,
                }
            }
        }
        let s = (icon_scale / marker.extent).max(0.0);
        xf.scale = Vec3::splat(s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authored_artwork_camera_owns_opaque_background_without_tone_curve() {
        let mut world = World::new();
        for ocean in [[0.084, 0.7, 0.94], [0.25, 0.5, 0.75]] {
            let entity = world
                .spawn((Camera3d::default(), map_camera_presentation(ocean)))
                .id();
            assert_eq!(world.get::<Tonemapping>(entity), Some(&Tonemapping::None));
            let camera = world.get::<Camera>(entity).unwrap();
            assert_eq!(camera.order, 1);
            assert!(matches!(camera.msaa_writeback, MsaaWriteback::Off));
            let ClearColorConfig::Custom(clear) = camera.clear_color else {
                panic!("map must clear its background");
            };
            assert_eq!(
                clear.to_srgba().to_f32_array(),
                [ocean[0], ocean[1], ocean[2], 1.0]
            );
        }
    }

    #[test]
    fn frame_rect_matches_the_viewport_at_every_ui_scale() {
        let (pos, size) = (Vec2::new(300.0, 120.0), Vec2::new(268.0, 240.0));
        for (sf, ui) in [(1.0_f32, 1.0_f32), (2.0, 1.25), (1.0, 1.5), (2.0, 1.5)] {
            let r = frame_rect(pos, size, sf * ui, ui);
            // Back in physical pixels, exactly as layout multiplies it.
            let phys = |v: f32| v * sf * ui;
            let border = phys(FRAME_BORDER / ui);
            assert!((phys(r.x) + border - pos.x).abs() < 1e-3, "{sf} {ui}");
            assert!((phys(r.y) + border - pos.y).abs() < 1e-3);
            assert!((phys(r.z) - 2.0 * border - size.x).abs() < 1e-3);
            assert!((phys(r.w) - 2.0 * border - size.y).abs() < 1e-3);
        }
    }

    /// The retail-shaped spec the layout legs frame against (both
    /// stock cities author this `Pos`/`Size` shape).
    fn retail_shaped_spec() -> HudMapSpec {
        HudMapSpec::parse(
            "mmHudMap {
Size 0.21 0.25
Pos 0.78 0.75
ZoomIn 0
Approach Rate 1.2
ZoomInDist 577
ZoomOutDist 1195
IconScaleMin 34
IconScaleMax 52
ZoomInDistFS 786
ZoomOutDistFS 1581
IconScaleMinFS 15
IconScaleMaxFS 18
Ocean Color 0.084 0.7 0.94
}",
        )
        .unwrap()
    }

    /// F22-AC06 (scaling half): no clipped, unreadable corner map at
    /// any supported text size (the `UiScale` the 100/125/150 % rows
    /// apply), any window scale factor, any window size — small
    /// window, 4:3, 16:9, 21:9 ultrawide, 32:9 super-ultrawide and
    /// 4K — in both inset views. The composition `drive_hud_map` performs is reproduced
    /// verbatim (viewport from the authored fractions plus the
    /// breathing-room offset, bezel through `frame_rect`), and the
    /// matrix asserts the two invariants a driver can see: the
    /// viewport is a real sub-rectangle of the window, and the bezel
    /// multiplied back to physical pixels covers that viewport exactly
    /// while staying on screen. Fullscreen needs no bezel — the pause
    /// map owns the whole window — so the matrix is the inset contract.
    #[test]
    fn the_bezel_matrix_fits_every_text_size_and_window() {
        let spec = retail_shaped_spec();
        let windows = [
            (800.0_f32, 600.0_f32), // small window
            (1280.0, 960.0),        // 4:3
            (1920.0, 1080.0),       // 16:9
            (2560.0, 1080.0),       // 21:9 ultrawide
            (3440.0, 1440.0),       // 21.5:9 ultrawide panel
            (5120.0, 1440.0),       // 32:9 super-ultrawide
            (3840.0, 2160.0),       // 4K
        ];
        let text_sizes = [1.0_f32, 1.25, 1.5];
        let scale_factors = [1.0_f32, 2.0];
        for (win_x, win_y) in windows {
            let window = Vec2::new(win_x, win_y);
            for view in [MapView::Inset, MapView::Large] {
                let (raw_pos, vp_size) = inset_rect(view, &spec, window);
                assert!(
                    vp_size.x >= 1.0 && vp_size.y >= 1.0,
                    "{view:?} {window:?}: the viewport never degenerates"
                );
                for sf in scale_factors {
                    // The production composition: the breathing-room
                    // offset `drive_hud_map` applies after `inset_rect`.
                    let vp_pos = Vec2::new(
                        (raw_pos.x + 12.0 * sf).min(win_x - vp_size.x),
                        (raw_pos.y - 18.0 * sf).max(0.0),
                    );
                    assert!(
                        vp_pos.x >= 0.0
                            && vp_pos.y >= 0.0
                            && vp_pos.x + vp_size.x <= win_x + 1e-3
                            && vp_pos.y + vp_size.y <= win_y + 1e-3,
                        "{view:?} {window:?} sf{sf}: viewport {vp_pos:?} {vp_size:?} \
                         is a sub-rectangle of the window"
                    );
                    for ui in text_sizes {
                        let r = frame_rect(vp_pos, vp_size, sf * ui, ui);
                        // Back to physical pixels, exactly as layout
                        // multiplies the node.
                        let (l, t) = (r.x * sf * ui, r.y * sf * ui);
                        let (w, h) = (r.z * sf * ui, r.w * sf * ui);
                        assert!(
                            l <= vp_pos.x + 1e-3
                                && t <= vp_pos.y + 1e-3
                                && l + w >= vp_pos.x + vp_size.x - 1e-3
                                && t + h >= vp_pos.y + vp_size.y - 1e-3,
                            "{view:?} {window:?} sf{sf} ui{ui}: the bezel must wrap \
                             the viewport (bezel {l},{t} {w}×{h} vs viewport {vp_pos:?} {vp_size:?})"
                        );
                        assert!(
                            l >= -1e-3 && t >= -1e-3,
                            "{view:?} {window:?} sf{sf} ui{ui}: bezel spills off the \
                             top/left ({l}, {t})"
                        );
                        assert!(
                            l + w <= win_x + 1e-3 && t + h <= win_y + 1e-3,
                            "{view:?} {window:?} sf{sf} ui{ui}: bezel spills off the \
                             bottom/right ({}, {})",
                            l + w,
                            t + h
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn inset_fullscreen_inset_preserves_camera_artwork_policy() {
        let spec = HudMapSpec::parse(
            "mmHudMap {
Size 0.21 0.25
Pos 0.78 0.75
ZoomIn 0
Approach Rate 1.2
ZoomInDist 577
ZoomOutDist 1195
IconScaleMin 34
IconScaleMax 52
ZoomInDistFS 786
ZoomOutDistFS 1581
IconScaleMinFS 15
IconScaleMaxFS 18
Ocean Color 0.084 0.7 0.94
}",
        )
        .unwrap();
        let ocean = spec.ocean_color;
        let mut app = App::new();
        app.insert_resource(Time::<()>::default())
            .insert_resource(Session::new())
            .insert_resource(crate::hud::HudVisible(true))
            .insert_resource(HudMap::new(spec, 0))
            .insert_resource(HudMapReport {
                spec_path: String::new(),
                pkg_path: String::new(),
                tiles: 1,
                markers: 0,
                absent: None,
                dot_materials: Vec::new(),
                marker_y: 5.0,
            })
            .add_systems(Update, drive_hud_map);
        app.world_mut().spawn((
            PrimaryWindow,
            Window {
                resolution: bevy::window::WindowResolution::new(1280, 960),
                ..default()
            },
        ));
        app.world_mut().spawn((
            Player {
                id: mm2_game::PlayerId(0),
                control: PlayerControl::Local,
            },
            GlobalTransform::from_translation(Vec3::new(10.0, 0.0, 20.0)),
        ));
        let camera = app
            .world_mut()
            .spawn((
                HudMapCamera,
                Camera3d::default(),
                map_camera_presentation(ocean),
                Transform::default(),
                Projection::Orthographic(OrthographicProjection::default_3d()),
            ))
            .id();
        app.insert_resource(UiScale(1.5));
        let frame = app
            .world_mut()
            .spawn((HudMapFrame, Node::default(), Visibility::Hidden))
            .id();
        app.update();
        let inset = app
            .world()
            .get::<Camera>(camera)
            .unwrap()
            .viewport
            .clone()
            .unwrap();
        assert_eq!(inset.physical_size, UVec2::new(268, 240));
        // At a 150% text size the bezel still wraps the physical viewport:
        // layout multiplies its pixels by `UiScale`, the camera does not.
        let node = app.world().get::<Node>(frame).unwrap();
        let (Val::Px(left), Val::Px(width), Val::Px(border)) =
            (node.left, node.width, node.border.left)
        else {
            panic!("the bezel is laid out in pixels");
        };
        assert!((left * 1.5 + border * 1.5 - inset.physical_position.x as f32).abs() < 1.0);
        assert!((width * 1.5 - 2.0 * border * 1.5 - inset.physical_size.x as f32).abs() < 1.0);
        for fullscreen in [true, false] {
            app.world_mut().resource_mut::<HudMap>().fullscreen = fullscreen;
            app.update();
            let cam = app.world().get::<Camera>(camera).unwrap();
            assert!(cam.is_active);
            assert!(matches!(cam.msaa_writeback, MsaaWriteback::Off));
            assert_eq!(
                app.world().get::<Tonemapping>(camera),
                Some(&Tonemapping::None)
            );
            if fullscreen {
                assert!(cam.viewport.is_none());
            } else {
                assert_eq!(
                    cam.viewport.as_ref().unwrap().physical_size,
                    inset.physical_size
                );
            }
        }
    }
}
