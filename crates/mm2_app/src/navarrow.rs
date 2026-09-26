//! The authored navigation arrow (F22-A.5; RACE-6's green compass
//! arrow — the `mmArrow` instrument the recovered `mmHUD` layout
//! owns). The instrument binds the authored `geometry/hudarrow*.pkg`
//! arrow mesh and its `s_hudarrow_*` texture tiles through the VFS
//! rather than drawing a stand-in UI needle — the same authored
//! content the original instrument draws.
//!
//! What is original here: the arrow *artwork* (the pkg's chevron mesh
//! authored flat in XZ, tip toward −Z, pivot at the local origin) and
//! its *color rule* — each package carries two paint jobs,
//! `[family colour, s_hudarrow_yellow]`, matching RACE-6's documented
//! "green while the target is ahead, yellow when it is behind" with
//! the event-family tile the exe authored per mode (`hudarrow01`
//! green for Checkpoint/Circuit, `hudarrow_blitz01` red for Blitz,
//! `hudarrow_cc01` violet for Crash Course). What stays designed
//! (DSN-8 updated): the on-screen size and placement, since no retail
//! capture or recovered draw body pins them — the arrow keeps the
//! needle's top-centre slot.
//!
//! The pkg geometry is authored as a flat XZ billboard, so the
//! instrument rasterizes each paint job's triangles once at spawn
//! into an `ARROW_CANVAS_PX` sprite (top-down projection, mesh origin
//! at canvas centre so [`update_nav_arrow`]'s rotation pivots
//! exactly where the authored transform would) and the HUD node
//! swaps between the two bound sprites — paint 0 ahead, paint 1
//! behind — instead of tinting. A missing package, unparseable pkg,
//! or missing/undecodable texture records `absent` on the
//! [`NavArrowReport`] — never substitute art.
//!
//! - [`spawn_nav_arrow`] runs in `load_session_world`'s event arm
//!   with the event's table so the family variant binds.
//! - [`update_nav_arrow`] owns visibility, rotation and the
//!   ahead/behind sprite pick every frame off authoritative race
//!   state, hidden under the `H` gate like every `mmHUD` member;
//!   `camera::retarget_hud` pins it to the active world camera.
//! - [`nav_target_input`] keeps the X/Z target cycling (the original
//!   X/S pair is remapped because `S` is brake under the enhanced
//!   WASD map — DSN-8).

use std::collections::HashMap;

use avian3d::prelude::{Position, Rotation};
use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use mm2_assets::Vfs;
use mm2_formats::pkg::{Pkg, PkgShader, lod_split};
use mm2_game::{
    EventTableKind, ParticipantState, RacePhase, RaceProgress, RaceState, Session, SessionEntity,
    TargetSelection, cycle_target, navigation_target, relative_bearing,
};

/// Square raster canvas the authored mesh renders into — also the
/// node's screen size. The mesh's authored origin sits at the canvas
/// centre so [`update_nav_arrow`]'s `UiTransform` rotation pivots the
/// arrow where the pkg pivots. Designed size (DSN-8): ~1.5× the dev
/// needle's footprint, inside the top-centre band the mirror strip
/// already overlaps.
pub const ARROW_CANVAS_PX: u32 = 80;

/// Top edge of the arrow node — the canvas centre lands ~64 px down,
/// the same top-centre slot the dev needle occupied.
const ARROW_TOP_PX: f32 = 24.0;

/// Canvas padding kept between the authored mesh's outer radius and
/// the sprite edge so a rotated arrow never clips its own raster.
const ARROW_PAD_PX: f32 = 2.0;

/// The authored arrow package each event family binds (paint job 0 is
/// the family's "ahead" tile; job 1 is the shared `s_hudarrow_yellow`
/// "behind" tile). `hudarrow01` is the generic green arrow — it covers
/// Checkpoint and Circuit alike (the instrument hides under `Ordered`
/// rules anyway, so a Circuit race never shows it).
fn arrow_pkg_path(table: EventTableKind) -> &'static str {
    match table {
        EventTableKind::Blitz => "geometry/hudarrow_blitz01.pkg",
        EventTableKind::CrashCourse => "geometry/hudarrow_cc01.pkg",
        _ => "geometry/hudarrow01.pkg",
    }
}

/// Marker on the session-owned navigation-arrow node: an
/// `ARROW_CANVAS_PX`-square image at screen top-center whose
/// `UiTransform.rotation` is the signed bearing to the player's
/// current objective (clockwise, `+` = right) and whose `ImageNode`
/// swaps between the bound ahead/behind sprites.
#[derive(Component)]
pub struct NavArrow;

/// The two bound sprite handles on a [`NavArrow`] node: the event
/// family's paint job 0 while the target is in the forward
/// half-plane, paint job 1 (the shared authored yellow) behind.
#[derive(Component)]
pub struct NavArrowSprites {
    /// Paint job 0's sprite — the family's ahead tile
    /// (`s_hudarrow_{green,red,violet}`).
    pub ahead: Handle<Image>,
    /// Paint job 1's sprite — `s_hudarrow_yellow`; equals `ahead` when
    /// the package authors a single paint job.
    pub behind: Handle<Image>,
}

/// Which sprite the arrow shows — the `arr=` record field's live
/// state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArrowFacing {
    /// Target in the forward half-plane — the family's ahead tile.
    Ahead,
    /// Target in the rear half-plane — the authored yellow tile
    /// (RACE-6's documented colour flip).
    Behind,
}

impl ArrowFacing {
    fn token(self) -> &'static str {
        match self {
            Self::Ahead => "ahead",
            Self::Behind => "behind",
        }
    }
}

/// Session-scoped report for the arrow instrument — the `arr=`
/// record field's source. Inserted for every event session (bound or
/// not — `absent` says why not); cruise and dev-world sessions get no
/// report, so their records stay bit-identical.
#[derive(Resource)]
pub struct NavArrowReport {
    /// `geometry/hudarrow*.pkg` logical path attempted.
    pub pkg_path: String,
    /// Paint-job sprites bound (2 when the authored ahead + behind
    /// pair both rasterized).
    pub sprites: usize,
    /// Why nothing bound: `missing-pkg`, `unparseable-pkg`,
    /// `no-shaders`, `missing-texture`, `undecodable-texture`,
    /// `no-geometry`, `bad-index`.
    pub absent: Option<&'static str>,
    /// Which way the arrow faces on the last drive pass — kept live
    /// even while the `H` gate hides the node, so the field shows
    /// demand like `tmr=`'s `display`. `None` when no live race
    /// wants an arrow.
    pub facing: Option<ArrowFacing>,
}

impl NavArrowReport {
    /// The `arr=` record field body: `<pkg stem>/<ahead|behind|off>`
    /// when bound, or `absent:<why>` when the authored package never
    /// loaded.
    pub fn smoke_detail(&self) -> String {
        match self.absent {
            Some(why) => format!("absent:{why}"),
            None => format!(
                "{}/{}",
                self.pkg_path
                    .rsplit('/')
                    .next()
                    .unwrap_or(&self.pkg_path)
                    .trim_end_matches(".pkg"),
                self.facing.map(ArrowFacing::token).unwrap_or("off")
            ),
        }
    }
}

/// CPU-readable RGBA8 texture for the rasterizer — top-down rows, the
/// convention `city::decode_*` produces (so the pkg's authored `v` is
/// complemented when sampling, the same complement `emit_strip`
/// applies).
#[derive(Clone)]
pub(crate) struct RgbaTex {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl RgbaTex {
    /// Nearest sample with repeat wrapping — the pipeline default
    /// sampler `load_image` stamps.
    fn sample(&self, u: f32, v: f32) -> [u8; 4] {
        let x = (u.rem_euclid(1.0) * self.width as f32) as u32 % self.width;
        let y = (v.rem_euclid(1.0) * self.height as f32) as u32 % self.height;
        let i = ((y * self.width + x) * 4) as usize;
        [
            self.rgba[i],
            self.rgba[i + 1],
            self.rgba[i + 2],
            self.rgba[i + 3],
        ]
    }
}

/// Pull RGBA pixels out of a decoded [`Image`] for the rasterizer —
/// `None` when the decoded format is not a plain 8-bit RGBA/BGRA
/// (e.g. a mod's compressed `ktx2`), which binds as
/// `undecodable-texture` rather than a misread.
fn image_pixels(img: &Image) -> Option<RgbaTex> {
    let size = img.texture_descriptor.size;
    let data = img.data.as_ref()?;
    let rgba = match img.texture_descriptor.format {
        TextureFormat::Rgba8UnormSrgb | TextureFormat::Rgba8Unorm => data.clone(),
        TextureFormat::Bgra8UnormSrgb | TextureFormat::Bgra8Unorm => data
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|&[b, g, r, a]| [r, g, b, a])
            .collect(),
        _ => return None,
    };
    Some(RgbaTex {
        width: size.width,
        height: size.height,
        rgba,
    })
}

/// Rasterize one paint job of the arrow package into a square
/// `canvas`-pixel RGBA sprite. The authored mesh is a flat XZ
/// billboard (tip toward −Z, which lands at the sprite's top — the
/// bearing-0 "up" [`update_nav_arrow`] rotates from). Per-pixel
/// `y` resolves draw order where triangles overlap, preserving the
/// authored layering the file's section order implies. Returns the
/// rasterized image, or `Err` with the `absent` token when the
/// package cannot produce pixels: `missing-texture` /
/// `undecodable-texture` / `no-geometry`.
fn rasterize_paint(
    pkg: &Pkg,
    paint: usize,
    canvas: usize,
    tex: &mut impl FnMut(&str) -> Result<RgbaTex, &'static str>,
) -> Result<Image, &'static str> {
    let Some(shaders) = pkg.shaders() else {
        return Err("no-shaders");
    };
    let spj = shaders.shaders_per_paint_job.max(1) as usize;

    // Rasterize the same chunks `city.rs` `pkg_to_parts` would draw:
    // the best-LOD chunk per stem (a package can carry `_l`/`_m`/`_h`
    // variants whose triangles would overlay), with shadow/damage
    // stand-ins excluded. The authored `hudarrow*` packages ship a
    // single chunk; the filter matters for mod substitutes.
    let mut best: HashMap<String, (u8, &str)> = HashMap::new();
    for (name, _geo) in pkg.geometries() {
        let (stem, rank) = lod_split(name);
        let entry = best.entry(stem).or_insert((rank, name));
        if rank > entry.0 {
            *entry = (rank, name);
        }
    }
    let drawable = |name: &str| {
        let (stem, _) = lod_split(name);
        !stem.contains("shadow")
            && !stem.contains("dmg")
            && best.get(&stem).map(|(_, n)| *n) == Some(name)
    };

    // The authored radius maps to the canvas half-extent so the whole
    // mesh fits and the origin stays dead centre.
    let mut radius = 0.0f32;
    for (name, geo) in pkg.geometries() {
        if !drawable(name) {
            continue;
        }
        for section in &geo.sections {
            for strip in &section.strips {
                for v in &strip.vertices {
                    radius = radius.max(v.position[0].hypot(v.position[2]));
                }
            }
        }
    }
    if radius <= 0.0 {
        return Err("no-geometry");
    }
    let half = canvas as f32 * 0.5;
    let scale = (half - ARROW_PAD_PX) / radius;

    let mut px = vec![0u8; canvas * canvas * 4];
    // Per-pixel winning mesh `y` — the flat billboard's overlap order.
    let mut depth = vec![f32::NEG_INFINITY; canvas * canvas];
    let mut covered = false;

    for (name, geo) in pkg.geometries() {
        if !drawable(name) {
            continue;
        }
        for section in &geo.sections {
            // The production convention (`city.rs` `shader_at`): a
            // section's shader offset indexes within the paint job —
            // `paint * per_job + offset` — and a negative offset is
            // untextured.
            let shader = if section.shader_offset < 0 {
                None
            } else {
                let idx = paint
                    .saturating_mul(spj)
                    .saturating_add(section.shader_offset as usize);
                shaders.shaders.get(idx)
            };
            // Resolve the section's texture lazily — a named texture
            // that cannot decode aborts the whole instrument rather
            // than drawing a half-bound arrow.
            let teximg = match shader {
                Some(s) if !s.texture.is_empty() => Some(tex(&s.texture)?),
                _ => None,
            };
            let tint = shader.map(|s: &PkgShader| s.diffuse).unwrap_or([1.0; 4]);
            for strip in &section.strips {
                if strip.prim_type != mm2_formats::pkg::PRIMTYPE_TRIANGLES {
                    // Only triangle lists are interpreted — the same
                    // skip the production mesh builders make.
                    continue;
                }
                for t in strip.indices.as_chunks::<3>().0 {
                    // File-supplied indices index the strip's own
                    // vertex table — a corrupt or hostile pkg (the VFS
                    // mounts mod overrides above stock) must fail the
                    // spawn, not panic inside it.
                    if t.iter().any(|i| *i as usize >= strip.vertices.len()) {
                        return Err("bad-index");
                    }
                    rasterize_tri(
                        &mut px,
                        &mut depth,
                        canvas,
                        half,
                        scale,
                        strip,
                        *t,
                        teximg.as_ref(),
                        tint,
                        &mut covered,
                    );
                }
            }
        }
    }
    if !covered {
        return Err("no-geometry");
    }
    let mut image = Image::new(
        Extent3d {
            width: canvas as u32,
            height: canvas as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        px,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    // Linear sampling so the rotated sprite filters smoothly — the
    // authored tiles are 8×8 fills, so this only shapes the edges.
    image.sampler = ImageSampler::linear();
    Ok(image)
}

/// Barycentric-fill one strip triangle into `px`: canvas `x` maps the
/// authored `+x`, canvas `y` the authored `+z` (so the −Z tip points
/// up at bearing 0). `w` double-duty as the winding-insensitive
/// inside test and the `y`/`uv` interpolant; `depth` keeps the
/// topmost authored `y` per pixel.
// The fill genuinely needs the canvas buffer, depth buffer, geometry
// and sampling state as separate borrows — splitting them into a
// struct would only obscure the pixel loop.
#[allow(clippy::too_many_arguments)]
fn rasterize_tri(
    px: &mut [u8],
    depth: &mut [f32],
    canvas: usize,
    half: f32,
    scale: f32,
    strip: &mm2_formats::pkg::PkgStrip,
    tri: [u16; 3],
    tex: Option<&RgbaTex>,
    tint: [f32; 4],
    covered: &mut bool,
) {
    let v = |i: u16| -> (f32, f32, f32, [f32; 2]) {
        let v = &strip.vertices[i as usize];
        (
            half + v.position[0] * scale,
            half + v.position[2] * scale,
            v.position[1],
            v.tex_coords.first().copied().unwrap_or([0.0, 0.0]),
        )
    };
    let (x0, y0, h0, uv0) = v(tri[0]);
    let (x1, y1, h1, uv1) = v(tri[1]);
    let (x2, y2, h2, uv2) = v(tri[2]);

    let area = (x1 - x0) * (y2 - y0) - (x2 - x0) * (y1 - y0);
    if area.abs() < 1e-9 {
        return;
    }
    let min_x = x0.min(x1).min(x2).floor().max(0.0) as usize;
    let max_x = x0.max(x1).max(x2).ceil().min(canvas as f32 - 1.0) as usize;
    let min_y = y0.min(y1).min(y2).floor().max(0.0) as usize;
    let max_y = y0.max(y1).max(y2).ceil().min(canvas as f32 - 1.0) as usize;

    for py in min_y..=max_y {
        for pxi in min_x..=max_x {
            let (cx, cy) = (pxi as f32 + 0.5, py as f32 + 0.5);
            let w0 = ((x1 - cx) * (y2 - cy) - (x2 - cx) * (y1 - cy)) / area;
            let w1 = ((x2 - cx) * (y0 - cy) - (x0 - cx) * (y2 - cy)) / area;
            let w2 = 1.0 - w0 - w1;
            const EPS: f32 = 1e-6;
            if w0 < -EPS || w1 < -EPS || w2 < -EPS {
                continue;
            }
            let h = w0 * h0 + w1 * h1 + w2 * h2;
            let i = py * canvas + pxi;
            if h <= depth[i] {
                continue;
            }
            let color = match tex {
                Some(t) => {
                    // Pkg `v` is authored against bottom-up rows;
                    // decoded images are top-down (city.rs's
                    // `1.0 - v` precedent).
                    let u = w0 * uv0[0] + w1 * uv1[0] + w2 * uv2[0];
                    let vtex = w0 * uv0[1] + w1 * uv1[1] + w2 * uv2[1];
                    let [r, g, b, a] = t.sample(u, 1.0 - vtex);
                    [
                        (r as f32 * tint[0]).min(255.0) as u8,
                        (g as f32 * tint[1]).min(255.0) as u8,
                        (b as f32 * tint[2]).min(255.0) as u8,
                        (a as f32 * tint[3]).min(255.0) as u8,
                    ]
                }
                None => [
                    (tint[0].clamp(0.0, 1.0) * 255.0) as u8,
                    (tint[1].clamp(0.0, 1.0) * 255.0) as u8,
                    (tint[2].clamp(0.0, 1.0) * 255.0) as u8,
                    (tint[3].clamp(0.0, 1.0) * 255.0) as u8,
                ],
            };
            depth[i] = h;
            px[i * 4..i * 4 + 4].copy_from_slice(&color);
            *covered = true;
        }
    }
}

/// Spawn the RACE-6 navigation arrow bound to the authored
/// `hudarrow*` package for `table`'s event family. Every texture the
/// mesh's paint jobs name resolves through `crate::city::load_image`
/// — the same VFS lookup (mods can substitute `png`/`ktx2`/`tex`)
/// every other texture takes. A missing/unparseable pkg or a named
/// texture that cannot load aborts the spawn: the report records
/// `absent` and no half-bound node exists. Hidden until a live race
/// hands it a target — [`update_nav_arrow`] owns visibility, rotation
/// and the ahead/behind sprite pick every frame.
pub fn spawn_nav_arrow(
    commands: &mut Commands,
    vfs: &Vfs,
    images: &mut Assets<Image>,
    owner: SessionEntity,
    table: EventTableKind,
) -> NavArrowReport {
    let pkg_path = arrow_pkg_path(table);
    let mut report = NavArrowReport {
        pkg_path: pkg_path.to_string(),
        sprites: 0,
        absent: None,
        facing: None,
    };
    let pkg = match crate::hudmap::read_pkg(vfs, pkg_path) {
        Some(p) => p,
        None => {
            report.absent = Some(if vfs.resolve(pkg_path).is_some() {
                "unparseable-pkg"
            } else {
                "missing-pkg"
            });
            return report;
        }
    };
    let Some(shaders) = pkg.shaders() else {
        report.absent = Some("no-shaders");
        return report;
    };
    // Only paint jobs 0 (ahead) and 1 (behind) bind — the authored
    // pair RACE-6 documents. A shaderless package cannot produce a
    // faithful arrow at all.
    let paints = (shaders.paint_jobs as usize).min(2);
    if paints == 0 {
        report.absent = Some("no-shaders");
        return report;
    }

    // Decode every texture stem the bound paints can name once — the
    // same stem can serve several paint jobs. `resolve_preferred`
    // distinguishes a genuine miss from a decode failure.
    let mut cache: HashMap<String, Result<RgbaTex, &'static str>> = HashMap::new();
    for shader in &shaders.shaders {
        if shader.texture.is_empty() || cache.contains_key(&shader.texture) {
            continue;
        }
        let path = format!("texture/{}", shader.texture);
        let decoded = if vfs
            .resolve_preferred(&path, crate::city::TEXTURE_EXTS)
            .is_none()
        {
            Err("missing-texture")
        } else {
            crate::city::load_image(vfs, &shader.texture)
                .and_then(|(img, _)| image_pixels(&img))
                .ok_or("undecodable-texture")
        };
        cache.insert(shader.texture.clone(), decoded);
    }
    let mut tex = |stem: &str| -> Result<RgbaTex, &'static str> {
        cache.get(stem).cloned().unwrap_or(Err("missing-texture"))
    };

    let mut sprites = Vec::with_capacity(paints);
    for paint in 0..paints {
        match rasterize_paint(&pkg, paint, ARROW_CANVAS_PX as usize, &mut tex) {
            Ok(img) => {
                sprites.push(images.add(img));
                report.sprites += 1;
            }
            Err(why) => {
                report.absent = Some(why);
                return report;
            }
        }
    }
    let ahead = sprites[0].clone();
    let behind = sprites.get(1).cloned().unwrap_or_else(|| ahead.clone());
    commands.spawn((
        owner,
        NavArrow,
        NavArrowSprites { ahead, behind },
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(ARROW_TOP_PX),
            left: Val::Percent(50.0),
            width: Val::Px(ARROW_CANVAS_PX as f32),
            height: Val::Px(ARROW_CANVAS_PX as f32),
            ..default()
        },
        // Percent resolves against the node's own size, so the sprite
        // stays centred on the authored pivot whatever the canvas is.
        UiTransform::from_xy(Val::Percent(-50.0), Val::ZERO),
        ImageNode::new(sprites[0].clone()),
        Visibility::Hidden,
    ));
    report
}

/// `X`/`Z` cycle the arrow's target through the remaining gates
/// (RACE-6). The original binds this to X/S (CTL-1), but `S` is brake
/// under this app's added WASD mapping, so the backward cycle moved to
/// `Z` — an input-map departure, not a rules one (DSN-8). Cycling
/// is allowed while the race counts down: the arrow is already live.
pub fn nav_target_input(
    keys: Res<ButtonInput<KeyCode>>,
    session: Res<Session>,
    race: Option<Res<RaceState>>,
    mut players: Query<(&Position, &RaceProgress, &mut TargetSelection)>,
) {
    let dir = if keys.just_pressed(KeyCode::KeyX) {
        1
    } else if keys.just_pressed(KeyCode::KeyZ) {
        -1
    } else {
        return;
    };
    let Some(race) = race else { return };
    if race.is_stale(session.generation()) || race.phase == RacePhase::Complete {
        return;
    }
    for (pos, progress, mut selection) in &mut players {
        selection.picked = cycle_target(&race.definition, progress, selection.picked, pos.0, dir);
    }
}

/// Drive the arrow to the player's current objective every frame: the
/// node's rotation is the signed bearing to the target (clockwise,
/// so `+` bearing = right), showing the family's ahead sprite while
/// the target is in the forward half-plane and the authored yellow
/// behind (RACE-6). Hidden whenever there is no live target — no
/// race, a stale/complete race, an `Ordered` definition, or a
/// participant already resolved — and while the `H` HUD gate is off
/// (the arrow is an `mmHUD` instrument, F22-A.3). The report's
/// `facing` records the same demand whether or not the node shows.
pub fn update_nav_arrow(
    race: Option<Res<RaceState>>,
    session: Res<Session>,
    hud: Res<crate::hud::HudVisible>,
    mut report: Option<ResMut<NavArrowReport>>,
    players: Query<(&Position, &Rotation, &RaceProgress, &TargetSelection)>,
    mut arrow: Query<
        (
            &mut UiTransform,
            &mut Visibility,
            &mut ImageNode,
            &NavArrowSprites,
        ),
        With<NavArrow>,
    >,
) {
    let live = race
        .filter(|r| !r.is_stale(session.generation()))
        .filter(|r| r.phase != RacePhase::Complete);
    let target = live.and_then(|r| {
        players.iter().find_map(|(pos, rot, progress, selection)| {
            if !matches!(
                progress.state,
                ParticipantState::AwaitingStart | ParticipantState::Racing
            ) {
                return None;
            }
            let target = navigation_target(&r.definition, progress, selection.picked, pos.0)?;
            let target_pos = target.position(&r.definition)?;
            // Heading from the physics rotation, projected to XZ:
            // vehicle forward is local −Z (same convention
            // `relative_bearing` documents).
            let fwd = rot.0 * Vec3::NEG_Z;
            let yaw = (-fwd.x).atan2(-fwd.z);
            Some(relative_bearing(yaw, pos.0, target_pos))
        })
    });
    let facing = target.map(|b| {
        if b.abs() > std::f32::consts::FRAC_PI_2 {
            ArrowFacing::Behind
        } else {
            ArrowFacing::Ahead
        }
    });
    for (mut ui, mut vis, mut image, sprites) in &mut arrow {
        match (target, facing) {
            (Some(b), Some(f)) if hud.0 => {
                *vis = Visibility::Visible;
                ui.rotation = Rot2::radians(b);
                let want = match f {
                    ArrowFacing::Ahead => &sprites.ahead,
                    ArrowFacing::Behind => &sprites.behind,
                };
                if image.image != *want {
                    image.image = want.clone();
                }
            }
            _ => *vis = Visibility::Hidden,
        }
    }
    if let Some(report) = report.as_deref_mut() {
        report.facing = facing;
    }
}
