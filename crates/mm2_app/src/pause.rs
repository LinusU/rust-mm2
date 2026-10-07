//! Pause menu (F17-B.1): `Esc`/pad `Start` on a live session pauses
//! when the authority allows it (MP-6 — `Session::transition` rejects
//! `Playing → Paused` for host/remote authorities), an overlay offers
//! Resume/Restart/Quit, and the physics clock freezes for the duration.
//!
//! The phase machine itself already lives in `mm2_game::Session`; this
//! module is the in-session half:
//!
//! - [`pause_input`] owns every key while `Paused` — arrow/D-pad/stick
//!   focus, `Enter`/`Space`/South activate, `Esc`/Backspace/East/Start
//!   resume — except while HUD-4's full-screen pause map is up: the map
//!   *replaces* the overlay, `hudmap_input` owns its Q/Esc ahead of this
//!   system, and `pause_input` takes nothing so no key can reach the
//!   hidden rows. It is scheduled between `session_control_input` (which
//!   ignores `Paused`) and `drive_session`, so the `Esc` press that
//!   entered pause can never be re-read as a resume in the same
//!   update, and a resume press can't be re-read as a pause.
//! - [`sync_physics_pause`] mirrors `phase == Paused` onto
//!   `Time<Physics>`: Avian skips the `PhysicsSchedule` while paused,
//!   and the session tick, race clock and every input-gated system
//!   already hold still outside `Playing`. Mirroring (rather than edge
//!   hooks) covers every exit path — resume, restart and quit all
//!   leave `Paused`, so the clock unpauses whichever way out is taken.
//! - [`pause_present`] draws the overlay while `Paused`. The tree is
//!   `SessionEntity`-stamped — quitting from pause is cleaned by the
//!   normal session teardown — and carries [`PauseUi`] so
//!   `camera::retarget_hud` keeps it on the live camera.
//! - [`dev_pause_once`] is the `--pause` dev override: it pauses the
//!   first `Playing` frame so a `--frames`/`--screenshot` capture
//!   (which freezes live input) can render the overlay.
//!
//! `Options` opens the graphics page in place — the same shadow,
//! anti-aliasing and volume rows as the main menu, Left/Right to change,
//! saved at once, applied to the frozen world as it is drawn. Esc there backs out
//! to the pause rows rather than resuming. Its "Driving controls" row opens
//! the rebinding page — each key slot listens for the next key, X clears
//! one, the stick tuning rows step in place — saved and applied to the
//! live `ControlSettings` at once. The volume rows reach running loops
//! on the next mix and one-shots as they spawn.
//!
//! Deferred honestly, like the root menu does: pause is reachable only
//! from `Playing` — `Countdown` still takes `Esc` as quit (the lifecycle's
//! legal-transition table has no `Countdown → Paused` edge), while
//! `Results` is owned by `crate::results` (Esc is its Continue row).

use avian3d::prelude::*;
use bevy::prelude::*;
use mm2_game::{HudMap, Session, SessionEntity, SessionPhase};

use crate::controls::{ControlItem, ControlSettings, ControlsSave, DriveAction, SLOTS};
use crate::input::pad_nav;
use crate::menu::{MenuCommand, MenuShell};
use crate::session::SessionControl;
use crate::settings::{AudioLevel, GraphicsSettings, SettingsFile};

/// What an enabled pause row does. Disabled rows carry `Err(reason)`
/// instead — the reason renders on the row and lands on the status
/// line if activated; it never navigates to a fake screen (F17 spec
/// req 3 / AC05's no-dead-end rule, same convention as the root menu).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PauseAction {
    /// `Paused → Playing`.
    Resume,
    /// The existing restart intent: `Unloading → Menu → begin`.
    Restart,
    /// The existing quit intent: `Unloading → Menu` (menu or exit).
    Quit,
    /// Open the graphics page.
    OpenOptions,
    /// Cycle the shadow quality (Left/Right step either way).
    CycleShadows,
    /// Cycle the anti-aliasing.
    CycleAntialiasing,
    /// Cycle the on-screen text size.
    CycleTextSize,
    /// Toggle reduced flashing.
    CycleFlashing,
    /// Cycle the window mode.
    CycleDisplay,
    /// Cycle the camera field of view.
    CycleFieldOfView,
    /// Toggle vsync.
    CycleVsync,
    /// Step one of the volume levels.
    CycleAudio(AudioLevel),
    /// Put the graphics and audio settings back to their defaults.
    ResetGraphics,
    /// Leave the graphics page for the pause rows.
    CloseOptions,
    /// Open the driving-controls page.
    OpenControls,
    /// Listen for the next key and bind it to this slot.
    Rebind(DriveAction, usize),
    /// Step a controls tuning row, or reset the controls.
    Tune(ControlItem),
    /// Leave the controls page for the graphics page.
    CloseControls,
}

/// Which page of the pause overlay is showing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PausePage {
    /// Resume / Restart / Options / Quit.
    #[default]
    Pause,
    /// The graphics rows, and the way on to the controls.
    Options,
    /// Rebindable driving keys and stick tuning.
    Controls,
}

/// One pause-menu row.
struct PauseRow {
    text: String,
    enabled: Result<PauseAction, String>,
}

/// The pause overlay's rows — fixed and small. `has_menu` only changes
/// the quit row's label: with a `MenuShell` running, quit lands back on
/// the menu; without one it exits the process. `page` selects the rows;
/// `settings` is `None` in an app that has none (a bare test rig), which
/// leaves the Options row disabled with its reason, and `controls` is
/// `None` likewise for the Driving controls row.
fn pause_rows(
    has_menu: bool,
    page: PausePage,
    settings: Option<GraphicsSettings>,
    controls: Option<&ControlSettings>,
) -> Vec<PauseRow> {
    let row = |text: String, enabled: Result<PauseAction, String>| PauseRow { text, enabled };
    match page {
        PausePage::Options => {
            let settings = settings.unwrap_or_default();
            let mut rows = vec![
                row(settings.shadows_row(), Ok(PauseAction::CycleShadows)),
                row(
                    settings.antialiasing_row(),
                    Ok(PauseAction::CycleAntialiasing),
                ),
                row(settings.text_size_row(), Ok(PauseAction::CycleTextSize)),
                row(settings.flashing_row(), Ok(PauseAction::CycleFlashing)),
                row(settings.display_row(), Ok(PauseAction::CycleDisplay)),
                row(settings.vsync_row(), Ok(PauseAction::CycleVsync)),
                row(
                    settings.field_of_view_row(),
                    Ok(PauseAction::CycleFieldOfView),
                ),
            ];
            rows.extend(AudioLevel::ALL.map(|level| {
                row(
                    settings.audio.row(level),
                    Ok(PauseAction::CycleAudio(level)),
                )
            }));
            rows.extend([
                row(
                    "Reset to defaults".into(),
                    if settings == GraphicsSettings::default() {
                        Err("already at the defaults".into())
                    } else {
                        Ok(PauseAction::ResetGraphics)
                    },
                ),
                row(
                    "Driving controls".into(),
                    if controls.is_some() {
                        Ok(PauseAction::OpenControls)
                    } else {
                        Err("driving controls unavailable".into())
                    },
                ),
                row("Back".into(), Ok(PauseAction::CloseOptions)),
            ]);
            return rows;
        }
        PausePage::Controls => {
            let c = controls.cloned().unwrap_or_default();
            let mut rows = Vec::new();
            for action in DriveAction::ALL {
                for slot in 0..SLOTS {
                    let name = if slot == 0 { "" } else { " (alt)" };
                    rows.push(row(
                        format!("{}{name}: {}", action.label(), c.slot_label(action, slot)),
                        Ok(PauseAction::Rebind(action, slot)),
                    ));
                }
            }
            rows.extend(c.tuning_rows().into_iter().map(|r| {
                let action = match r.item {
                    ControlItem::Key { action, slot } => PauseAction::Rebind(action, slot),
                    item => PauseAction::Tune(item),
                };
                row(r.text, r.enabled.map(|()| action))
            }));
            rows.push(row("Back".into(), Ok(PauseAction::CloseControls)));
            return rows;
        }
        PausePage::Pause => {}
    }
    vec![
        row("Resume".into(), Ok(PauseAction::Resume)),
        row("Restart".into(), Ok(PauseAction::Restart)),
        row(
            "Options".into(),
            if settings.is_some() {
                Ok(PauseAction::OpenOptions)
            } else {
                Err("graphics settings unavailable".into())
            },
        ),
        row(
            if has_menu { "Quit to menu" } else { "Quit" }.into(),
            Ok(PauseAction::Quit),
        ),
    ]
}

/// Index of the `Options` row in the pause rows — where focus returns
/// when the graphics page closes.
const OPTIONS_ROW: usize = 2;
/// Index of the `Driving controls` row on the graphics page — where focus
/// returns when the controls page closes.
const CONTROLS_ROW: usize = 6 + AudioLevel::ALL.len() + 1;

/// The pause overlay's presentation state — focus, a status line and a
/// redraw latch. Session flow itself stays in `Session`/`SessionControl`;
/// this is never more than which row is highlighted.
#[derive(Resource)]
pub struct PauseMenu {
    /// Focused row of [`pause_rows`].
    pub focus: usize,
    /// Transient line under the rows (a disabled row's reason).
    pub status: Option<String>,
    /// Which page is showing.
    pub page: PausePage,
    /// The controls page is waiting for the key to bind to this action's
    /// slot. While set, only the next key (or Esc/pad East) is read.
    pub capture: Option<(DriveAction, usize)>,
    /// Set by `pause_input` whenever the state changed; cleared by
    /// `pause_present` after redrawing.
    dirty: bool,
    /// Last gamepad nav-axis reading — edge detection for stick moves,
    /// same latch `MenuShell::pad_axis` uses.
    pad_axis: f32,
}

impl Default for PauseMenu {
    fn default() -> Self {
        Self {
            focus: 0,
            status: None,
            page: PausePage::Pause,
            capture: None,
            dirty: true,
            pad_axis: 0.0,
        }
    }
}

/// Marker for pause-overlay entities — session-owned (the tree also
/// carries `SessionEntity`), and part of the `HudNodes` set
/// `camera::retarget_hud` keeps on the active camera.
#[derive(Component)]
pub struct PauseUi;

/// The optional resources `pause_input` consults — bundled so it stays
/// under the argument lint. All are optional: a bare test rig has none
/// of them, and the absence of settings disables the Options row (and of
/// controls, the Driving controls row).
#[derive(bevy::ecs::system::SystemParam)]
pub struct PauseGraphics<'w> {
    menu_shell: Option<Res<'w, MenuShell>>,
    hudmap: Option<Res<'w, HudMap>>,
    settings: Option<ResMut<'w, GraphicsSettings>>,
    file: Option<Res<'w, SettingsFile>>,
    controls: Option<ResMut<'w, ControlSettings>>,
    controls_save: Option<Res<'w, ControlsSave>>,
}

/// What `pause_present` reads besides the pause state itself.
#[derive(bevy::ecs::system::SystemParam)]
pub struct PauseView<'w> {
    menu_shell: Option<Res<'w, MenuShell>>,
    hudmap: Option<Res<'w, HudMap>>,
    settings: Option<Res<'w, GraphicsSettings>>,
    controls: Option<Res<'w, ControlSettings>>,
}

/// Pause-phase input. Runs only while `Paused`; every command lands on
/// the shared intents — Resume/Esc transition straight back to
/// `Playing` (`Paused → Playing` is unconditionally legal), Restart
/// and Quit set the flags `drive_session` already consumes from live
/// phases.
pub fn pause_input(
    keys: Res<ButtonInput<KeyCode>>,
    pads: Query<&Gamepad>,
    mut session: ResMut<Session>,
    mut control: ResMut<SessionControl>,
    mut pause: ResMut<PauseMenu>,
    mut graphics: PauseGraphics,
) {
    if !matches!(session.phase(), SessionPhase::Paused) {
        return;
    }
    // F22-A.1/HUD-4: while the full-screen pause map is up it replaces
    // this overlay, so the hidden rows must take no input — an unseen
    // `Enter` would activate whichever row last held focus and a drifted
    // focus could fire Restart/Quit from a screen that looks like a
    // map. `hudmap_input` (scheduled ahead) owns the map's Q/Esc close.
    if graphics
        .hudmap
        .as_deref()
        .is_some_and(|m| !m.is_stale(session.generation()) && m.fullscreen)
    {
        return;
    }
    let has_menu = graphics.menu_shell.is_some();
    let mut cmds = Vec::new();
    let nav = pad_nav(pads.iter(), &mut pause.pad_axis);
    if pause.capture.is_some() {
        // The controls page is waiting for a key: the first key pressed
        // this frame is the candidate (Esc cancels in the command loop)
        // and the pad can only back out — no nav key may fire, or the
        // key being bound would also move the focus.
        if let Some(key) = keys.get_just_pressed().min() {
            cmds.push(MenuCommand::Capture(*key));
        }
        if nav.back || nav.start {
            cmds.push(MenuCommand::Back);
        }
    } else {
        if keys.just_pressed(KeyCode::ArrowUp) || keys.just_pressed(KeyCode::KeyW) {
            cmds.push(MenuCommand::Up);
        }
        if keys.just_pressed(KeyCode::ArrowDown) || keys.just_pressed(KeyCode::KeyS) {
            cmds.push(MenuCommand::Down);
        }
        if keys.just_pressed(KeyCode::ArrowLeft) || keys.just_pressed(KeyCode::KeyA) {
            cmds.push(MenuCommand::Left);
        }
        if keys.just_pressed(KeyCode::ArrowRight) || keys.just_pressed(KeyCode::KeyD) {
            cmds.push(MenuCommand::Right);
        }
        if keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::Space) {
            cmds.push(MenuCommand::Activate);
        }
        // Esc/Backspace while paused is Back — resume, or out of a page.
        if keys.just_pressed(KeyCode::Escape) || keys.just_pressed(KeyCode::Backspace) {
            cmds.push(MenuCommand::Back);
        }
        // Only the controls page has a deletable row (a key slot).
        if keys.just_pressed(KeyCode::Delete) || keys.just_pressed(KeyCode::KeyX) {
            cmds.push(MenuCommand::Delete);
        }
        for (pressed, cmd) in [
            (nav.up, MenuCommand::Up),
            (nav.down, MenuCommand::Down),
            (nav.left, MenuCommand::Left),
            (nav.right, MenuCommand::Right),
            (nav.accept, MenuCommand::Activate),
            (nav.back || nav.start, MenuCommand::Back),
            (nav.delete, MenuCommand::Delete),
        ] {
            if pressed {
                cmds.push(cmd);
            }
        }
    }
    for cmd in cmds {
        // A pending key capture owns the page: the next key binds (or
        // cancels), Back cancels, everything else is inert.
        if let Some((action, slot)) = pause.capture {
            match cmd {
                MenuCommand::Capture(KeyCode::Escape) | MenuCommand::Back => {
                    pause.capture = None;
                    pause.status = Some("rebinding cancelled".into());
                }
                MenuCommand::Capture(key) => bind_key(action, slot, key, &mut graphics, &mut pause),
                _ => continue,
            }
            pause.dirty = true;
            continue;
        }
        // Rows are rebuilt per command: a change relabels them, resetting
        // disables the reset row, and opening or closing a page swaps the
        // whole set.
        let rows = pause_rows(
            has_menu,
            pause.page,
            graphics.settings.as_deref().copied(),
            graphics.controls.as_deref(),
        );
        match cmd {
            MenuCommand::Up => pause.focus = pause.focus.saturating_sub(1),
            MenuCommand::Down => pause.focus = (pause.focus + 1).min(rows.len().saturating_sub(1)),
            MenuCommand::Activate => match rows.get(pause.focus).map(|r| &r.enabled) {
                Some(Ok(action)) => match action {
                    PauseAction::Resume => session
                        .transition(SessionPhase::Playing)
                        .expect("Paused → Playing is a legal transition"),
                    PauseAction::Restart => control.restart = true,
                    PauseAction::Quit => control.quit = true,
                    PauseAction::OpenOptions => open_page(&mut pause, PausePage::Options, 0),
                    PauseAction::CloseOptions => {
                        open_page(&mut pause, PausePage::Pause, OPTIONS_ROW);
                    }
                    PauseAction::OpenControls => open_page(&mut pause, PausePage::Controls, 0),
                    PauseAction::CloseControls => {
                        open_page(&mut pause, PausePage::Options, CONTROLS_ROW);
                    }
                    PauseAction::Rebind(action, slot) => {
                        pause.capture = Some((*action, *slot));
                        pause.status = Some(format!(
                            "press the new key for {} (Esc cancels)",
                            action.label()
                        ));
                    }
                    PauseAction::CycleShadows
                    | PauseAction::CycleAntialiasing
                    | PauseAction::CycleTextSize
                    | PauseAction::CycleFlashing
                    | PauseAction::CycleDisplay
                    | PauseAction::CycleVsync
                    | PauseAction::CycleFieldOfView
                    | PauseAction::CycleAudio(_)
                    | PauseAction::ResetGraphics
                    | PauseAction::Tune(_) => adopt(*action, true, &mut graphics, &mut pause),
                },
                Some(Err(reason)) => pause.status = Some(reason.clone()),
                None => {}
            },
            // Left/Right step a settings value either way; everywhere
            // else they do nothing.
            MenuCommand::Left | MenuCommand::Right => {
                if let Some(Ok(action)) = rows.get(pause.focus).map(|r| &r.enabled) {
                    adopt(
                        *action,
                        cmd == MenuCommand::Right,
                        &mut graphics,
                        &mut pause,
                    );
                }
            }
            // Only a key slot is deletable; the last key stays.
            MenuCommand::Delete => {
                if let Some(Ok(PauseAction::Rebind(action, slot))) =
                    rows.get(pause.focus).map(|r| &r.enabled)
                {
                    clear_key(*action, *slot, &mut graphics, &mut pause);
                }
            }
            MenuCommand::Back => match pause.page {
                PausePage::Controls => open_page(&mut pause, PausePage::Options, CONTROLS_ROW),
                PausePage::Options => open_page(&mut pause, PausePage::Pause, OPTIONS_ROW),
                PausePage::Pause => session
                    .transition(SessionPhase::Playing)
                    .expect("Paused → Playing is a legal transition"),
            },
            // No text fields or mouse-produced focus commands under
            // pause, and a capture is handled above.
            MenuCommand::FocusAt(_)
            | MenuCommand::FocusSide(_)
            | MenuCommand::Type(_)
            | MenuCommand::Erase
            | MenuCommand::Capture(_) => {}
        }
        pause.dirty = true;
    }
}

/// Show `page` with focus on `focus` and a clean status line.
fn open_page(pause: &mut PauseMenu, page: PausePage, focus: usize) {
    pause.page = page;
    pause.focus = focus;
    pause.status = None;
    pause.capture = None;
}

/// Apply a settings row's action to the live graphics settings or driving
/// controls and save them. A change reaches the lights, cameras and
/// `vehicle_input` through the resources' change detection; a save failure
/// is a status line, the change still stands for the run. Rows of any
/// other kind do nothing.
fn adopt(action: PauseAction, forward: bool, graphics: &mut PauseGraphics, pause: &mut PauseMenu) {
    if let PauseAction::Tune(item) = action {
        let next = graphics
            .controls
            .as_deref()
            .and_then(|c| c.adjusted(item, forward));
        if let Some(next) = next {
            set_controls(next, None, graphics, pause);
        }
        return;
    }
    let Some(settings) = graphics.settings.as_mut() else {
        return;
    };
    let next = match action {
        PauseAction::CycleShadows => settings.cycled_shadows(forward),
        PauseAction::CycleAntialiasing => settings.cycled_antialiasing(forward),
        PauseAction::CycleTextSize => settings.cycled_text_size(forward),
        PauseAction::CycleFlashing => settings.toggled_reduce_flashing(),
        PauseAction::CycleDisplay => settings.cycled_display(forward),
        PauseAction::CycleVsync => settings.toggled_vsync(),
        PauseAction::CycleFieldOfView => settings.cycled_field_of_view(forward),
        PauseAction::CycleAudio(level) => settings.stepped_audio(level, forward),
        PauseAction::ResetGraphics => GraphicsSettings::default(),
        _ => return,
    };
    if next == **settings {
        return;
    }
    **settings = next;
    pause.status = graphics.file.as_deref().and_then(|f| f.save(&next).err());
}

/// Adopt `next` as the live controls and save it. `message` is the status
/// line for a success; a failed save replaces it, which matters more.
fn set_controls(
    next: ControlSettings,
    message: Option<String>,
    graphics: &mut PauseGraphics,
    pause: &mut PauseMenu,
) {
    let Some(controls) = graphics.controls.as_mut() else {
        return;
    };
    if next == **controls {
        return;
    }
    let saved = graphics
        .controls_save
        .as_deref()
        .and_then(|f| f.save(&next).err());
    **controls = next;
    pause.status = saved.or(message);
}

/// Finish a key capture: bind `key`, or keep listening with the refusal
/// on the status line.
fn bind_key(
    action: DriveAction,
    slot: usize,
    key: KeyCode,
    graphics: &mut PauseGraphics,
    pause: &mut PauseMenu,
) {
    let Some(controls) = graphics.controls.as_deref() else {
        pause.capture = None;
        return;
    };
    match controls.with_key(action, slot, key) {
        Ok((next, line)) => {
            pause.capture = None;
            pause.status = Some(line.clone());
            set_controls(next, Some(line), graphics, pause);
        }
        Err(line) => pause.status = Some(line),
    }
}

/// Clear one key slot; the last key of an action stays.
fn clear_key(
    action: DriveAction,
    slot: usize,
    graphics: &mut PauseGraphics,
    pause: &mut PauseMenu,
) {
    let Some(controls) = graphics.controls.as_deref() else {
        return;
    };
    match controls.without_key(action, slot) {
        Ok((next, line)) => set_controls(next, Some(line), graphics, pause),
        Err(line) => pause.status = Some(line),
    }
}

/// Keep `Time<Physics>` paused exactly while the session is `Paused`.
/// Avian's schedule runner skips the step when the clock is paused, so
/// the whole world holds still — not just the inputs. Mirroring the
/// phase every update means every way out of `Paused` (resume,
/// restart, quit) unpauses without a transition hook to miss.
pub fn sync_physics_pause(session: Res<Session>, time: Option<ResMut<Time<Physics>>>) {
    let Some(mut time) = time else {
        // No physics plugins (a bare menu/test app) — nothing to sync.
        return;
    };
    let want = matches!(session.phase(), SessionPhase::Paused);
    if time.is_paused() == want {
        return;
    }
    if want {
        time.pause();
    } else {
        time.unpause();
    }
}

/// `--pause` (quarantined `DevOverrides`, evidence runs only): set the
/// pause intent on the first `Playing` frame. A capture freezes live
/// input, so without this the overlay could never be rendered. It is a
/// one-shot — a session resumed or restarted afterwards stays
/// un-paused until Esc is pressed again. It carries the same MP-6
/// `allows_pause` gate as the Esc path: on a non-pausable authority it
/// never fires, so no rejected intent is queued.
pub fn dev_pause_once(
    session: Res<Session>,
    mut control: ResMut<SessionControl>,
    mut fired: Local<bool>,
) {
    if *fired {
        return;
    }
    if matches!(session.phase(), SessionPhase::Playing)
        && session
            .config()
            .is_some_and(|c| c.dev.pause && c.authority.allows_pause())
    {
        *fired = true;
        control.pause = true;
    }
}

/// (Re)draw the pause overlay while `Paused` and despawn it otherwise.
/// The root is `SessionEntity`-stamped so quit/restart teardown removes
/// it with the session even if a redraw race were possible, and the
/// whole tree carries [`PauseUi`] so `camera::retarget_hud` follows the active
/// camera (chase/free) the same way the HUD does.
pub fn pause_present(
    mut commands: Commands,
    session: Res<Session>,
    mut pause: ResMut<PauseMenu>,
    view: PauseView,
    roots: Query<Entity, (With<PauseUi>, Without<ChildOf>)>,
) {
    // F22-A.1/HUD-4: Q's full-screen map *replaces* the pause overlay —
    // while it is up the dimmed rows must not draw over the map. If the
    // map somehow closes with the session still `Paused`, `dirty`
    // redraws the rows it hid.
    let map_open = view
        .hudmap
        .is_some_and(|m| !m.is_stale(session.generation()) && m.fullscreen);
    if map_open {
        for root in &roots {
            commands.entity(root).despawn();
        }
        pause.dirty = true;
        return;
    }
    if !matches!(session.phase(), SessionPhase::Paused) {
        for root in &roots {
            commands.entity(root).despawn();
        }
        // The next pause starts at the top row with a clean status.
        pause.focus = 0;
        pause.status = None;
        pause.page = PausePage::Pause;
        pause.capture = None;
        return;
    }
    if !pause.dirty && !roots.is_empty() {
        return;
    }
    pause.dirty = false;
    for root in &roots {
        commands.entity(root).despawn();
    }

    let mut lines: Vec<(String, f32, Color)> = Vec::new();
    let title = match pause.page {
        PausePage::Pause => "Paused",
        PausePage::Options => "Graphics and audio options",
        PausePage::Controls => "Driving controls",
    };
    lines.push((title.to_string(), 34.0, Color::srgb(0.95, 0.9, 0.6)));
    lines.push((String::new(), 8.0, Color::NONE));
    for (i, row) in pause_rows(
        view.menu_shell.is_some(),
        pause.page,
        view.settings.as_deref().copied(),
        view.controls.as_deref(),
    )
    .iter()
    .enumerate()
    {
        let (text, color) = match &row.enabled {
            Ok(_) => (
                if i == pause.focus {
                    // The bundled font has no `›` glyph — ASCII markers
                    // only (same constraint as the root menu).
                    format!("> {}", row.text)
                } else {
                    format!("  {}", row.text)
                },
                if i == pause.focus {
                    Color::srgb(1.0, 1.0, 1.0)
                } else {
                    Color::srgb(0.75, 0.75, 0.8)
                },
            ),
            Err(reason) => (
                format!("  {} - {reason}", row.text),
                Color::srgb(0.45, 0.45, 0.5),
            ),
        };
        lines.push((text, 22.0, color));
    }
    lines.push((String::new(), 8.0, Color::NONE));
    if let Some(status) = &pause.status {
        lines.push((status.clone(), 18.0, Color::srgb(1.0, 0.75, 0.35)));
    }
    lines.push((
        match (pause.page, pause.capture.is_some()) {
            (PausePage::Controls, true) => "Press the new key | Esc cancels",
            (PausePage::Controls, false) => {
                "Up/Down move | Enter rebind | Left/Right change | X clear | Esc back"
            }
            (PausePage::Options, _) => "Up/Down move | Left/Right change | Esc back",
            (PausePage::Pause, _) => "Up/Down move | Enter select | Esc resume",
        }
        .to_string(),
        14.0,
        Color::srgb(0.5, 0.5, 0.55),
    ));

    commands
        .spawn((
            PauseUi,
            // Session-owned UI: teardown removes it wholesale on
            // quit/restart, and the `!Paused` arm above removes it on
            // resume — the two paths never double-despawn.
            SessionEntity(session.generation()),
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(0.0),
                left: Val::Px(0.0),
                right: Val::Px(0.0),
                bottom: Val::Px(0.0),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                row_gap: Val::Px(4.0),
                ..default()
            },
            // Dim the frozen world rather than hiding it — pause sits
            // over the session, and the HUD stays readable around the
            // panel.
            BackgroundColor(Color::srgba(0.02, 0.03, 0.07, 0.72)),
        ))
        .with_children(|parent| {
            for (text, size, color) in lines {
                parent.spawn((
                    PauseUi,
                    Text::new(text),
                    TextFont {
                        font_size: bevy::text::FontSize::Px(size),
                        ..default()
                    },
                    TextColor(color),
                ));
            }
        });
}
