//! User graphics settings: the render costs a player can trade for
//! frame time (the Options screen's first slice, F23), plus the audio
//! volume levels that ride the same machine-level file ([`AudioLevels`]
//! — the struct's name predates them).
//!
//! Two graphics settings, both measured as the cost that decides whether London's
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

/// A volume bus: the group of voices one Options row scales. The
/// original's Audio Options screen separates Sound FX, Commentary,
/// Music and City Sounds (CTL-5); there is no music player here yet, so
/// there is no music bus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioBus {
    /// Engines, impacts, tyres, horns, sirens and the race cues.
    Effects,
    /// Announcer and Cops & Robbers speech.
    Commentary,
    /// The world's own sounds: weather beds, thunder and the moving
    /// objects' (drawbridge, ferry, Underground) ambience.
    City,
}

/// The user's volume levels, in whole percent so the settings stay `Eq`
/// and the file holds exact numbers. A voice plays at its authored
/// volume times `master` times its bus (`gain`); `Default` is every
/// level at 100 — the authored mix the game shipped with.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct AudioLevels {
    /// Scales every voice.
    pub master: u8,
    /// [`AudioBus::Effects`].
    pub effects: u8,
    /// [`AudioBus::Commentary`].
    pub commentary: u8,
    /// [`AudioBus::City`].
    pub city: u8,
}

impl Default for AudioLevels {
    fn default() -> Self {
        Self {
            master: MAX_LEVEL,
            effects: MAX_LEVEL,
            commentary: MAX_LEVEL,
            city: MAX_LEVEL,
        }
    }
}

/// The loudest level: the authored volume, unscaled.
pub const MAX_LEVEL: u8 = 100;
/// What one Left/Right/Enter press moves a level by.
pub const LEVEL_STEP: u8 = 10;

/// One adjustable row of [`AudioLevels`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioLevel {
    /// The master volume.
    Master,
    /// One bus's volume.
    Bus(AudioBus),
}

impl AudioLevel {
    /// Every row, in menu order.
    pub const ALL: [Self; 4] = [
        Self::Master,
        Self::Bus(AudioBus::Effects),
        Self::Bus(AudioBus::Commentary),
        Self::Bus(AudioBus::City),
    ];

    /// The menu label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Master => "Master volume",
            Self::Bus(AudioBus::Effects) => "Sound effects volume",
            Self::Bus(AudioBus::Commentary) => "Commentary volume",
            Self::Bus(AudioBus::City) => "City sounds volume",
        }
    }
}

impl AudioLevels {
    /// The level of one row.
    pub fn get(self, level: AudioLevel) -> u8 {
        match level {
            AudioLevel::Master => self.master,
            AudioLevel::Bus(AudioBus::Effects) => self.effects,
            AudioLevel::Bus(AudioBus::Commentary) => self.commentary,
            AudioLevel::Bus(AudioBus::City) => self.city,
        }
    }

    /// These levels with one row set, clamped to [`MAX_LEVEL`].
    pub fn with(mut self, level: AudioLevel, value: u8) -> Self {
        let value = value.min(MAX_LEVEL);
        match level {
            AudioLevel::Master => self.master = value,
            AudioLevel::Bus(AudioBus::Effects) => self.effects = value,
            AudioLevel::Bus(AudioBus::Commentary) => self.commentary = value,
            AudioLevel::Bus(AudioBus::City) => self.city = value,
        }
        self
    }

    /// These levels with one row stepped by [`LEVEL_STEP`], wrapping
    /// past either end like the other settings' cycles (so Enter always
    /// does something): up from 100 lands on 0, down from 0 on 100.
    pub fn stepped(self, level: AudioLevel, forward: bool) -> Self {
        let now = self.get(level);
        let next = if forward {
            if now >= MAX_LEVEL {
                0
            } else {
                now + LEVEL_STEP
            }
        } else if now == 0 {
            MAX_LEVEL
        } else {
            now.saturating_sub(LEVEL_STEP)
        };
        self.with(level, next)
    }

    /// The row's text, shared by the main menu and the pause overlay.
    pub fn row(self, level: AudioLevel) -> String {
        format!("{}: {}%", level.label(), self.get(level))
    }

    /// The linear gain a voice on `bus` plays at: `master` × the bus,
    /// 0.0 to 1.0. Always finite — the levels are clamped on load.
    pub fn gain(self, bus: AudioBus) -> f32 {
        let bus = self.get(AudioLevel::Bus(bus));
        f32::from(self.master) / f32::from(MAX_LEVEL) * (f32::from(bus) / f32::from(MAX_LEVEL))
    }

    /// Clamp every level to [`MAX_LEVEL`]; whether anything moved. A
    /// hand-edited file can hold 250, which must not amplify a voice.
    fn repair(&mut self) -> bool {
        let before = *self;
        for level in AudioLevel::ALL {
            *self = self.with(level, self.get(level));
        }
        *self != before
    }
}

/// The user's graphics choices and audio levels. `Default` is the
/// shipped look and the authored mix.
#[derive(Resource, Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(default)]
pub struct GraphicsSettings {
    /// Key-light shadows.
    pub shadows: ShadowQuality,
    /// Camera anti-aliasing.
    pub antialiasing: Antialiasing,
    /// Volume levels.
    pub audio: AudioLevels,
}

/// Step through every value of a setting, wrapping at both ends —
/// settings have no "all" entry the way the menu's Records filters do.
fn cycle_wrapping<T: PartialEq + Copy>(all: &[T], current: T, forward: bool) -> T {
    let pos = all.iter().position(|v| *v == current).unwrap_or(0);
    let next = if forward {
        (pos + 1) % all.len()
    } else {
        (pos + all.len() - 1) % all.len()
    };
    all[next]
}

impl GraphicsSettings {
    /// These settings with the shadow quality stepped.
    pub fn cycled_shadows(self, forward: bool) -> Self {
        Self {
            shadows: cycle_wrapping(&ShadowQuality::ALL, self.shadows, forward),
            ..self
        }
    }

    /// These settings with the anti-aliasing stepped.
    pub fn cycled_antialiasing(self, forward: bool) -> Self {
        Self {
            antialiasing: cycle_wrapping(&Antialiasing::ALL, self.antialiasing, forward),
            ..self
        }
    }

    /// These settings with one audio level stepped.
    pub fn stepped_audio(self, level: AudioLevel, forward: bool) -> Self {
        Self {
            audio: self.audio.stepped(level, forward),
            ..self
        }
    }

    /// The shadow row's text, shared by the main menu and the pause overlay.
    pub fn shadows_row(&self) -> String {
        format!("Shadows: {}", self.shadows.label())
    }

    /// The anti-aliasing row's text.
    pub fn antialiasing_row(&self) -> String {
        format!("Anti-aliasing: {}", self.antialiasing.label())
    }

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
        match serde_json::from_slice::<Self>(&bytes) {
            Ok(mut settings) => {
                if settings.audio.repair() {
                    warn!(path = %path.display(), "an audio level was above 100; clamped to 100");
                }
                settings
            }
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
        write_json_atomically(path, self)
    }
}

/// Write `value` as pretty JSON to `path`, creating its directory. The
/// file is written beside itself and renamed over, so a crash mid-save
/// leaves the previous file rather than a truncated one. Shared by every
/// machine-level settings file.
pub(crate) fn write_json_atomically<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let mut file = std::fs::File::create(&tmp)?;
    file.write_all(&serde_json::to_vec_pretty(value)?)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    std::fs::rename(&tmp, path)
}

/// Where the running app saves its settings — `None` keeps them for
/// this run only (an evidence run). A resource so the pause overlay can
/// save a change the way the main menu does.
#[derive(Resource, Clone, Debug, Default)]
pub struct SettingsFile(pub Option<PathBuf>);

impl SettingsFile {
    /// Save `settings`. `Err` is a line for a status bar; the settings
    /// still apply for the run.
    pub fn save(&self, settings: &GraphicsSettings) -> Result<(), String> {
        let Some(path) = &self.0 else { return Ok(()) };
        settings.save(path).map_err(|e| {
            warn!(path = %path.display(), error = %e, "graphics settings not saved");
            format!("settings not saved: {e}")
        })
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
            audio: AudioLevels {
                master: 70,
                effects: 100,
                commentary: 0,
                city: 40,
            },
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

    #[test]
    fn default_levels_are_the_authored_mix() {
        let levels = AudioLevels::default();
        for bus in [AudioBus::Effects, AudioBus::Commentary, AudioBus::City] {
            assert_eq!(levels.gain(bus), 1.0);
        }
    }

    #[test]
    fn gain_is_master_times_the_bus() {
        let levels = AudioLevels {
            master: 50,
            effects: 40,
            commentary: 0,
            city: 100,
        };
        assert!((levels.gain(AudioBus::Effects) - 0.2).abs() < 1e-6);
        assert_eq!(levels.gain(AudioBus::Commentary), 0.0);
        assert!((levels.gain(AudioBus::City) - 0.5).abs() < 1e-6);
        let silent = AudioLevels {
            master: 0,
            ..levels
        };
        assert_eq!(silent.gain(AudioBus::City), 0.0);
    }

    #[test]
    fn stepping_moves_by_ten_and_wraps_at_both_ends() {
        let level = AudioLevel::Bus(AudioBus::City);
        let mut levels = AudioLevels::default();
        levels = levels.stepped(level, false);
        assert_eq!(levels.city, 90);
        assert_eq!(levels.master, 100, "only the stepped row moves");
        for _ in 0..9 {
            levels = levels.stepped(level, false);
        }
        assert_eq!(levels.city, 0);
        assert_eq!(levels.stepped(level, false).city, 100, "down from 0 wraps");
        assert_eq!(levels.stepped(level, true).city, 10);
        assert_eq!(AudioLevels::default().stepped(level, true).city, 0);
        // A level a hand edit left off the ten-grid still steps without
        // overflow and lands back in range.
        let odd = AudioLevels::default().with(level, 5);
        assert_eq!(odd.stepped(level, false).city, 0);
        assert_eq!(odd.stepped(level, true).city, 15);
    }

    #[test]
    fn levels_round_trip_and_a_partial_file_keeps_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let path = settings_path(dir.path());
        std::fs::write(&path, br#"{"audio":{"commentary":30}}"#).unwrap();
        let loaded = GraphicsSettings::load(&path);
        assert_eq!(loaded.audio.commentary, 30);
        assert_eq!(loaded.audio.master, MAX_LEVEL);
        assert_eq!(loaded.shadows, ShadowQuality::High);
        // A settings file from before audio levels existed loads at the
        // authored mix.
        std::fs::write(&path, br#"{"shadows":"low","antialiasing":"2"}"#).unwrap();
        assert_eq!(GraphicsSettings::load(&path).audio, AudioLevels::default());
    }

    #[test]
    fn an_out_of_range_level_is_clamped_and_a_malformed_one_resets() {
        let dir = tempfile::tempdir().unwrap();
        let path = settings_path(dir.path());
        // 250 fits a `u8` but must never amplify a voice.
        std::fs::write(&path, br#"{"shadows":"low","audio":{"master":250}}"#).unwrap();
        let loaded = GraphicsSettings::load(&path);
        assert_eq!(loaded.audio.master, MAX_LEVEL);
        assert_eq!(loaded.shadows, ShadowQuality::Low, "the rest survives");
        assert!(loaded.audio.gain(AudioBus::Effects) <= 1.0);
        // Not a `u8` at all: the file is unusable, like any other bad
        // field, and the defaults apply (the next save replaces it).
        for bad in [
            &br#"{"audio":{"master":-5}}"#[..],
            br#"{"audio":{"master":999}}"#,
            br#"{"audio":{"master":"loud"}}"#,
            br#"{"audio":{"master":1e30}}"#,
        ] {
            std::fs::write(&path, bad).unwrap();
            assert_eq!(GraphicsSettings::load(&path), GraphicsSettings::default());
        }
    }

    #[test]
    fn level_rows_read_as_percent() {
        let levels = AudioLevels::default().with(AudioLevel::Master, 70);
        assert_eq!(levels.row(AudioLevel::Master), "Master volume: 70%");
        // `with` clamps, so a row can never read past 100.
        assert_eq!(levels.with(AudioLevel::Master, 200).master, MAX_LEVEL);
    }
}
