//! Safe display recovery (F23 req 6, AC04's invalid-display-mode edge):
//! a change to the window mode or size is a *trial* until the person
//! confirms it. The new mode applies at once, a banner counts down, and
//! unless they keep it (Enter / Space / pad A) the previous mode and
//! size come back — Esc / pad B reverts early, and so does letting the
//! clock run out, which is the whole point: a mode the display cannot
//! show leaves nobody able to press anything.
//!
//! The watcher works on the [`GraphicsSettings`] resource, so the main
//! menu and the pause overlay are covered by one mechanism and neither
//! knows about it. While a trial is pending:
//!
//! - the menu and pause input systems are gated off
//!   ([`display_trial_pending`]), so the confirming key is never also an
//!   Options-row activation and no second change can stack on the first;
//! - the saved `settings.json` keeps the *confirmed* mode, so a crash or
//!   a force-quit during the trial reopens at the last good display
//!   rather than the one under test. Keeping rewrites the file.
//!
//! Only a change that is visible counts: a window size picked while in
//! borderless fullscreen (where the monitor sets the size) and vsync
//! are never trialled.

use std::time::Duration;

use bevy::prelude::*;

use crate::menu::{MenuData, MenuShell};
use crate::settings::{DisplayMode, GraphicsSettings, SettingsFile, WindowSize};

/// How long the person has to keep a new display mode.
pub const TRIAL_SECONDS: f32 = 15.0;

/// A display mode and window size as the settings hold them.
type Shown = (DisplayMode, WindowSize);

/// What the window will actually look like: fullscreen ignores the
/// window size, so every fullscreen pair reads the same.
fn visible((mode, size): Shown) -> Shown {
    match mode {
        DisplayMode::Windowed => (mode, size),
        DisplayMode::Fullscreen => (mode, WindowSize::default()),
    }
}

/// A trial in progress.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Pending {
    /// What to put back.
    restore: Shown,
    /// Seconds until the revert.
    remaining: f32,
}

/// The confirmed display and the trial (if any) over it.
#[derive(Resource, Default, Debug)]
pub struct DisplayTrial {
    /// The last display the person confirmed — the one the run started
    /// with until they confirm another. `None` until the first frame.
    confirmed: Option<Shown>,
    pending: Option<Pending>,
}

impl DisplayTrial {
    /// Whether a trial is waiting on the person.
    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Whole seconds left on the trial, rounded up (what the banner shows).
    pub fn seconds_left(&self) -> Option<u32> {
        self.pending.map(|p| p.remaining.max(0.0).ceil() as u32)
    }

    /// Compare `now` with the confirmed display. Returns the display to
    /// save as the file's value when this call opens a trial.
    fn observe(&mut self, now: &GraphicsSettings) -> Option<Shown> {
        let shown = (now.display, now.window_size);
        let Some(confirmed) = self.confirmed else {
            self.confirmed = Some(shown);
            return None;
        };
        if self.pending.is_some() {
            return None;
        }
        if visible(shown) == visible(confirmed) {
            // Nothing a person would see changed (a hidden size, a
            // reset that landed on the same display): follow it quietly.
            self.confirmed = Some(shown);
            return None;
        }
        self.pending = Some(Pending {
            restore: confirmed,
            remaining: TRIAL_SECONDS,
        });
        Some(confirmed)
    }

    /// Run the clock; `true` once it has expired.
    fn tick(&mut self, dt: Duration) -> bool {
        match &mut self.pending {
            Some(p) => {
                p.remaining -= dt.as_secs_f32();
                p.remaining <= 0.0
            }
            None => false,
        }
    }

    /// The person kept `now`.
    fn keep(&mut self, now: &GraphicsSettings) {
        self.pending = None;
        self.confirmed = Some((now.display, now.window_size));
    }

    /// The display to put back, ending the trial.
    fn revert(&mut self) -> Option<Shown> {
        self.pending.take().map(|p| p.restore)
    }
}

/// Run condition for the menu and pause input systems: `false` while a
/// trial owns the keyboard and the pad.
pub fn display_trial_pending(trial: Option<Res<DisplayTrial>>) -> bool {
    trial.is_some_and(|t| t.is_pending())
}

/// The banner root.
#[derive(Component)]
pub struct TrialUi;

/// The banner's countdown line.
#[derive(Component)]
pub struct TrialCountdown;

fn countdown_line(seconds: u32) -> String {
    format!("Reverting in {seconds} s")
}

fn spawn_banner(commands: &mut Commands, seconds: u32) {
    commands
        .spawn((
            TrialUi,
            GlobalZIndex(1000),
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(0.0),
                left: Val::Px(0.0),
                right: Val::Px(0.0),
                bottom: Val::Px(0.0),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                row_gap: Val::Px(6.0),
                ..default()
            },
            BackgroundColor(Color::srgba(0.02, 0.03, 0.07, 0.85)),
        ))
        .with_children(|parent| {
            let line = |text: String, size: f32, color: Color| {
                (
                    Text::new(text),
                    TextFont {
                        font_size: bevy::text::FontSize::Px(size),
                        ..default()
                    },
                    TextColor(color),
                )
            };
            parent.spawn(line(
                "Keep these display settings?".into(),
                34.0,
                Color::srgb(0.95, 0.9, 0.6),
            ));
            parent.spawn((
                TrialCountdown,
                line(countdown_line(seconds), 24.0, Color::srgb(1.0, 0.75, 0.35)),
            ));
            parent.spawn(line(
                "Enter / Space / pad A keep | Esc / pad B revert".into(),
                16.0,
                Color::srgb(0.7, 0.7, 0.75),
            ));
        });
}

/// What a trial reads: the keyboard, the pads and the settings file.
#[derive(bevy::ecs::system::SystemParam)]
pub struct TrialIo<'w, 's> {
    keys: Res<'w, ButtonInput<KeyCode>>,
    pads: Query<'w, 's, &'static Gamepad>,
    time: Res<'w, Time<Real>>,
    file: Option<Res<'w, SettingsFile>>,
    // The Options screen's own copy of the settings, when there is one.
    data: Option<ResMut<'w, MenuData>>,
    shell: Option<ResMut<'w, MenuShell>>,
}

/// Open, count down and close the trial.
pub fn drive_display_trial(
    mut commands: Commands,
    mut trial: ResMut<DisplayTrial>,
    mut settings: ResMut<GraphicsSettings>,
    mut io: TrialIo,
    banner: Query<Entity, With<TrialUi>>,
    mut countdown: Query<&mut Text, With<TrialCountdown>>,
) {
    let save = |io: &TrialIo, settings: &GraphicsSettings| {
        if let Some(file) = &io.file {
            // A save failure is logged by `SettingsFile`; the display
            // still resolves for this run.
            let _ = file.save(settings);
        }
    };
    if !trial.is_pending() {
        if let Some((mode, size)) = trial.observe(&settings) {
            // The file keeps the confirmed display until the person
            // keeps the new one (the menu has just saved the new one).
            save(
                &io,
                &GraphicsSettings {
                    display: mode,
                    window_size: size,
                    ..*settings
                },
            );
            spawn_banner(&mut commands, TRIAL_SECONDS as u32);
        }
        // The press that opened the trial is this frame's too; it is
        // read as nothing, and the next frame starts clean.
        return;
    }
    let expired = trial.tick(io.time.delta());
    let pad = |button| io.pads.iter().any(|p| p.just_pressed(button));
    let keep = io.keys.just_pressed(KeyCode::Enter)
        || io.keys.just_pressed(KeyCode::Space)
        || pad(GamepadButton::South);
    let revert = io.keys.just_pressed(KeyCode::Escape)
        || io.keys.just_pressed(KeyCode::Backspace)
        || pad(GamepadButton::East);
    if revert || (expired && !keep) {
        if let Some((mode, size)) = trial.revert() {
            settings.display = mode;
            settings.window_size = size;
        }
    } else if keep {
        trial.keep(&settings);
    } else {
        if let (Some(seconds), Ok(mut text)) = (trial.seconds_left(), countdown.single_mut()) {
            let line = countdown_line(seconds);
            if text.0 != line {
                text.0 = line;
            }
        }
        return;
    }
    // Resolved either way: close the banner, write the settled value and
    // hand the Options screen the live settings.
    for entity in &banner {
        commands.entity(entity).despawn();
    }
    save(&io, &settings);
    if let Some(data) = io.data.as_mut() {
        data.adopt_settings(*settings);
    }
    if let Some(shell) = io.shell.as_mut() {
        shell.redraw();
    }
}

/// Registers the trial and its watcher. Ordered after the menu and
/// pause input so the press that changes the display is read by them
/// first, and the press that resolves a trial is never read by them at
/// all (their gate is still shut when they run).
pub struct DisplayTrialPlugin;

impl Plugin for DisplayTrialPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DisplayTrial>().add_systems(
            Update,
            drive_display_trial
                .after(crate::menu::menu_input)
                .after(crate::pause::pause_input),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(display: DisplayMode, window_size: WindowSize) -> GraphicsSettings {
        GraphicsSettings {
            display,
            window_size,
            ..default()
        }
    }

    #[test]
    fn the_first_frame_only_records_the_display() {
        let mut t = DisplayTrial::default();
        assert_eq!(
            t.observe(&with(DisplayMode::Fullscreen, WindowSize::Hd1080)),
            None
        );
        assert!(!t.is_pending());
    }

    #[test]
    fn a_visible_change_opens_a_trial_that_remembers_the_confirmed_display() {
        let mut t = DisplayTrial::default();
        t.observe(&GraphicsSettings::default());
        let opened = t.observe(&with(DisplayMode::Fullscreen, WindowSize::Hd720));
        assert_eq!(opened, Some((DisplayMode::Windowed, WindowSize::Hd720)));
        assert_eq!(t.seconds_left(), Some(TRIAL_SECONDS as u32));
        // A further change cannot stack a second trial on the first.
        assert_eq!(
            t.observe(&with(DisplayMode::Windowed, WindowSize::Hd1080)),
            None
        );
        assert_eq!(
            t.revert(),
            Some((DisplayMode::Windowed, WindowSize::Hd720)),
            "the revert goes back to the confirmed display, not the intermediate one"
        );
    }

    #[test]
    fn a_size_hidden_by_fullscreen_and_a_reset_to_the_same_display_do_not_trial() {
        let mut t = DisplayTrial::default();
        t.observe(&with(DisplayMode::Fullscreen, WindowSize::Hd720));
        assert_eq!(
            t.observe(&with(DisplayMode::Fullscreen, WindowSize::Hd1080)),
            None
        );
        assert!(!t.is_pending());
        // And the quiet follow means a later windowed pick compares
        // against what is on screen.
        assert!(
            t.observe(&with(DisplayMode::Windowed, WindowSize::Hd1080))
                .is_some()
        );
    }

    #[test]
    fn the_clock_expires_and_keeping_confirms() {
        let mut t = DisplayTrial::default();
        t.observe(&GraphicsSettings::default());
        let next = with(DisplayMode::Windowed, WindowSize::Hd1080);
        t.observe(&next);
        assert!(!t.tick(Duration::from_secs(14)));
        assert_eq!(t.seconds_left(), Some(1));
        assert!(t.tick(Duration::from_secs(2)));
        t.keep(&next);
        assert!(!t.is_pending());
        // The kept display is now the one a revert would not undo.
        assert_eq!(t.observe(&next), None);
    }

    /// The real watcher over a real `GraphicsSettings`, with a settings
    /// file on disk, driven frame by frame.
    struct Rig {
        app: App,
        dir: std::path::PathBuf,
    }

    impl Rig {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("mm2_trial_{name}_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let path = crate::settings::settings_path(&dir);
            GraphicsSettings::default().save(&path).unwrap();
            let mut app = App::new();
            app.init_resource::<ButtonInput<KeyCode>>()
                .init_resource::<Time<Real>>()
                .init_resource::<GraphicsSettings>()
                .insert_resource(SettingsFile::new(Some(path)))
                .init_resource::<DisplayTrial>()
                .add_systems(Update, drive_display_trial);
            app.update();
            Self { app, dir }
        }

        fn frame(&mut self, seconds: f32) {
            self.app
                .world_mut()
                .resource_mut::<Time<Real>>()
                .advance_by(Duration::from_secs_f32(seconds));
            self.app.update();
            self.app
                .world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .clear();
        }

        fn press(&mut self, key: KeyCode) {
            self.app
                .world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .press(key);
        }

        fn change(&mut self, display: DisplayMode, size: WindowSize) {
            let mut s = self.app.world_mut().resource_mut::<GraphicsSettings>();
            s.display = display;
            s.window_size = size;
        }

        fn live(&self) -> GraphicsSettings {
            *self.app.world().resource::<GraphicsSettings>()
        }

        fn on_disk(&self) -> GraphicsSettings {
            GraphicsSettings::load(&crate::settings::settings_path(&self.dir))
        }

        fn banners(&mut self) -> usize {
            self.app
                .world_mut()
                .query::<&TrialUi>()
                .iter(self.app.world())
                .count()
        }
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn letting_the_clock_run_out_puts_the_old_display_back_and_into_the_file() {
        let mut rig = Rig::new("timeout");
        rig.change(DisplayMode::Fullscreen, WindowSize::Hd720);
        // The Options screen saved the new mode before the watcher saw it.
        rig.live()
            .save(&crate::settings::settings_path(&rig.dir))
            .unwrap();
        rig.frame(0.016);
        assert_eq!(rig.banners(), 1, "the trial opened its banner");
        assert_eq!(
            rig.on_disk().display,
            DisplayMode::Windowed,
            "the file keeps the confirmed display during the trial"
        );
        assert_eq!(
            rig.live().display,
            DisplayMode::Fullscreen,
            "but it applies live"
        );
        rig.frame(10.0);
        assert_eq!(
            rig.live().display,
            DisplayMode::Fullscreen,
            "still counting"
        );
        rig.frame(6.0);
        assert_eq!(rig.live().display, DisplayMode::Windowed);
        assert_eq!(rig.banners(), 0);
        assert_eq!(rig.on_disk().display, DisplayMode::Windowed);
        // And it does not re-open on the reverted value.
        rig.frame(0.016);
        assert_eq!(rig.banners(), 0);
    }

    #[test]
    fn enter_keeps_and_the_file_follows_while_escape_reverts_early() {
        let mut rig = Rig::new("keys");
        rig.change(DisplayMode::Windowed, WindowSize::Hd1080);
        rig.frame(0.016);
        assert_eq!(rig.banners(), 1);
        rig.press(KeyCode::Enter);
        rig.frame(0.016);
        assert_eq!(rig.banners(), 0);
        assert_eq!(rig.live().window_size, WindowSize::Hd1080);
        assert_eq!(rig.on_disk().window_size, WindowSize::Hd1080);

        rig.change(DisplayMode::Fullscreen, WindowSize::Hd1080);
        rig.frame(0.016);
        assert_eq!(rig.banners(), 1);
        rig.press(KeyCode::Escape);
        rig.frame(0.016);
        assert_eq!(rig.banners(), 0);
        assert_eq!(
            (rig.live().display, rig.live().window_size),
            (DisplayMode::Windowed, WindowSize::Hd1080),
            "back to the last kept display, not the shipped one"
        );
        assert_eq!(rig.on_disk().display, DisplayMode::Windowed);
    }

    #[test]
    fn the_press_that_opens_a_trial_does_not_also_keep_it() {
        let mut rig = Rig::new("opening_press");
        rig.change(DisplayMode::Fullscreen, WindowSize::Hd720);
        // Enter activated the row that changed the display this frame.
        rig.press(KeyCode::Enter);
        rig.frame(0.016);
        assert_eq!(rig.banners(), 1);
        assert!(rig.app.world().resource::<DisplayTrial>().is_pending());
    }

    #[test]
    fn the_gate_is_shut_exactly_while_a_trial_is_pending() {
        let mut rig = Rig::new("gate");
        let pending = |rig: &mut Rig| {
            rig.app
                .world_mut()
                .run_system_cached(display_trial_pending)
                .unwrap()
        };
        assert!(!pending(&mut rig));
        rig.change(DisplayMode::Fullscreen, WindowSize::Hd720);
        rig.frame(0.016);
        assert!(pending(&mut rig));
        rig.press(KeyCode::Space);
        rig.frame(0.016);
        assert!(!pending(&mut rig));
    }

    #[test]
    fn the_countdown_line_follows_the_clock() {
        let mut rig = Rig::new("banner");
        rig.change(DisplayMode::Fullscreen, WindowSize::Hd720);
        rig.frame(0.016);
        rig.frame(4.0);
        let line = rig
            .app
            .world_mut()
            .query_filtered::<&Text, With<TrialCountdown>>()
            .single(rig.app.world())
            .unwrap()
            .0
            .clone();
        assert_eq!(line, countdown_line(11));
    }
}
