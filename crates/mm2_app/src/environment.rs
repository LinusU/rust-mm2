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
//!
//! The illuminance/ambient-brightness scales are designed mappings —
//! the authored colours carry the relative weight, the constants anchor
//! the rig to the lighting level the city used before presets bound
//! (the previous fixed 15 000 lux sun). A missing or unparseable preset
//! spawns that same pre-preset rig and reports `fallback` — an explicit
//! diagnostic (F18-AC06), never a silent default.
//!
//! Deferred: `.sky` dome geometry, fog parameters (none are authored in
//! `.ltNN`), `.cpvs` PVS culling, `.lmap`/`.ldef` semantics — UNK-24.

use bevy::prelude::*;
use mm2_assets::Vfs;
use mm2_formats::lighting::{LightSpec, LightingPreset};
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
/// configured one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConditionsSource {
    /// `SessionConfig::conditions` — cruise/dev fallback (`--weather` /
    /// `--time-of-day`, or the selector-0 default).
    Configured,
    /// The running event's authored `EventParams::conditions` (RACE-2).
    Authored,
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
}

impl EnvironmentReport {
    /// The smoke record's `env=` field, e.g. `lt04(clear-noon)` or
    /// `lt07(fallback)`.
    pub fn smoke_detail(&self) -> String {
        let tag = self.name.as_deref().unwrap_or("fallback");
        format!("lt{:02}({tag})", self.slot)
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
    let mut report = EnvironmentReport {
        slot,
        path: path.clone(),
        name: None,
        source,
        fallback: false,
        issues: 0,
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
    report
}
