//! Results screen (F17-B.2, UI-5): after the local participant
//! resolves — `advance_race` transitions `Playing → Results` on a
//! finish or a time-out — an overlay shows what the run produced and
//! offers the two honest actions: continue (back to the menu, or exit
//! when no `MenuShell` runs) or restart the event.
//!
//! The content is read-only presentation over authoritative state —
//! the screen never mints results, applies rewards, or transitions the
//! session itself:
//!
//! - The headline is the local participant's [`ParticipantState`]:
//!   placing + total time on a finish (UI-5's documented content), or
//!   "Out of time" on expiry.
//! - The field list reads [`ResultLedger::standings_in`] — the same
//!   ordering results/progression use — and marks participants with no
//!   result yet as still racing (DSN-11: the session ends at local
//!   resolution, so an unfinished field is reported, not invented).
//! - The rewards block reads [`SessionReport`], which
//!   `record_session_results` maintains — granted unlocks, "recorded",
//!   or the honest reason nothing was kept. Nothing here decides
//!   eligibility; it only displays it.
//! - [`results_input`] owns every key while `Results`: arrow/D-pad/
//!   stick focus, `Enter`/`Space`/South activate, `Esc`/Backspace/
//!   East/Start are Continue. It is scheduled between
//!   `session_control_input` (which ignores `Results`) and
//!   `drive_session` (which consumes the quit/restart intents the rows
//!   produce — the same teardown the pause menu uses).
//! - [`dev_finish_once`] is the `--finish` dev override: it sweeps the
//!   local participant through the remaining triggers — one gate per
//!   update — so a `--frames`/`--screenshot` capture (which freezes
//!   live input, including `--bot`) can reach and render this screen.
//!   The results it produces are record-ineligible
//!   (`record_eligibility` rejects the `finish` override) and the
//!   screen says so through the report's note.

use avian3d::prelude::*;
use bevy::prelude::*;
use mm2_game::{
    CheckpointRule, ParticipantState, Player, PlayerControl, RACE_TICK_HZ, RaceDefinition,
    RaceProgress, RaceState, ResultLedger, Session, SessionEntity, SessionOutcome, SessionPhase,
    navigation_target, ordinal,
};

use crate::menu::{MenuCommand, MenuShell};
use crate::opponents::OpponentDriver;
use crate::progression::SessionReport;
use crate::session::SessionControl;

/// What a results row does when activated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResultsAction {
    /// The existing quit intent: `Unloading → Menu` (menu or exit).
    Continue,
    /// The existing restart intent: `Unloading → Menu → begin`.
    Restart,
}

/// One results-screen row.
struct ResultsRow {
    text: String,
    action: ResultsAction,
}

/// The results overlay's rows — `has_menu` only changes the continue
/// row's label, same convention as the pause menu.
fn results_rows(has_menu: bool) -> Vec<ResultsRow> {
    vec![
        ResultsRow {
            text: if has_menu { "Continue to menu" } else { "Quit" }.into(),
            action: ResultsAction::Continue,
        },
        ResultsRow {
            text: "Restart race".into(),
            action: ResultsAction::Restart,
        },
    ]
}

/// The results overlay's presentation state — focus and a redraw
/// latch. Session flow stays in `Session`/`SessionControl`.
#[derive(Resource)]
pub struct ResultsMenu {
    /// Focused row of [`results_rows`].
    pub focus: usize,
    /// Set by `results_input` whenever the state changed; cleared by
    /// `results_present` after redrawing.
    dirty: bool,
    /// Last gamepad nav-axis reading — edge detection for stick moves,
    /// same latch `MenuShell::pad_axis`/`PauseMenu` use.
    pad_axis: f32,
}

impl Default for ResultsMenu {
    fn default() -> Self {
        Self {
            focus: 0,
            dirty: true,
            pad_axis: 0.0,
        }
    }
}

/// Marker for results-overlay entities — session-owned (the tree also
/// carries `SessionEntity`), and part of the `HudNodes` set
/// `retarget_hud` keeps on the active camera.
#[derive(Component)]
pub struct ResultsUi;

/// Results-phase input. Runs only while `Results`; nav moves focus and
/// activation maps to the shared session intents — Continue and
/// `Esc`/`Back` are quit-to-menu (or exit), Restart replays the event.
pub fn results_input(
    keys: Res<ButtonInput<KeyCode>>,
    pads: Query<&Gamepad>,
    session: Res<Session>,
    mut control: ResMut<SessionControl>,
    mut results: ResMut<ResultsMenu>,
    menu_shell: Option<Res<MenuShell>>,
) {
    if !matches!(session.phase(), SessionPhase::Results) {
        return;
    }
    let rows = results_rows(menu_shell.is_some());
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
    // Esc/Backspace at results is Back — here, continue.
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
        let y = pad.get(GamepadAxis::LeftStickY).unwrap_or(0.0);
        if y > 0.6 && results.pad_axis <= 0.6 {
            cmds.push(MenuCommand::Up);
        } else if y < -0.6 && results.pad_axis >= -0.6 {
            cmds.push(MenuCommand::Down);
        }
        results.pad_axis = y;
    }
    for cmd in cmds {
        match cmd {
            MenuCommand::Up => results.focus = results.focus.saturating_sub(1),
            MenuCommand::Down => {
                results.focus = (results.focus + 1).min(rows.len().saturating_sub(1))
            }
            MenuCommand::Activate => match rows.get(results.focus).map(|r| r.action) {
                Some(ResultsAction::Continue) => control.quit = true,
                Some(ResultsAction::Restart) => control.restart = true,
                None => {}
            },
            MenuCommand::Back => control.quit = true,
            // No value rows, deletable rows, text fields or
            // mouse-produced focus commands at results.
            MenuCommand::Left
            | MenuCommand::Right
            | MenuCommand::Delete
            | MenuCommand::FocusAt(_)
            | MenuCommand::Type(_)
            | MenuCommand::Erase => {}
        }
        results.dirty = true;
    }
}

/// `--finish` (quarantined `DevOverrides`, evidence runs only): each
/// update, move the local participant into its next open trigger —
/// `checkpoints[next]` under `Ordered`, the navigation objective under
/// `AnyOrder`. The swept segment from the previous position still
/// crosses the trigger the real `advance_race` checks, so the finish,
/// ledger entry and standings all come from the live path — nothing
/// here mints a result. A capture freezes live input (and `--bot`),
/// so without this a `--frames`/`--screenshot` run can never reach
/// `Results`. The override is gameplay-affecting, so
/// `record_eligibility` rejects it and the report says the run kept
/// nothing.
pub fn dev_finish_once(
    session: Res<Session>,
    race: Option<Res<RaceState>>,
    mut participants: Query<(&Player, &RaceProgress, &mut Position, &mut Transform)>,
) {
    let Some(race) = race else { return };
    if !session.is_playing()
        || race.is_stale(session.generation())
        || !session.config().is_some_and(|c| c.dev.finish)
    {
        return;
    }
    let Some((_, progress, mut pos, mut transform)) = participants
        .iter_mut()
        .find(|(p, _, _, _)| p.control == PlayerControl::Local)
    else {
        return;
    };
    if progress.state != ParticipantState::Racing {
        return;
    }
    let Some(target) = next_open_trigger(&race.definition, progress, pos.0) else {
        return;
    };
    pos.0 = target;
    transform.translation = target;
}

/// The trigger a `Racing` participant must cross next — the target the
/// navigation arrow would point at (the finish once every gate is
/// cleared). `Ordered` races have no arrow target (HUD-2), so the next
/// authored gate stands in.
fn next_open_trigger(def: &RaceDefinition, progress: &RaceProgress, pos: Vec3) -> Option<Vec3> {
    match def.rule {
        CheckpointRule::Ordered => def.checkpoints.get(progress.next).map(|c| c.center),
        CheckpointRule::AnyOrder => {
            navigation_target(def, progress, None, pos).and_then(|t| t.position(def))
        }
    }
}

/// How one participant is named in the standings list — the authored
/// opponent's vehicle id, `You` for the local driver, a stable id for
/// anything else (remote names are F24+).
fn participant_name(player: &Player, opponent: Option<&OpponentDriver>) -> String {
    match player.control {
        PlayerControl::Local => "You".to_string(),
        _ => opponent
            .map(|o| o.spec.vehicle.clone())
            .unwrap_or_else(|| format!("driver {}", player.id.0)),
    }
}

/// (Re)draw the results overlay while `Results` and despawn it
/// otherwise. The root is `SessionEntity`-stamped so teardown removes
/// it with the session, and the whole tree carries [`ResultsUi`] so
/// `retarget_hud` follows the active camera like the HUD and pause
/// overlay do.
#[allow(clippy::too_many_arguments)]
pub fn results_present(
    mut commands: Commands,
    session: Res<Session>,
    mut results: ResMut<ResultsMenu>,
    menu_shell: Option<Res<MenuShell>>,
    ledger: Option<Res<ResultLedger>>,
    report: Option<Res<SessionReport>>,
    participants: Query<(&Player, &RaceProgress, Option<&OpponentDriver>)>,
    roots: Query<Entity, (With<ResultsUi>, Without<ChildOf>)>,
) {
    if !matches!(session.phase(), SessionPhase::Results) {
        for root in &roots {
            commands.entity(root).despawn();
        }
        // The next resolution starts at the top row.
        results.focus = 0;
        return;
    }
    // Redraw on entry, on input, and when the report lands — the
    // report is written after the ledger entry exists, so the rewards
    // block can arrive a frame late.
    let report_changed = report.as_ref().is_some_and(|r| r.is_changed());
    if !results.dirty && !report_changed && !roots.is_empty() {
        return;
    }
    results.dirty = false;
    for root in &roots {
        commands.entity(root).despawn();
    }

    let generation = session.generation();
    let field: Vec<(&Player, &RaceProgress, Option<&OpponentDriver>)> =
        participants.iter().collect();
    let n_field = field.len();
    let name_of = |id: mm2_game::PlayerId| -> String {
        field
            .iter()
            .find(|(p, _, _)| p.id == id)
            .map(|(p, _, o)| participant_name(p, *o))
            .unwrap_or_else(|| format!("driver {}", id.0))
    };
    let secs = |ticks: u64| ticks as f32 / RACE_TICK_HZ as f32;

    let mut lines: Vec<(String, f32, Color)> = Vec::new();
    lines.push((
        "Race results".to_string(),
        34.0,
        Color::srgb(0.95, 0.9, 0.6),
    ));

    // The local outcome: placing + total time on a finish (UI-5), or
    // the expiry on a time-out. A session with no local participant or
    // no resolved state should not reach `Results`; the fallback keeps
    // the screen honest if one does.
    let local = field
        .iter()
        .find(|(p, _, _)| p.control == PlayerControl::Local);
    let headline = match local.map(|(_, pr, _)| &pr.state) {
        Some(ParticipantState::Finished { race_ticks, .. }) => {
            let place = local
                .and_then(|(p, _, _)| {
                    ledger
                        .as_ref()
                        .and_then(|l| l.place_of_in(generation, p.id))
                })
                .map(|p| match n_field {
                    n if n > 1 => format!("{} of {n}", ordinal(p)),
                    _ => ordinal(p),
                });
            match place {
                Some(p) => format!("{p} - {:.1}s", secs(*race_ticks)),
                None => format!("Finished - {:.1}s", secs(*race_ticks)),
            }
        }
        Some(ParticipantState::TimedOut { .. }) => "Out of time".to_string(),
        _ => "Race over".to_string(),
    };
    lines.push((headline, 24.0, Color::srgb(1.0, 1.0, 1.0)));
    lines.push((String::new(), 8.0, Color::NONE));

    // The field, in ledger order — resolved entries with their
    // outcome, unresolved ones honestly still racing (the session ends
    // at the local resolution, DSN-11).
    if let Some(ledger) = ledger.as_ref() {
        let standings = ledger.standings_in(generation);
        for (i, result) in standings.iter().enumerate() {
            let outcome = match result.outcome {
                SessionOutcome::Finished { race_ticks } => format!("{:.1}s", secs(race_ticks)),
                SessionOutcome::TimedOut { .. } => "out of time".to_string(),
            };
            lines.push((
                format!("{}. {} - {outcome}", i + 1, name_of(result.id.participant)),
                20.0,
                Color::srgb(0.85, 0.85, 0.9),
            ));
        }
        let mut pending: Vec<&Player> = field
            .iter()
            .map(|(p, _, _)| *p)
            .filter(|p| !standings.iter().any(|r| r.id.participant == p.id))
            .collect();
        pending.sort_by_key(|p| p.id);
        for p in pending {
            lines.push((
                format!("   {} - still racing", name_of(p.id)),
                20.0,
                Color::srgb(0.6, 0.6, 0.65),
            ));
        }
    }

    // Rewards — the real disposition, not a guess: grants, "recorded",
    // or the reason nothing was kept.
    if let Some(report) = report.as_ref().filter(|r| r.generation == generation) {
        if !report.granted.is_empty() || report.recorded || report.note.is_some() {
            lines.push((String::new(), 8.0, Color::NONE));
        }
        for grant in &report.granted {
            lines.push((
                format!("Unlocked: {grant}"),
                20.0,
                Color::srgb(0.55, 0.9, 0.5),
            ));
        }
        if report.recorded {
            lines.push((
                "Result saved to the driver profile".to_string(),
                18.0,
                Color::srgb(0.55, 0.8, 0.9),
            ));
        }
        if let Some(note) = &report.note {
            lines.push((note.clone(), 18.0, Color::srgb(1.0, 0.75, 0.35)));
        }
    }

    lines.push((String::new(), 8.0, Color::NONE));
    for (i, row) in results_rows(menu_shell.is_some()).iter().enumerate() {
        let (text, color) = if i == results.focus {
            // The bundled font has no `›` glyph — ASCII markers only
            // (same constraint as the root menu and pause overlay).
            (format!("> {}", row.text), Color::srgb(1.0, 1.0, 1.0))
        } else {
            (format!("  {}", row.text), Color::srgb(0.75, 0.75, 0.8))
        };
        lines.push((text, 22.0, color));
    }
    lines.push((String::new(), 8.0, Color::NONE));
    lines.push((
        "Up/Down move | Enter select | Esc continue".to_string(),
        14.0,
        Color::srgb(0.5, 0.5, 0.55),
    ));

    commands
        .spawn((
            ResultsUi,
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
            // Dim the world behind like the pause overlay — the
            // session is over but still visible, and the HUD stays
            // readable around the panel.
            BackgroundColor(Color::srgba(0.02, 0.03, 0.07, 0.72)),
        ))
        .with_children(|parent| {
            for (text, size, color) in lines {
                parent.spawn((
                    ResultsUi,
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
