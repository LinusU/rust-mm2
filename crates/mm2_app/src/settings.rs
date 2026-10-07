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
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

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

/// How large the on-screen text and HUD draw — the accessibility
/// setting for a player who cannot read the stock layout (F23 req 4).
/// It is Bevy's [`UiScale`], so every UI node (menus, HUD, results,
/// pause) grows together; the 3D world is untouched (a node tied to a
/// camera viewport, like the minimap bezel, must divide it back out).
/// Capped at 150%:
/// the authored HUD anchors to the window's edges and a larger step
/// pushes its fixed-size elements off a 1280x720 window.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TextSize {
    /// The layout the game shipped with.
    #[default]
    Normal,
    /// One quarter larger.
    Large,
    /// One half larger.
    Larger,
}

impl TextSize {
    /// Every value, in menu order.
    pub const ALL: [Self; 3] = [Self::Normal, Self::Large, Self::Larger];

    /// The menu label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "100%",
            Self::Large => "125%",
            Self::Larger => "150%",
        }
    }

    /// The factor [`UiScale`] carries.
    pub fn factor(self) -> f32 {
        match self {
            Self::Normal => 1.0,
            Self::Large => 1.25,
            Self::Larger => 1.5,
        }
    }
}

/// How the game window sits on the display. Borderless fullscreen is the
/// only fullscreen offered: it never changes the display's video mode,
/// so a bad choice cannot leave the player with an unusable screen (the
/// transactional-recovery leg of F23 req 6 only matters for mode
/// switches, which this deliberately does not make).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DisplayMode {
    /// A normal window at the startup size.
    #[default]
    Windowed,
    /// A borderless window covering the current monitor.
    Fullscreen,
}

impl DisplayMode {
    /// Every value, in menu order.
    pub const ALL: [Self; 2] = [Self::Windowed, Self::Fullscreen];

    /// The menu label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Windowed => "Windowed",
            Self::Fullscreen => "Borderless fullscreen",
        }
    }

    /// The Bevy window mode this value asks for.
    pub fn window_mode(self) -> bevy::window::WindowMode {
        match self {
            Self::Windowed => bevy::window::WindowMode::Windowed,
            Self::Fullscreen => bevy::window::WindowMode::BorderlessFullscreen(
                bevy::window::MonitorSelection::Current,
            ),
        }
    }
}

/// `vsync` defaults on: serde fills a field an older file lacks from
/// this, not from `bool::default`.
fn vsync_on() -> bool {
    true
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
}

/// The user's graphics choices and audio levels. `Default` is the
/// shipped look and the authored mix.
#[derive(Resource, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct GraphicsSettings {
    /// Key-light shadows.
    pub shadows: ShadowQuality,
    /// Camera anti-aliasing.
    pub antialiasing: Antialiasing,
    /// Volume levels.
    pub audio: AudioLevels,
    /// Size of the on-screen text and HUD.
    pub text_size: TextSize,
    /// Photosensitivity option: lights that alternate or pulse hold
    /// steady instead (the cop light bar, the low-time warning).
    pub reduce_flashing: bool,
    /// Window or borderless fullscreen.
    pub display: DisplayMode,
    /// Wait for the display's refresh when presenting a frame.
    #[serde(default = "vsync_on")]
    pub vsync: bool,
}

impl Default for GraphicsSettings {
    fn default() -> Self {
        Self {
            shadows: ShadowQuality::default(),
            antialiasing: Antialiasing::default(),
            audio: AudioLevels::default(),
            text_size: TextSize::default(),
            reduce_flashing: false,
            display: DisplayMode::default(),
            vsync: true,
        }
    }
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

    /// These settings with the text size stepped.
    pub fn cycled_text_size(self, forward: bool) -> Self {
        Self {
            text_size: cycle_wrapping(&TextSize::ALL, self.text_size, forward),
            ..self
        }
    }

    /// These settings with reduced flashing flipped. Both step
    /// directions flip it — a two-valued row has no ends to wrap past.
    pub fn toggled_reduce_flashing(self) -> Self {
        Self {
            reduce_flashing: !self.reduce_flashing,
            ..self
        }
    }

    /// These settings with the display mode stepped.
    pub fn cycled_display(self, forward: bool) -> Self {
        Self {
            display: cycle_wrapping(&DisplayMode::ALL, self.display, forward),
            ..self
        }
    }

    /// These settings with vsync flipped (either step direction flips
    /// it).
    pub fn toggled_vsync(self) -> Self {
        Self {
            vsync: !self.vsync,
            ..self
        }
    }

    /// The Bevy present mode vsync asks for: `AutoVsync` falls back
    /// across the vsync modes the surface supports, `AutoNoVsync` picks
    /// the lowest-latency mode that does not wait for the display.
    pub fn present_mode(&self) -> bevy::window::PresentMode {
        if self.vsync {
            bevy::window::PresentMode::AutoVsync
        } else {
            bevy::window::PresentMode::AutoNoVsync
        }
    }

    /// The display row's text.
    pub fn display_row(&self) -> String {
        format!("Display: {}", self.display.label())
    }

    /// The vsync row's text.
    pub fn vsync_row(&self) -> String {
        format!("VSync: {}", if self.vsync { "On" } else { "Off" })
    }

    /// The flashing row's text.
    pub fn flashing_row(&self) -> String {
        let value = if self.reduce_flashing {
            "Reduced"
        } else {
            "Normal"
        };
        format!("Flashing: {value}")
    }

    /// The text-size row's text.
    pub fn text_size_row(&self) -> String {
        format!("Text size: {}", self.text_size.label())
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
    /// yields the defaults silently. An invalid piece of an otherwise
    /// good file warns and falls back on its own — one hand-edited typo
    /// must not throw away the person's other choices — and a file that
    /// is not a JSON object at all warns, is set aside as
    /// `settings.json.bad` (so the next save does not destroy it) and
    /// yields the defaults. A broken settings file never keeps the game
    /// from starting.
    pub fn load(path: &Path) -> Self {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                warn!(path = %path.display(), error = %e, "graphics settings unreadable; using the defaults");
                return Self::default();
            }
        };
        match Self::from_json(&bytes) {
            Ok((settings, issues)) => {
                for issue in issues {
                    warn!(path = %path.display(), "settings: {issue}");
                }
                settings
            }
            Err(e) => {
                set_aside(path, &e, "graphics settings");
                Self::default()
            }
        }
    }

    /// Parse settings JSON field by field: a field that is missing keeps
    /// its default silently, one that is present but invalid keeps its
    /// default and is named in the returned issues, and an out-of-range
    /// audio level clamps. Only a document that is not a JSON object is
    /// an error.
    fn from_json(bytes: &[u8]) -> Result<(Self, Vec<String>), String> {
        let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        let serde_json::Value::Object(map) = value else {
            return Err("the document is not a JSON object".into());
        };
        let mut issues = Vec::new();
        let d = Self::default();
        let out = Self {
            shadows: pick(&map, "shadows", d.shadows, &mut issues),
            antialiasing: pick(&map, "antialiasing", d.antialiasing, &mut issues),
            audio: pick_audio(&map, &mut issues),
            text_size: pick(&map, "text_size", d.text_size, &mut issues),
            reduce_flashing: pick(&map, "reduce_flashing", d.reduce_flashing, &mut issues),
            display: pick(&map, "display", d.display, &mut issues),
            vsync: pick(&map, "vsync", d.vsync, &mut issues),
        };
        Ok((out, issues))
    }

    /// Write the settings file, creating its directory. The file is
    /// written beside itself and renamed over, so a crash mid-save
    /// leaves the previous settings rather than a truncated file.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        write_json_atomically(path, self)
    }
}

/// Move an unparseable settings file to `<name>.bad` so the next save
/// does not destroy what the person had; best effort, always warns.
pub(crate) fn set_aside(path: &Path, error: &str, what: &str) {
    let bad = path.with_extension("json.bad");
    match std::fs::rename(path, &bad) {
        Ok(()) => {
            warn!(path = %path.display(), %error, kept = %bad.display(), "{what} unparseable; using the defaults");
        }
        Err(rename) => {
            warn!(path = %path.display(), %error, %rename, "{what} unparseable and could not be set aside; using the defaults");
        }
    }
}

/// One settings field out of a parsed document: absent keeps `default`;
/// present but invalid keeps it too and records why.
fn pick<T: serde::de::DeserializeOwned>(
    map: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    default: T,
    issues: &mut Vec<String>,
) -> T {
    match map.get(key) {
        None => default,
        Some(v) => serde_json::from_value(v.clone()).unwrap_or_else(|e| {
            issues.push(format!("{key} is invalid ({e}); using the default"));
            default
        }),
    }
}

/// The audio levels one by one: a whole number above [`MAX_LEVEL`]
/// clamps (a hand-edited 250 must not amplify a voice), anything that is
/// not a non-negative whole number keeps that level's default.
fn pick_audio(
    map: &serde_json::Map<String, serde_json::Value>,
    issues: &mut Vec<String>,
) -> AudioLevels {
    let mut levels = AudioLevels::default();
    let Some(audio) = map.get("audio") else {
        return levels;
    };
    let Some(audio) = audio.as_object() else {
        issues.push("audio is not an object; using the default levels".into());
        return levels;
    };
    for (key, level) in [
        ("master", AudioLevel::Master),
        ("effects", AudioLevel::Bus(AudioBus::Effects)),
        ("commentary", AudioLevel::Bus(AudioBus::Commentary)),
        ("city", AudioLevel::Bus(AudioBus::City)),
    ] {
        let Some(v) = audio.get(key) else { continue };
        match v.as_u64() {
            Some(n) if n > u64::from(MAX_LEVEL) => {
                issues.push(format!("audio {key} is {n}; clamped to {MAX_LEVEL}"));
                levels = levels.with(level, MAX_LEVEL);
            }
            Some(n) => levels = levels.with(level, n as u8),
            None => issues.push(format!(
                "audio {key} is not a whole number from 0 to {MAX_LEVEL}; using the default"
            )),
        }
    }
    levels
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

/// What a run's command-line flags (`--shadows`, `--msaa`, `--no-vsync`)
/// changed against the settings file. Those flags are measurement
/// switches for this run, so a later save from the menu must not write
/// them into the file: a field still holding its overridden value is
/// saved as the file had it. Once the person changes a field the flag no
/// longer applies to it — even if they later step back to the flag's
/// value, that is now their choice. Clones share that memory, so the
/// main menu and the pause overlay agree.
#[derive(Clone, Debug, Default)]
pub struct RunOverrides {
    /// The settings as the file had them.
    disk: GraphicsSettings,
    /// The settings the run started with, flags applied.
    effective: GraphicsSettings,
    /// Fields the person has changed since: one `FIELD_*` bit each.
    chosen: Arc<AtomicU8>,
}

const FIELD_SHADOWS: u8 = 1;
const FIELD_ANTIALIASING: u8 = 2;
const FIELD_VSYNC: u8 = 4;

impl RunOverrides {
    /// The overrides that turn `disk` into `effective`.
    pub fn between(disk: GraphicsSettings, effective: GraphicsSettings) -> Self {
        Self {
            disk,
            effective,
            chosen: Arc::default(),
        }
    }

    /// `now` as it should be written to the settings file.
    pub fn persisted(&self, now: GraphicsSettings) -> GraphicsSettings {
        let (disk, effective) = (&self.disk, &self.effective);
        let mut moved = 0;
        for (bit, changed) in [
            (FIELD_SHADOWS, now.shadows != effective.shadows),
            (
                FIELD_ANTIALIASING,
                now.antialiasing != effective.antialiasing,
            ),
            (FIELD_VSYNC, now.vsync != effective.vsync),
        ] {
            if changed {
                moved |= bit;
            }
        }
        let chosen = self.chosen.fetch_or(moved, Ordering::Relaxed) | moved;
        let mut out = now;
        if chosen & FIELD_SHADOWS == 0 {
            out.shadows = disk.shadows;
        }
        if chosen & FIELD_ANTIALIASING == 0 {
            out.antialiasing = disk.antialiasing;
        }
        if chosen & FIELD_VSYNC == 0 {
            out.vsync = disk.vsync;
        }
        out
    }
}

/// Where the running app saves its settings — `None` keeps them for
/// this run only (an evidence run). A resource so the pause overlay can
/// save a change the way the main menu does.
#[derive(Resource, Clone, Debug, Default)]
pub struct SettingsFile {
    path: Option<PathBuf>,
    overrides: RunOverrides,
}

impl SettingsFile {
    /// Save to `path` (`None`: this run only) with no flag overrides.
    pub fn new(path: Option<PathBuf>) -> Self {
        Self {
            path,
            overrides: RunOverrides::default(),
        }
    }

    /// Keep this run's command-line overrides out of the saved file.
    pub fn with_overrides(mut self, overrides: RunOverrides) -> Self {
        self.overrides = overrides;
        self
    }

    /// Save `settings`. `Err` is a line for a status bar; the settings
    /// still apply for the run.
    pub fn save(&self, settings: &GraphicsSettings) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        self.overrides.persisted(*settings).save(path).map_err(|e| {
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

/// Push [`GraphicsSettings::text_size`] onto Bevy's [`UiScale`] when the
/// setting changes (the resource starts at 1.0, which is `Normal`).
pub fn apply_text_size(settings: Res<GraphicsSettings>, mut scale: ResMut<UiScale>) {
    if !settings.is_changed() {
        return;
    }
    let factor = settings.text_size.factor();
    if scale.0 != factor {
        scale.0 = factor;
    }
}

/// Push [`GraphicsSettings::display`] and [`GraphicsSettings::vsync`]
/// onto the primary window when the settings change. Each field is
/// written only when it differs, so a frame that changes something else
/// (the shadows row) never touches the window; the startup window is
/// built from the same settings, so the first run writes nothing.
pub fn apply_display_settings(
    settings: Res<GraphicsSettings>,
    mut windows: Query<&mut Window, With<bevy::window::PrimaryWindow>>,
) {
    if !settings.is_changed() {
        return;
    }
    let mode = settings.display.window_mode();
    let present = settings.present_mode();
    for mut window in &mut windows {
        if window.mode != mode {
            window.mode = mode;
        }
        if window.present_mode != present {
            window.present_mode = present;
        }
    }
}

/// Registers the settings resource (at its defaults unless the app
/// inserted one first) and the systems that apply it.
pub struct GraphicsSettingsPlugin;

impl Plugin for GraphicsSettingsPlugin {
    fn build(&self, app: &mut App) {
        // `UiPlugin` initialises `UiScale` too; a bare test rig has none.
        app.init_resource::<GraphicsSettings>()
            .init_resource::<UiScale>()
            .add_systems(
                Update,
                (
                    apply_shadow_settings,
                    apply_antialiasing,
                    apply_text_size,
                    apply_display_settings,
                ),
            );
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
    fn a_flag_override_is_not_saved_until_the_person_changes_that_field() {
        let disk = GraphicsSettings {
            shadows: ShadowQuality::Low,
            ..default()
        };
        let run = GraphicsSettings {
            vsync: false,
            antialiasing: Antialiasing::Off,
            ..disk
        };
        let overrides = RunOverrides::between(disk, run);
        let shared = overrides.clone();

        // Another setting changes: the flags stay out of the file.
        let other = GraphicsSettings {
            text_size: TextSize::Larger,
            ..run
        };
        assert_eq!(
            overrides.persisted(other),
            GraphicsSettings {
                text_size: TextSize::Larger,
                ..disk
            }
        );
        // The person turns VSync on, then back off: both are their choice,
        // and the clone (the pause overlay's copy) agrees.
        let on = GraphicsSettings {
            vsync: true,
            ..other
        };
        assert!(overrides.persisted(on).vsync);
        assert!(!shared.persisted(other).vsync);
        assert_eq!(
            shared.persisted(other).antialiasing,
            Antialiasing::X4,
            "the untouched --msaa flag still stays out"
        );
        // No flags at all is the identity.
        let plain = RunOverrides::between(disk, disk);
        assert_eq!(plain.persisted(other), other);
        assert_eq!(RunOverrides::default().persisted(other), other);
    }

    #[test]
    fn the_settings_file_saves_without_the_run_flags() {
        let dir = tempfile::tempdir().unwrap();
        let path = settings_path(dir.path());
        let disk = GraphicsSettings::default();
        let run = GraphicsSettings {
            vsync: false,
            ..disk
        };
        let file =
            SettingsFile::new(Some(path.clone())).with_overrides(RunOverrides::between(disk, run));
        let next = GraphicsSettings {
            reduce_flashing: true,
            ..run
        };
        file.save(&next).unwrap();
        let saved = GraphicsSettings::load(&path);
        assert!(saved.reduce_flashing && saved.vsync);
        // No path: nothing is written and nothing fails.
        assert!(SettingsFile::new(None).save(&next).is_ok());
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
            text_size: TextSize::Larger,
            reduce_flashing: true,
            display: DisplayMode::Fullscreen,
            vsync: false,
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
    fn an_invalid_field_falls_back_alone_and_the_rest_survives() {
        let dir = tempfile::tempdir().unwrap();
        let path = settings_path(dir.path());
        std::fs::write(
            &path,
            br#"{"shadows":"ultra","antialiasing":"x2","text_size":"huge","reduce_flashing":"yes",
                "display":"exclusive","vsync":false,"audio":{"master":40,"effects":"loud","commentary":-3,"city":250}}"#,
        )
        .unwrap();
        let s = GraphicsSettings::load(&path);
        let d = GraphicsSettings::default();
        assert_eq!(s.shadows, d.shadows, "unknown shadow quality");
        assert_eq!(s.antialiasing, Antialiasing::X2, "valid field kept");
        assert_eq!(s.text_size, d.text_size);
        assert_eq!(s.reduce_flashing, d.reduce_flashing);
        assert_eq!(s.display, d.display, "an unknown display mode is windowed");
        assert!(!s.vsync, "valid field kept");
        assert_eq!(s.audio.master, 40, "valid level kept");
        assert_eq!(s.audio.effects, MAX_LEVEL, "text level uses its default");
        assert_eq!(
            s.audio.commentary, MAX_LEVEL,
            "negative level uses its default"
        );
        assert_eq!(s.audio.city, MAX_LEVEL, "250 clamps");
        assert!(path.exists(), "a readable object is not moved aside");
    }

    #[test]
    fn an_unparseable_file_is_set_aside_not_destroyed_by_the_next_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = settings_path(dir.path());
        for (i, broken) in [&b"{ not json"[..], b"[1,2]", b"", b"\"shadows\""]
            .into_iter()
            .enumerate()
        {
            std::fs::write(&path, broken).unwrap();
            assert_eq!(GraphicsSettings::load(&path), GraphicsSettings::default());
            assert!(!path.exists(), "case {i}: the broken file moved away");
            let kept = path.with_extension("json.bad");
            assert_eq!(
                std::fs::read(&kept).unwrap(),
                broken,
                "case {i}: kept intact"
            );
            // The next save writes a fresh file and leaves the copy alone.
            GraphicsSettings::default().save(&path).unwrap();
            assert_eq!(std::fs::read(&kept).unwrap(), broken);
            assert_eq!(GraphicsSettings::load(&path), GraphicsSettings::default());
        }
    }

    #[test]
    fn a_partial_file_keeps_the_defaults_for_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let path = settings_path(dir.path());
        std::fs::write(&path, br#"{"shadows":"off"}"#).unwrap();
        let s = GraphicsSettings::load(&path);
        assert_eq!(s.shadows, ShadowQuality::Off);
        assert_eq!(s.antialiasing, Antialiasing::X4);
        assert_eq!(
            s.text_size,
            TextSize::Normal,
            "a file from before the setting existed"
        );
    }

    #[test]
    fn reduced_flashing_toggles_both_ways_and_an_old_file_keeps_it_off() {
        let d = GraphicsSettings::default();
        assert!(!d.reduce_flashing, "the shipped look flashes");
        assert_eq!(d.flashing_row(), "Flashing: Normal");
        let on = d.toggled_reduce_flashing();
        assert!(on.reduce_flashing);
        assert_eq!(on.flashing_row(), "Flashing: Reduced");
        assert_eq!(on.toggled_reduce_flashing(), d);
        // Only the flag moved.
        assert_eq!(
            GraphicsSettings {
                reduce_flashing: false,
                ..on
            },
            d
        );
        // A file from before the option existed loads with it off.
        let dir = tempfile::tempdir().unwrap();
        let path = settings_path(dir.path());
        std::fs::write(&path, br#"{"text_size":"large"}"#).unwrap();
        let loaded = GraphicsSettings::load(&path);
        assert!(!loaded.reduce_flashing);
        assert_eq!(loaded.text_size, TextSize::Large);
        // A value that is not a bool resets the file like any bad field.
        std::fs::write(&path, br#"{"reduce_flashing":"yes"}"#).unwrap();
        assert_eq!(GraphicsSettings::load(&path), d);
    }

    #[test]
    fn an_unknown_text_size_is_recovered_not_trusted() {
        let dir = tempfile::tempdir().unwrap();
        let path = settings_path(dir.path());
        std::fs::write(&path, br#"{"text_size":"huge"}"#).unwrap();
        assert_eq!(GraphicsSettings::load(&path), GraphicsSettings::default());
    }

    #[test]
    fn text_size_steps_wrap_and_never_exceed_the_cap() {
        let d = GraphicsSettings::default();
        assert_eq!(d.text_size.factor(), 1.0);
        let up = d.cycled_text_size(true);
        assert_eq!(up.text_size, TextSize::Large);
        assert_eq!(up.cycled_text_size(true).text_size, TextSize::Larger);
        assert_eq!(up.cycled_text_size(true).cycled_text_size(true), d);
        assert_eq!(d.cycled_text_size(false).text_size, TextSize::Larger);
        for size in TextSize::ALL {
            assert!((1.0..=1.5).contains(&size.factor()));
        }
        assert_eq!(up.text_size_row(), "Text size: 125%");
    }

    #[test]
    fn the_text_size_setting_moves_the_ui_scale() {
        let mut app = App::new();
        app.add_plugins(GraphicsSettingsPlugin);
        app.update();
        assert_eq!(app.world().resource::<UiScale>().0, 1.0);
        app.world_mut().resource_mut::<GraphicsSettings>().text_size = TextSize::Larger;
        app.update();
        assert_eq!(app.world().resource::<UiScale>().0, 1.5);
        app.world_mut().resource_mut::<GraphicsSettings>().text_size = TextSize::Normal;
        app.update();
        assert_eq!(app.world().resource::<UiScale>().0, 1.0);
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
    fn an_out_of_range_level_is_clamped_and_a_malformed_one_keeps_its_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = settings_path(dir.path());
        // 250 fits a `u8` but must never amplify a voice.
        std::fs::write(&path, br#"{"shadows":"low","audio":{"master":250}}"#).unwrap();
        let loaded = GraphicsSettings::load(&path);
        assert_eq!(loaded.audio.master, MAX_LEVEL);
        assert_eq!(loaded.shadows, ShadowQuality::Low, "the rest survives");
        assert!(loaded.audio.gain(AudioBus::Effects) <= 1.0);
        // Not a whole percent: that one level keeps its default and the
        // others (and the other settings) survive.
        for bad in [
            &br#"{"shadows":"low","audio":{"master":-5,"city":30}}"#[..],
            br#"{"shadows":"low","audio":{"master":"loud","city":30}}"#,
            br#"{"shadows":"low","audio":{"master":1e30,"city":30}}"#,
            br#"{"shadows":"low","audio":{"master":2.5,"city":30}}"#,
        ] {
            std::fs::write(&path, bad).unwrap();
            let loaded = GraphicsSettings::load(&path);
            assert_eq!(loaded.audio.master, MAX_LEVEL);
            assert_eq!(loaded.audio.city, 30);
            assert_eq!(loaded.shadows, ShadowQuality::Low);
        }
        // `audio` itself the wrong shape: all four levels default.
        std::fs::write(&path, br#"{"shadows":"low","audio":[50]}"#).unwrap();
        let loaded = GraphicsSettings::load(&path);
        assert_eq!(loaded.audio, AudioLevels::default());
        assert_eq!(loaded.shadows, ShadowQuality::Low);
    }

    #[test]
    fn level_rows_read_as_percent() {
        let levels = AudioLevels::default().with(AudioLevel::Master, 70);
        assert_eq!(levels.row(AudioLevel::Master), "Master volume: 70%");
        // `with` clamps, so a row can never read past 100.
        assert_eq!(levels.with(AudioLevel::Master, 200).master, MAX_LEVEL);
    }
    #[test]
    fn display_and_vsync_step_both_ways_and_an_old_file_keeps_the_shipped_window() {
        let d = GraphicsSettings::default();
        assert_eq!(d.display, DisplayMode::Windowed);
        assert!(d.vsync, "the shipped look waits for the display");
        assert_eq!(d.display_row(), "Display: Windowed");
        assert_eq!(d.vsync_row(), "VSync: On");
        let full = d.cycled_display(true);
        assert_eq!(full.display_row(), "Display: Borderless fullscreen");
        assert_eq!(full.cycled_display(true), d, "forward wraps");
        assert_eq!(d.cycled_display(false), full, "backward wraps");
        let off = d.toggled_vsync();
        assert_eq!(off.vsync_row(), "VSync: Off");
        assert_eq!(off.present_mode(), bevy::window::PresentMode::AutoNoVsync);
        assert_eq!(d.present_mode(), bevy::window::PresentMode::AutoVsync);
        assert_eq!(off.toggled_vsync(), d);

        // A file from before these fields existed still vsyncs, and one
        // that says so keeps it; a value that is not a bool or a known
        // mode is a bad field, which resets the file like any other.
        let dir = tempfile::tempdir().unwrap();
        let path = settings_path(dir.path());
        std::fs::write(&path, br#"{"shadows":"off"}"#).unwrap();
        let loaded = GraphicsSettings::load(&path);
        assert!(loaded.vsync);
        assert_eq!(loaded.display, DisplayMode::Windowed);
        std::fs::write(&path, br#"{"vsync":false,"display":"fullscreen"}"#).unwrap();
        let loaded = GraphicsSettings::load(&path);
        assert!(!loaded.vsync);
        assert_eq!(loaded.display, DisplayMode::Fullscreen);
        for bad in [
            &br#"{"display":"exclusive"}"#[..],
            &br#"{"display":3}"#[..],
            &br#"{"vsync":"yes"}"#[..],
        ] {
            std::fs::write(&path, bad).unwrap();
            assert_eq!(GraphicsSettings::load(&path), d);
        }
    }

    #[test]
    fn the_window_follows_the_display_settings_and_only_when_they_change() {
        use bevy::window::{MonitorSelection, PresentMode, PrimaryWindow, WindowMode};
        let mut app = App::new();
        app.add_plugins(GraphicsSettingsPlugin);
        let window = app
            .world_mut()
            .spawn((Window::default(), PrimaryWindow))
            .id();
        // A second, non-primary window (a tool window) is left alone.
        let other = app.world_mut().spawn(Window::default()).id();
        app.update();
        let w = app.world().get::<Window>(window).unwrap();
        assert_eq!(w.mode, WindowMode::Windowed);
        assert_eq!(w.present_mode, PresentMode::AutoVsync);

        let chosen = GraphicsSettings {
            display: DisplayMode::Fullscreen,
            vsync: false,
            ..default()
        };
        *app.world_mut().resource_mut::<GraphicsSettings>() = chosen;
        app.update();
        let w = app.world().get::<Window>(window).unwrap();
        assert_eq!(
            w.mode,
            WindowMode::BorderlessFullscreen(MonitorSelection::Current)
        );
        assert_eq!(w.present_mode, PresentMode::AutoNoVsync);
        let o = app.world().get::<Window>(other).unwrap();
        assert_eq!(o.mode, WindowMode::Windowed);
        assert_eq!(o.present_mode, Window::default().present_mode);

        // A frame where the settings did not change leaves a window the
        // platform moved (a fullscreen exit) alone.
        app.world_mut().get_mut::<Window>(window).unwrap().mode = WindowMode::Windowed;
        app.update();
        assert_eq!(
            app.world().get::<Window>(window).unwrap().mode,
            WindowMode::Windowed
        );
        // Back to windowed once the setting says so.
        *app.world_mut().resource_mut::<GraphicsSettings>() = GraphicsSettings::default();
        app.update();
        let w = app.world().get::<Window>(window).unwrap();
        assert_eq!(w.mode, WindowMode::Windowed);
        assert_eq!(w.present_mode, PresentMode::AutoVsync);
    }
}
