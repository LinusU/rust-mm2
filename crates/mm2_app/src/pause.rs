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
//! Deferred honestly, like the root menu does: `Options` is a visible
//! disabled row (F23 owns it), and pause is reachable only from
//! `Playing` — `Countdown` still takes `Esc` as quit (the lifecycle's
//! legal-transition table has no `Countdown → Paused` edge), while
//! `Results` is owned by `crate::results` (Esc is its Continue row).

use avian3d::prelude::*;
use bevy::prelude::*;
use mm2_game::{HudMap, Session, SessionEntity, SessionPhase};

use crate::menu::{MenuCommand, MenuShell};
use crate::session::SessionControl;

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
}

/// One pause-menu row.
struct PauseRow {
    text: String,
    enabled: Result<PauseAction, String>,
}

/// The pause overlay's rows — fixed and small. `has_menu` only changes
/// the quit row's label: with a `MenuShell` running, quit lands back on
/// the menu; without one it exits the process.
fn pause_rows(has_menu: bool) -> Vec<PauseRow> {
    vec![
        PauseRow {
            text: "Resume".into(),
            enabled: Ok(PauseAction::Resume),
        },
        PauseRow {
            text: "Restart".into(),
            enabled: Ok(PauseAction::Restart),
        },
        PauseRow {
            text: "Options".into(),
            enabled: Err("not implemented yet (F23)".into()),
        },
        PauseRow {
            text: if has_menu { "Quit to menu" } else { "Quit" }.into(),
            enabled: Ok(PauseAction::Quit),
        },
    ]
}

/// The pause overlay's presentation state — focus, a status line and a
/// redraw latch. Session flow itself stays in `Session`/`SessionControl`;
/// this is never more than which row is highlighted.
#[derive(Resource)]
pub struct PauseMenu {
    /// Focused row of [`pause_rows`].
    pub focus: usize,
    /// Transient line under the rows (a disabled row's reason).
    pub status: Option<String>,
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
    menu_shell: Option<Res<MenuShell>>,
    hudmap: Option<Res<HudMap>>,
) {
    if !matches!(session.phase(), SessionPhase::Paused) {
        return;
    }
    // F22-A.1/HUD-4: while the full-screen pause map is up it replaces
    // this overlay, so the hidden rows must take no input — an unseen
    // `Enter` would activate whichever row last held focus and a drifted
    // focus could fire Restart/Quit from a screen that looks like a
    // map. `hudmap_input` (scheduled ahead) owns the map's Q/Esc close.
    if hudmap.is_some_and(|m| !m.is_stale(session.generation()) && m.fullscreen) {
        return;
    }
    let rows = pause_rows(menu_shell.is_some());
    let mut cmds = Vec::new();
    if keys.just_pressed(KeyCode::ArrowUp) || keys.just_pressed(KeyCode::KeyW) {
        cmds.push(MenuCommand::Up);
    }
    if keys.just_pressed(KeyCode::ArrowDown) || keys.just_pressed(KeyCode::KeyS) {
        cmds.push(MenuCommand::Down);
    }
    if keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::Space) {
        cmds.push(MenuCommand::Activate);
    }
    // Esc/Backspace while paused is Back — here, resume.
    if keys.just_pressed(KeyCode::Escape) || keys.just_pressed(KeyCode::Backspace) {
        cmds.push(MenuCommand::Back);
    }
    if let Some(pad) = pads.iter().next() {
        if pad.just_pressed(GamepadButton::DPadUp) {
            cmds.push(MenuCommand::Up);
        }
        if pad.just_pressed(GamepadButton::DPadDown) {
            cmds.push(MenuCommand::Down);
        }
        if pad.just_pressed(GamepadButton::South) {
            cmds.push(MenuCommand::Activate);
        }
        if pad.just_pressed(GamepadButton::East) || pad.just_pressed(GamepadButton::Start) {
            cmds.push(MenuCommand::Back);
        }
        // Left-stick nav on edge transitions, same as the menu.
        let y = pad.get(GamepadAxis::LeftStickY).unwrap_or(0.0);
        if y > 0.6 && pause.pad_axis <= 0.6 {
            cmds.push(MenuCommand::Up);
        } else if y < -0.6 && pause.pad_axis >= -0.6 {
            cmds.push(MenuCommand::Down);
        }
        pause.pad_axis = y;
    }
    for cmd in cmds {
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
                },
                Some(Err(reason)) => pause.status = Some(reason.clone()),
                None => {}
            },
            MenuCommand::Back => session
                .transition(SessionPhase::Playing)
                .expect("Paused → Playing is a legal transition"),
            // No value rows, deletable rows, text fields or
            // mouse-produced focus commands under pause.
            MenuCommand::Left
            | MenuCommand::Right
            | MenuCommand::Delete
            | MenuCommand::FocusAt(_)
            | MenuCommand::Type(_)
            | MenuCommand::Erase => {}
        }
        pause.dirty = true;
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
    menu_shell: Option<Res<MenuShell>>,
    hudmap: Option<Res<HudMap>>,
    roots: Query<Entity, (With<PauseUi>, Without<ChildOf>)>,
) {
    // F22-A.1/HUD-4: Q's full-screen map *replaces* the pause overlay —
    // while it is up the dimmed rows must not draw over the map. If the
    // map somehow closes with the session still `Paused`, `dirty`
    // redraws the rows it hid.
    let map_open = hudmap.is_some_and(|m| !m.is_stale(session.generation()) && m.fullscreen);
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
    lines.push(("Paused".to_string(), 34.0, Color::srgb(0.95, 0.9, 0.6)));
    lines.push((String::new(), 8.0, Color::NONE));
    for (i, row) in pause_rows(menu_shell.is_some()).iter().enumerate() {
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
        "Up/Down move | Enter select | Esc resume".to_string(),
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
