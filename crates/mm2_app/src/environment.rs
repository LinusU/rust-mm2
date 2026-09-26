//! Authored environment presets → Bevy lighting (F18-A.2).
//!
//! A session's effective [`SessionConditions`] (see
//! `mm2_game::effective_conditions` — authored event params win, the
//! session config is the cruise/dev fallback) select one of the city's
//! sixteen `.ltNN` presets on the measured `NN = tod*4 + weather` grid
//! (WLD-21). [`spawn_environment`] reads the preset through the VFS and
//! binds it:
//!
//! - `Key`/`Fill1`/`Fill2` → three [`DirectionalLight`]s. Directions use
//!   the recovered `setLightDirectionInv` convention
//!   ([`LightSpec::to_light_dir`]; the Bevy forward axis gets
//!   [`LightSpec::travel_dir`]); colours are authored verbatim. Only the
//!   key casts shadows (designed — the fills stand in for bounce light).
//! - `Ambient` → [`GlobalAmbientLight`] from the BGRA-packed colour.
//! - `city/<stem>_fog.csv` → the session's [`DistanceFog`]: the authored
//!   per-preset table whose row index is the same `tod*4 + weather`
//!   slot, verified against mm2hook's recovered `lvlSky`
//!   (`FogColors[16]`/`FogNearClip[16]`/`FogFarClip[16]` indexed by
//!   `TimeWeatherType`). `fog start`/`fog end` bind as a linear
//!   `FogFalloff` — the clip distances the field names describe
//!   (inferred curve shape; the values are authored).
//!
//! The illuminance/ambient-brightness scales are designed mappings —
//! the authored colours carry the relative weight, the constants anchor
//! the rig to the lighting level the city used before presets bound
//! (the previous fixed 15 000 lux sun). A missing or unparseable preset
//! spawns that same pre-preset rig and reports `fallback` — an explicit
//! diagnostic (F18-AC06), never a silent default. The fog channel is
//! independent: a preset that fell back still binds its fog row, and a
//! missing/unparseable/degenerate fog table binds nothing with the
//! reason recorded in [`FogReport::absent`].
//!
//! Deferred: `.cpvs` PVS culling, `.lmap`/`.ldef` semantics —
//! UNK-24.

use bevy::pbr::{DistanceFog, FogFalloff};
use bevy::prelude::*;
use mm2_assets::Vfs;
use mm2_formats::fog::FogTable;
use mm2_formats::lighting::{LightSpec, LightingPreset};
use mm2_formats::pkg::Pkg;
use mm2_formats::sky::SkyDef;
use mm2_game::{SessionConditions, SessionEntity};
use tracing::{info, warn};

/// Illuminance shared by the three authored directional lights
/// (designed scale — authored `Color` carries each light's relative
/// weight; anchored at the sun intensity the city used pre-presets).
const DIRECTIONAL_ILLUMINANCE: f32 = 15_000.0;

/// `GlobalAmbientLight::brightness` for the authored ambient colour
/// (designed scale — calibrated so a typical day ambient lands near the
/// previous fixed ambient; night presets' darker ambients stay dark).
const AMBIENT_BRIGHTNESS: f32 = 2_000.0;

/// Where the session's effective conditions came from — drives the
/// report so evidence can tell an authored event's environment from a
/// configured or player-picked one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConditionsSource {
    /// `SessionConfig::conditions` — cruise/dev fallback (`--weather` /
    /// `--time-of-day`, or the selector-0 default).
    Configured,
    /// The running event's authored `EventParams::conditions` (RACE-2).
    Authored,
    /// The player's `SessionCustomization` picks (RACE-3/RACE-4 menu
    /// options) — beats every other source when present.
    Customized,
}

/// What [`spawn_environment`] did — a session-scoped report resource
/// (inserted by `load_session_world`, removed on teardown).
#[derive(Resource, Debug)]
pub struct EnvironmentReport {
    /// The `ltNN` grid slot the conditions resolved (`tod*4 + weather`).
    pub slot: usize,
    /// Logical path the preset was read from (or attempted at).
    pub path: String,
    /// The parsed preset's authored label (`clear-noon`); `None` on
    /// fallback.
    pub name: Option<String>,
    /// Which conditions source bound the preset.
    pub source: ConditionsSource,
    /// `true` when the preset file was missing or failed to parse and
    /// the fallback rig was spawned — explicit, not a silent default.
    pub fallback: bool,
    /// `LightingPreset::validate()` findings (warned; authored anomalies
    /// do not block a preset).
    pub issues: usize,
    /// The session's authored fog binding (`city/<stem>_fog.csv` row
    /// `slot`), or the explicit reason none bound.
    pub fog: FogReport,
    /// The session's `.sky` dome binding (F18-A.4), or the explicit
    /// reason none spawned.
    pub sky: SkyReport,
}

/// The authored fog row bound for the session, if any.
#[derive(Debug, Clone)]
pub struct FogReport {
    /// Logical path the table was read from (or attempted at).
    pub path: String,
    /// The bound authored row — `None` when [`Self::absent`] names why.
    pub bound: Option<FogSpec>,
    /// `FogTable::diagnostics` + `FogTable::validate()` findings
    /// (warned; authored anomalies do not block the row's binding).
    pub issues: usize,
    /// Why no fog bound — `"missing"`, `"unparseable"`,
    /// `"no row for slot"` or `"degenerate"` — `None` when bound.
    pub absent: Option<&'static str>,
}

/// One authored fog row, ready to bind.
#[derive(Debug, Clone, Copy)]
pub struct FogSpec {
    /// Authored colour channels (0-255, clamped at bind).
    pub color: [f32; 3],
    /// Near clip distance the fog starts at (metres).
    pub start: f32,
    /// Far clip distance the fog is opaque at (metres).
    pub end: f32,
}

impl FogSpec {
    /// The camera component form: a linear `FogFalloff` between the
    /// authored `fog start`/`fog end` clip distances — the fixed-function
    /// fog the recovered `FogNearClip`/`FogFarClip` fields describe
    /// (curve shape inferred; the values are authored). The directional
    /// glow stays disabled (`Color::NONE`) — the tables carry no
    /// directional data.
    pub fn distance_fog(self) -> DistanceFog {
        DistanceFog {
            color: Color::srgb(
                self.color[0] / 255.0,
                self.color[1] / 255.0,
                self.color[2] / 255.0,
            ),
            falloff: FogFalloff::Linear {
                start: self.start,
                end: self.end,
            },
            ..default()
        }
    }
}

/// The session's `.sky` dome binding (F18-A.4), or the explicit reason
/// none spawned. `Default` is the not-attempted state (non-city worlds
/// never run the binding); it reports `sky=none`.
#[derive(Debug, Clone, Default)]
pub struct SkyReport {
    /// Logical path the `.sky` was read from (or attempted at).
    pub path: String,
    /// The authored dome model name (`sky_dome_l`) — present whenever
    /// the `.sky` parsed, even if its pkg did not.
    pub model: Option<String>,
    /// The paint job bound — the session's `tod*4 + weather` slot
    /// modulo the dome's authored paint-job count (the designed
    /// fallback for a dome authoring fewer than 16 jobs).
    pub paint: Option<usize>,
    /// The bound paint job's texture stem — the authored name whether
    /// or not it resolved through the VFS.
    pub texture: Option<String>,
    /// `SkyDef::validate()` findings (warned; authored anomalies do not
    /// block the dome).
    pub issues: usize,
    /// Why no dome spawned — `"missing"`, `"unparseable"`,
    /// `"degenerate"`, `"model unavailable"`, `"model unparseable"` or
    /// `"empty"` — `None` when spawned.
    pub absent: Option<&'static str>,
}

/// The dome's rotating root: [`drive_sky_dome`] re-centres it on the
/// active camera in XZ at the authored `HatYOffset` height and advances
/// the authored rotation (F18-A.4).
#[derive(Component)]
pub struct SkyDome {
    /// Accumulated dome yaw in radians (wraps at τ).
    pub angle: f32,
    /// Authored `RotationRate`, read as radians/second (designed — the
    /// recovered `lvlSky` name fits but its units are unverified,
    /// UNK-24).
    pub rotation_rate: f32,
    /// World-space dome-centre height — the authored `HatYOffset`,
    /// unscaled.
    pub y: f32,
}

/// World radius the authored ~43 m dome mesh is scaled to (designed —
/// the `.sky` record carries no scale; 900 m sits inside the default
/// 1 000 m camera far plane and past every authored fog band's end, so
/// the dome backdrop never clips a building the player can still see).
const SKY_DOME_RADIUS: f32 = 900.0;

impl EnvironmentReport {
    /// The smoke record's `env=` field, e.g. `lt04(clear-noon)` or
    /// `lt07(fallback)`, followed by ` fog=<start>-<end>` when an
    /// authored fog row bound or ` fog=none` when it did not, then
    /// ` sky=<model>:<texture>` when a `.sky` dome spawned or
    /// ` sky=none` when it did not (F18-A.4).
    pub fn smoke_detail(&self) -> String {
        let tag = self.name.as_deref().unwrap_or("fallback");
        let fog = match &self.fog.bound {
            Some(f) => format!(" fog={}-{}", f.start, f.end),
            None => " fog=none".to_string(),
        };
        let sky = match (&self.sky.model, &self.sky.texture) {
            (Some(m), Some(t)) if self.sky.absent.is_none() => format!(" sky={m}:{t}"),
            _ => " sky=none".to_string(),
        };
        format!("lt{:02}({tag}){fog}{sky}", self.slot)
    }
}

/// One authored light → a [`DirectionalLight`] + orientation.
/// `shadows` marks the key light — the fills never cast.
fn light_bundle(spec: &LightSpec, shadows: bool) -> impl Bundle {
    let dir = Vec3::from_array(spec.travel_dir());
    // A non-finite or degenerate authored direction leaves the light
    // pointing at the default axis rather than panicking the spawn —
    // `validate()` already counts it as an issue.
    let rotation = if dir.is_finite() && dir.length_squared() > 1e-6 {
        Quat::from_rotation_arc(Vec3::NEG_Z, dir.normalize())
    } else {
        Quat::IDENTITY
    };
    // Negative/non-finite channels are flagged by `validate()`; they are
    // clamped here because a negative light is meaningless, not useful.
    let [r, g, b] = spec
        .color
        .map(|c| if c.is_finite() { c.max(0.0) } else { 0.0 });
    (
        DirectionalLight {
            illuminance: DIRECTIONAL_ILLUMINANCE,
            color: Color::srgb(r, g, b),
            shadow_maps_enabled: shadows,
            ..default()
        },
        Transform::from_rotation(rotation),
    )
}

/// The pre-preset rig the city used before `.ltNN` bound — also the
/// fallback for a missing/unparseable preset, reported as such.
fn spawn_fallback_lights(commands: &mut Commands, owner: SessionEntity) {
    commands.spawn((
        owner,
        DirectionalLight {
            illuminance: DIRECTIONAL_ILLUMINANCE,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_rotation(Quat::from_euler(EulerRot::YXZ, 0.6, -0.9, 0.0)),
    ));
    commands.insert_resource(GlobalAmbientLight {
        color: Color::srgb(0.7, 0.75, 0.85),
        brightness: 400.0,
        affects_lightmapped_meshes: false,
    });
}

/// Bind the session's lighting preset for `psdl_path`'s city.
///
/// The preset file sits beside the PSDL (`city/london.psdl` →
/// `city/london.ltNN`), so mod-provided cities resolve their own
/// presets through the same VFS precedence as every other resource.
/// Missing/unparseable content warns and falls back — reported via the
/// returned [`EnvironmentReport`], which the caller inserts as a
/// session-scoped resource.
pub fn spawn_environment(
    commands: &mut Commands,
    vfs: &Vfs,
    psdl_path: &str,
    conditions: SessionConditions,
    source: ConditionsSource,
    owner: SessionEntity,
) -> EnvironmentReport {
    let slot = conditions.time_of_day.get() as usize * 4 + conditions.weather.get() as usize;
    let base = psdl_path.strip_suffix(".psdl").unwrap_or(psdl_path);
    let path = format!("{base}.lt{slot:02}");
    let fog_path = format!("{base}_fog.csv");
    let mut report = EnvironmentReport {
        slot,
        path: path.clone(),
        name: None,
        source,
        fallback: false,
        issues: 0,
        fog: FogReport {
            path: fog_path,
            bound: None,
            issues: 0,
            absent: None,
        },
        sky: SkyReport::default(),
    };

    let preset = match vfs.read_path(&path) {
        Ok((bytes, _)) => match LightingPreset::parse(&String::from_utf8_lossy(&bytes)) {
            Ok(p) => Some(p),
            Err(e) => {
                warn!(path = %path, error = %e, "lighting preset failed to parse");
                None
            }
        },
        Err(e) => {
            warn!(path = %path, error = %e, "lighting preset unavailable");
            None
        }
    };

    match preset {
        Some(preset) => {
            let issues = preset.validate();
            report.issues = issues.len();
            for issue in &issues {
                warn!(path = %path, issue = ?issue, "lighting preset validation issue");
            }
            report.name = Some(preset.name.clone());
            commands.spawn((owner, light_bundle(&preset.key, true)));
            commands.spawn((owner, light_bundle(&preset.fill1, false)));
            commands.spawn((owner, light_bundle(&preset.fill2, false)));
            let [r, g, b, _a] = preset.ambient_rgba();
            commands.insert_resource(GlobalAmbientLight {
                color: Color::srgb_u8(r, g, b),
                brightness: AMBIENT_BRIGHTNESS,
                affects_lightmapped_meshes: false,
            });
            info!(
                path = %path,
                preset = %preset.name,
                "environment lighting bound"
            );
        }
        None => {
            report.fallback = true;
            spawn_fallback_lights(commands, owner);
        }
    }

    // `city/<stem>_fog.csv` — the authored per-preset fog table, indexed
    // by the same slot as the lighting preset (the `lvlSky` arrays it
    // fills are per `TimeWeatherType`). The fog channel is independent
    // of the lighting one: a preset that fell back still binds its row.
    let fog_path = report.fog.path.clone();
    match vfs.read_path(&fog_path) {
        Ok((bytes, _)) => match FogTable::parse(&String::from_utf8_lossy(&bytes)) {
            Ok(table) => {
                report.fog.issues = table.diagnostics.len();
                for d in &table.diagnostics {
                    warn!(path = %fog_path, diagnostic = %d, "fog table diagnostic");
                }
                let issues = table.validate();
                report.fog.issues += issues.len();
                for i in &issues {
                    warn!(path = %fog_path, issue = ?i, "fog table validation issue");
                }
                match table.row(slot) {
                    Some(row) => {
                        // Bind eligibility: the row must interpolate a
                        // finite 0..end band (validate() already counted
                        // the offender; a degenerate row binds nothing).
                        let eligible = row.color.iter().all(|c| c.is_finite())
                            && row.start.is_finite()
                            && row.end.is_finite()
                            && row.start >= 0.0
                            && row.end > row.start;
                        if eligible {
                            report.fog.bound = Some(FogSpec {
                                color: row.color.map(|c| c.clamp(0.0, 255.0)),
                                start: row.start,
                                end: row.end,
                            });
                        } else {
                            warn!(path = %fog_path, slot, "fog row is degenerate — no fog bound");
                            report.fog.absent = Some("degenerate");
                        }
                    }
                    None => {
                        warn!(path = %fog_path, slot, "fog table has no row for this slot");
                        report.fog.absent = Some("no row for slot");
                    }
                }
            }
            Err(e) => {
                warn!(path = %fog_path, error = %e, "fog table failed to parse");
                report.fog.absent = Some("unparseable");
            }
        },
        Err(e) => {
            warn!(path = %fog_path, error = %e, "fog table unavailable");
            report.fog.absent = Some("missing");
        }
    }
    report
}

// ---------------------------------------------------------------------------
// `.sky` dome (F18-A.4)
// ---------------------------------------------------------------------------

/// The dome mesh's horizontal extent — the radius its authored verts
/// reach from the origin — used to scale the ~43 m authored dome to
/// [`SKY_DOME_RADIUS`].
fn dome_extent(pkg: &Pkg) -> f32 {
    let mut extent = 0.0f32;
    for (_name, geo) in pkg.geometries() {
        for section in &geo.sections {
            for strip in &section.strips {
                for v in &strip.vertices {
                    extent = extent.max(v.position[0].hypot(v.position[2]));
                }
            }
        }
    }
    extent
}

/// Bind the session's `.sky` dome (F18-A.4): `city/<stem>.sky` names a
/// `geometry/<model>.pkg` dome mesh whose paint jobs are the same
/// `tod*4 + weather` grid the `.ltNN` preset and `_fog.csv` row bind —
/// measured on retail: `sky_dome_l`'s job *i* textures run
/// `skylondon_{c,p,f,r}{a,n,d,m}_l` in slot order, `sky_dome`'s the
/// `sky_*_f` equivalents (WLD-22). The dome spawns unlit, fog-free and
/// double-sided — the authored texture is the sky's final colour, so
/// the preset look comes from the texture selection rather than scene
/// lighting — scaled to [`SKY_DOME_RADIUS`] and re-centred on the
/// active camera each frame by [`drive_sky_dome`]. The original's
/// transform semantics are unrecovered (UNK-24): `HatYOffset` is read
/// as the dome's world-space height, `YMultiplier` as its vertical
/// squash, `RotationRate` as radians/second, and camera-centring is
/// the designed reading that keeps the authored horizon under the
/// player at any distance.
///
/// Missing/unparseable content spawns nothing and records why — never
/// a fabricated dome (F18-AC06); a missing dome *texture* warns and
/// draws the shared fallback material, matching the prop policy.
#[allow(clippy::too_many_arguments)]
pub fn spawn_sky_dome(
    commands: &mut Commands,
    vfs: &Vfs,
    psdl_path: &str,
    slot: usize,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
    owner: SessionEntity,
) -> SkyReport {
    let base = psdl_path.strip_suffix(".psdl").unwrap_or(psdl_path);
    let path = format!("{base}.sky");
    let mut report = SkyReport {
        path: path.clone(),
        ..SkyReport::default()
    };

    let def = match vfs.read_path(&path) {
        Ok((bytes, _)) => match SkyDef::parse(&String::from_utf8_lossy(&bytes)) {
            Ok(d) => Some(d),
            Err(e) => {
                warn!(path = %path, error = %e, "sky definition failed to parse");
                report.absent = Some("unparseable");
                None
            }
        },
        Err(e) => {
            warn!(path = %path, error = %e, "sky definition unavailable");
            report.absent = Some("missing");
            None
        }
    };
    let Some(def) = def else { return report };
    report.model = Some(def.model.clone());
    let issues = def.validate();
    report.issues = issues.len();
    for issue in &issues {
        warn!(path = %path, issue = ?issue, "sky definition validation issue");
    }
    // A dome with a non-finite transform field cannot be placed —
    // validate() already counted the offender.
    if !def.hat_y_offset.is_finite()
        || !def.y_multiplier.is_finite()
        || !def.rotation_rate.is_finite()
    {
        warn!(path = %path, "sky dome transform is degenerate — no dome spawned");
        report.absent = Some("degenerate");
        return report;
    }

    let pkg = match vfs
        .resolve_preferred(&format!("geometry/{}", def.model), &["pkg"])
        .and_then(|r| vfs.read(&r).ok())
        .map(|bytes| Pkg::parse(&bytes))
    {
        Some(Ok(p)) => Some(p),
        Some(Err(e)) => {
            warn!(model = %def.model, error = %e, "sky dome pkg failed to parse");
            report.absent = Some("model unparseable");
            None
        }
        None => {
            warn!(model = %def.model, "sky dome pkg unavailable");
            report.absent = Some("model unavailable");
            None
        }
    };
    let Some(pkg) = pkg else { return report };

    // The preset slot selects the paint job (the measured 16-slot
    // grid); a dome authoring fewer jobs wraps the slot, so a 4-job
    // dome in weather order still selects its weather — a designed
    // fallback, not a recovered rule.
    let jobs = pkg
        .shaders()
        .map(|s| s.paint_jobs.max(1) as usize)
        .unwrap_or(1);
    let paint = slot % jobs;
    report.paint = Some(paint);
    report.texture = pkg.shaders().and_then(|s| {
        s.shaders
            .get(paint * s.shaders_per_paint_job.max(1) as usize)
            .map(|s| s.texture.clone())
    });

    let extent = dome_extent(&pkg);
    if !extent.is_finite() || extent <= 0.0 {
        warn!(model = %def.model, "sky dome pkg has no extent — no dome spawned");
        report.absent = Some("empty");
        return report;
    }
    let scale = SKY_DOME_RADIUS / extent;

    let mut mats = crate::city::MaterialCache::new(vfs, images, materials);
    let mut missing_prims = 0usize;
    let parts = crate::city::pkg_paint_parts(&pkg, &mut mats, meshes, &mut missing_prims, paint);
    if missing_prims > 0 {
        warn!(model = %def.model, missing_prims, "sky dome strips skipped");
    }
    for stem in mats.missing_textures() {
        warn!(texture = %stem, "sky dome texture unavailable");
    }
    if parts.is_empty() {
        warn!(model = %def.model, "sky dome pkg produced no geometry");
        report.absent = Some("empty");
        return report;
    }

    commands
        .spawn((
            owner,
            SkyDome {
                angle: 0.0,
                rotation_rate: def.rotation_rate,
                y: def.hat_y_offset,
            },
            Transform::from_translation(Vec3::Y * def.hat_y_offset).with_scale(Vec3::new(
                scale,
                scale * def.y_multiplier,
                scale,
            )),
            Visibility::default(),
            Name::new(format!("sky-{}", def.model)),
        ))
        .with_children(|p| {
            for (mesh, mat) in parts {
                let material = mats.adjusted(&mat, |m| {
                    // The dome is the sky's final colour: unlit, exempt
                    // from the session's distance fog, drawn either
                    // side up, and never a shadow caster.
                    m.unlit = true;
                    m.fog_enabled = false;
                    m.cull_mode = None;
                });
                p.spawn((
                    owner,
                    Mesh3d(mesh),
                    MeshMaterial3d(material),
                    bevy::light::NotShadowCaster,
                ));
            }
        });
    info!(path = %path, model = %def.model, paint, "sky dome bound");
    report
}

/// Re-centre the dome on the active camera and advance its authored
/// rotation (F18-A.4). A frame with no active camera — or a session
/// with no dome — leaves it standing; the dome never teleports without
/// a viewer.
///
/// The focus is the camera's `GlobalTransform`: the authored cockpit
/// camera is a *child* of the vehicle, so its local `Transform` is the
/// car-space eye offset — reading it would park the dome near the
/// world origin wherever the car is (F22-B.1). The HUD map's
/// orthographic camera is not the viewer the dome follows (F22-A.1),
/// and only `Camera3d` views count — a stray menu `Camera2d` is never
/// the world view.
pub fn drive_sky_dome(
    time: Res<Time>,
    mut domes: Query<(&mut SkyDome, &mut Transform), Without<Camera>>,
    cameras: Query<(&Camera, &GlobalTransform), crate::hudmap::WorldCamera3d>,
) {
    let focus = cameras
        .iter()
        .find(|(c, _)| c.is_active)
        .map(|(_, t)| t.translation())
        .filter(|p| p.is_finite());
    for (mut dome, mut xf) in &mut domes {
        dome.angle = (dome.angle + dome.rotation_rate * time.delta_secs()) % std::f32::consts::TAU;
        if let Some(p) = focus {
            xf.translation = Vec3::new(p.x, dome.y, p.z);
        }
        xf.rotation = Quat::from_rotation_y(dome.angle);
    }
}
