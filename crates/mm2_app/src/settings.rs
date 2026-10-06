//! User graphics settings: the render costs a player can trade for
//! frame time (the Options screen's first slice, F23).
//!
//! Two settings, both measured as the cost that decides whether London's
//! "Tower Tour" holds 60 Hz on an M1 at Retina resolution (README,
//! "Frame-time profiling"):
//!
//! - [`ShadowQuality`] — whether the key light casts shadows and how
//!   many cascades it renders. Every cascade draws the visible city
//!   again, on the CPU (culling, queuing) and the GPU.
//! - [`Antialiasing`] — the MSAA sample count on every 3D camera.
//!
//! The defaults are the look the game shipped with before the settings
//! existed — shadows on the engine's default four cascades, 4x MSAA — so
//! nobody's picture changes until they ask for it to. The settings are
//! machine-level, not driver-level: they persist to `settings.json` in
//! the profile store's root, beside the `driver-*` files the store scans
//! (it ignores everything else), whichever driver is bound.
//!
//! [`GraphicsSettings`] is a resource; [`GraphicsSettingsPlugin`]'s
//! systems push it onto the world — onto every [`KeyLight`] and every
//! `Camera3d`, the frame after they spawn and whenever the resource
//! changes — so a session never has to know the settings exist.

use std::io::Write;
use std::path::{Path, PathBuf};

use bevy::light::{CascadeShadowConfig, CascadeShadowConfigBuilder};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use tracing::warn;

/// File name inside the settings directory.
pub const SETTINGS_FILE: &str = "settings.json";

/// The settings file inside `root` (the profile store's root).
pub fn settings_path(root: &Path) -> PathBuf {
    root.join(SETTINGS_FILE)
}

/// How the key light casts shadows.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum ShadowQuality {
    /// No shadows: the cheapest, and flat-lit.
    Off,
    /// Two cascades over the nearer part of the view.
    Low,
    /// The engine's default four cascades out to 150 m — how the game
    /// has always looked.
    #[default]
    High,
}

impl ShadowQuality {
    /// Every value, in menu order.
    pub const ALL: [Self; 3] = [Self::Off, Self::Low, Self::High];

    /// The menu label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Low => "Low",
            Self::High => "High",
        }
    }

    /// The cascade layout this quality renders, `None` when it casts no
    /// shadows. `High` is exactly what an untouched `DirectionalLight`
    /// carries, so choosing it restores the shipped look.
    fn cascades(self) -> Option<CascadeShadowConfig> {
        match self {
            Self::Off => None,
            Self::Low => Some(
                CascadeShadowConfigBuilder {
                    num_cascades: LOW_CASCADES,
                    maximum_distance: LOW_DISTANCE,
                    first_cascade_far_bound: LOW_FIRST_BOUND,
                    ..default()
                }
                .build(),
            ),
            Self::High => Some(CascadeShadowConfig::default()),
        }
    }
}

/// `Low` shadows: two cascades, and a shorter reach with the first
/// cascade's edge pushed out to match, so what is near the car stays
/// sharp enough while the far cascades the city rarely fills go.
const LOW_CASCADES: usize = 2;
const LOW_DISTANCE: f32 = 80.0;
const LOW_FIRST_BOUND: f32 = 20.0;

/// Multisample anti-aliasing on the 3D cameras.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Antialiasing {
    /// No anti-aliasing: the cheapest, with visibly stepped edges.
    #[value(name = "off")]
    Off,
    /// Two samples per pixel.
    #[value(name = "2")]
    X2,
    /// Four samples per pixel — the engine's default.
    #[default]
    #[value(name = "4")]
    X4,
}

impl Antialiasing {
    /// Every value, in menu order.
    pub const ALL: [Self; 3] = [Self::Off, Self::X2, Self::X4];

    /// The menu label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::X2 => "2x MSAA",
            Self::X4 => "4x MSAA",
        }
    }

    fn msaa(self) -> Msaa {
        match self {
            Self::Off => Msaa::Off,
            Self::X2 => Msaa::Sample2,
            Self::X4 => Msaa::Sample4,
        }
    }
}

/// The user's graphics choices. `Default` is the shipped look.
#[derive(Resource, Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(default)]
pub struct GraphicsSettings {
    /// Key-light shadows.
    pub shadows: ShadowQuality,
    /// Camera anti-aliasing.
    pub antialiasing: Antialiasing,
}

impl GraphicsSettings {
    /// Read the settings file. A missing file is the first run and
    /// yields the defaults silently; an unreadable or unparseable one
    /// warns and yields the defaults too — a broken settings file must
    /// never keep the game from starting, and the next save replaces it.
    pub fn load(path: &Path) -> Self {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                warn!(path = %path.display(), error = %e, "graphics settings unreadable; using the defaults");
                return Self::default();
            }
        };
        match serde_json::from_slice(&bytes) {
            Ok(settings) => settings,
            Err(e) => {
                warn!(path = %path.display(), error = %e, "graphics settings unparseable; using the defaults");
                Self::default()
            }
        }
    }

    /// Write the settings file, creating its directory. The file is
    /// written beside itself and renamed over, so a crash mid-save
    /// leaves the previous settings rather than a truncated file.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(&serde_json::to_vec_pretty(self)?)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)
    }
}

/// Marks the one directional light that casts shadows (the preset's
/// key light, or the fallback rig's only light) — the fills never do,
/// and must not start when the settings turn shadows on.
#[derive(Component)]
pub struct KeyLight;

/// Push [`GraphicsSettings::shadows`] onto every [`KeyLight`]: when the
/// setting changes, and for a key light on the frame after it spawns.
pub fn apply_shadow_settings(
    settings: Res<GraphicsSettings>,
    spawned: Query<(), Added<KeyLight>>,
    mut lights: Query<(Entity, &mut DirectionalLight), With<KeyLight>>,
    mut commands: Commands,
) {
    if !settings.is_changed() && spawned.is_empty() {
        return;
    }
    let cascades = settings.shadows.cascades();
    for (entity, mut light) in &mut lights {
        light.shadow_maps_enabled = cascades.is_some();
        if let Some(cascades) = cascades.clone() {
            commands.entity(entity).insert(cascades);
        }
    }
}

/// Push [`GraphicsSettings::antialiasing`] onto every `Camera3d`: when
/// the setting changes, and for cameras on the frame after they spawn.
/// Every 3D camera gets the same count — the map, mirror and dash
/// cameras share the window's target with the world camera, and mixed
/// sample counts on one target are not worth the saving.
pub fn apply_antialiasing(
    settings: Res<GraphicsSettings>,
    spawned: Query<(), Added<Camera3d>>,
    cameras: Query<(Entity, Option<&Msaa>), With<Camera3d>>,
    mut commands: Commands,
) {
    if !settings.is_changed() && spawned.is_empty() {
        return;
    }
    let msaa = settings.antialiasing.msaa();
    for (entity, current) in &cameras {
        if current != Some(&msaa) {
            commands.entity(entity).insert(msaa);
        }
    }
}

/// Registers the settings resource (at its defaults unless the app
/// inserted one first) and the systems that apply it.
pub struct GraphicsSettingsPlugin;

impl Plugin for GraphicsSettingsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GraphicsSettings>()
            .add_systems(Update, (apply_shadow_settings, apply_antialiasing));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_the_shipped_look() {
        let d = GraphicsSettings::default();
        assert_eq!(d.shadows, ShadowQuality::High);
        assert_eq!(d.antialiasing, Antialiasing::X4);
        // `High` must be what an untouched light carries, and 4x is
        // Bevy's own default sample count.
        assert_eq!(Msaa::default(), d.antialiasing.msaa());
        let high = ShadowQuality::High.cascades().unwrap();
        let untouched = CascadeShadowConfig::default();
        assert_eq!(high.bounds, untouched.bounds);
        assert_eq!(high.overlap_proportion, untouched.overlap_proportion);
        assert_eq!(high.minimum_distance, untouched.minimum_distance);
    }

    #[test]
    fn low_shadows_render_fewer_cascades_than_high() {
        let low = ShadowQuality::Low.cascades().unwrap();
        let high = ShadowQuality::High.cascades().unwrap();
        assert_eq!(low.bounds.len(), LOW_CASCADES);
        assert!(low.bounds.len() < high.bounds.len());
        assert!(low.bounds.last() < high.bounds.last());
        assert!(ShadowQuality::Off.cascades().is_none());
    }

    #[test]
    fn settings_round_trip_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = settings_path(&dir.path().join("nested"));
        let chosen = GraphicsSettings {
            shadows: ShadowQuality::Low,
            antialiasing: Antialiasing::Off,
        };
        chosen.save(&path).unwrap();
        assert_eq!(GraphicsSettings::load(&path), chosen);
        assert!(
            !path.with_extension("json.tmp").exists(),
            "the temp file is renamed away"
        );
    }

    #[test]
    fn a_missing_or_broken_file_loads_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = settings_path(dir.path());
        assert_eq!(GraphicsSettings::load(&path), GraphicsSettings::default());
        std::fs::write(&path, b"{ not json").unwrap();
        assert_eq!(GraphicsSettings::load(&path), GraphicsSettings::default());
        std::fs::write(&path, br#"{"shadows":"ultra"}"#).unwrap();
        assert_eq!(GraphicsSettings::load(&path), GraphicsSettings::default());
    }

    #[test]
    fn a_partial_file_keeps_the_defaults_for_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let path = settings_path(dir.path());
        std::fs::write(&path, br#"{"shadows":"off"}"#).unwrap();
        let s = GraphicsSettings::load(&path);
        assert_eq!(s.shadows, ShadowQuality::Off);
        assert_eq!(s.antialiasing, Antialiasing::X4);
    }
}
