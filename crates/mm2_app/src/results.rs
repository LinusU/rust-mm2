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
//!   result yet as still racing. They are: the field races on behind
//!   the screen (DSN-11), and the list redraws as each late result
//!   lands, so an unfinished field is reported, never invented.
//! - A Crash Course lesson (a `LessonDriver` in the world) swaps the
//!   headline and field for the sequencer's verdict: "Lesson passed"
//!   with the summed clear time, or "Lesson failed" naming the leg and
//!   why, with each leg's time (the failed one marked). There is no
//!   placing; the restart row reads "Retry lesson" (the same intent).
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
    CheckpointRule, LegFailure, LessonPhase, ParticipantState, Player, PlayerControl, RACE_TICK_HZ,
    RaceDefinition, RaceProgress, RaceState, ResultLedger, Session, SessionAuthority,
    SessionEntity, SessionOutcome, SessionPhase, navigation_target, ordinal,
};

use crate::cnr::{CnrHost, participant_id};
use crate::cnrhud::{match_view, result_lines};
use crate::cnrnet::CnrReplica;
use crate::input::pad_nav;
use crate::lesson::LessonDriver;
use crate::menu::{MenuCommand, MenuShell};
use crate::netdrive::NetPlayer;
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

/// What this results screen is for: a race (the default), a Cops &
/// Robbers match this process owns, or one it joined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResultsKind {
    Race,
    /// A `Local` match — *Play again* is the session's own restart.
    CnrLocal,
    /// A hosted or joined match: the lobby's `Cancel`/`Start` mint the
    /// next generation, so the only way on is back to the lobby.
    CnrNetworked,
    /// A Crash Course lesson (F21-B): a pass or fail over its legs, not
    /// a placing in a field. The restart is the lesson's own retry.
    Lesson,
}

impl ResultsKind {
    /// A race screen is a lesson screen while a `LessonDriver` runs;
    /// a match screen stays one.
    fn or_lesson(self, lesson: bool) -> Self {
        if lesson && self == Self::Race {
            Self::Lesson
        } else {
            self
        }
    }

    fn of(session: &Session, host: Option<&CnrHost>, replica: Option<&CnrReplica>) -> Self {
        if host.is_none() && replica.is_none() {
            return Self::Race;
        }
        if session
            .config()
            .is_some_and(|c| c.authority == SessionAuthority::Local)
        {
            Self::CnrLocal
        } else {
            Self::CnrNetworked
        }
    }
}

/// The results overlay's rows — `has_menu` only changes the continue
/// row's label, same convention as the pause menu; `kind` names the
/// restart row for what it replays (a networked match has none).
fn results_rows(has_menu: bool, kind: ResultsKind) -> Vec<ResultsRow> {
    if kind == ResultsKind::CnrNetworked {
        return vec![ResultsRow {
            text: "Back to lobby".into(),
            action: ResultsAction::Continue,
        }];
    }
    vec![
        ResultsRow {
            text: if has_menu { "Continue to menu" } else { "Quit" }.into(),
            action: ResultsAction::Continue,
        },
        ResultsRow {
            text: match kind {
                ResultsKind::CnrLocal => "Play again",
                ResultsKind::Lesson => "Retry lesson",
                _ => "Restart race",
            }
            .into(),
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
/// `camera::retarget_hud` keeps on the active camera.
#[derive(Component)]
pub struct ResultsUi;

/// Results-phase input. Runs only while `Results`; nav moves focus and
/// activation maps to the shared session intents — Continue and
/// `Esc`/`Back` are quit-to-menu (or exit), Restart replays the event.
// Pad, keys, session intents, the menu shell and the two match
// sources are each a distinct input; a bundle would only hide them.
#[allow(clippy::too_many_arguments)]
pub fn results_input(
    keys: Res<ButtonInput<KeyCode>>,
    pads: Query<&Gamepad>,
    session: Res<Session>,
    mut control: ResMut<SessionControl>,
    mut results: ResMut<ResultsMenu>,
    menu_shell: Option<Res<MenuShell>>,
    cnr: Option<Res<CnrHost>>,
    replica: Option<Res<CnrReplica>>,
    lesson: Option<Res<LessonDriver>>,
) {
    if !matches!(session.phase(), SessionPhase::Results) {
        return;
    }
    let kind =
        ResultsKind::of(&session, cnr.as_deref(), replica.as_deref()).or_lesson(lesson.is_some());
    let rows = results_rows(menu_shell.is_some(), kind);
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
    let nav = pad_nav(pads.iter(), &mut results.pad_axis);
    for (pressed, cmd) in [
        (nav.up, MenuCommand::Up),
        (nav.down, MenuCommand::Down),
        (nav.accept, MenuCommand::Activate),
        (nav.back || nav.start, MenuCommand::Back),
    ] {
        if pressed {
            cmds.push(cmd);
        }
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
            | MenuCommand::FocusSide(_)
            | MenuCommand::Type(_)
            | MenuCommand::Erase
            | MenuCommand::Capture(_)
            | MenuCommand::CapturePad(_) => {}
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
    let f = target.forward();
    let normal = Vec3::new(f.x, 0.0, f.y);
    let side = (pos.0 - target.center).dot(normal);
    // Stage off the plane if parked on it; the following update sweeps across.
    let destination = target.center + normal * if side < -1e-4 { 2.0 } else { -2.0 };
    pos.0 = destination;
    transform.translation = destination;
}

/// The trigger a `Racing` participant must cross next — the target the
/// navigation arrow would point at (the finish once every gate is
/// cleared). `Ordered` races have no arrow target (HUD-2), so the next
/// authored gate stands in.
fn next_open_trigger<'a>(
    def: &'a RaceDefinition,
    progress: &RaceProgress,
    pos: Vec3,
) -> Option<&'a mm2_game::Checkpoint> {
    match def.rule {
        CheckpointRule::Ordered => def.checkpoints.get(progress.next),
        CheckpointRule::AnyOrder => {
            navigation_target(def, progress, None, pos).and_then(|t| match t {
                mm2_game::NavTarget::Gate(i) => def.checkpoints.get(i),
                mm2_game::NavTarget::Finish => def.finish.as_ref(),
            })
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

/// The rewards block — the real disposition, not a guess: grants,
/// "recorded", or the reason nothing was kept. Only the report of this
/// session's generation counts.
fn reward_lines(
    report: Option<&SessionReport>,
    generation: u64,
    lines: &mut Vec<(String, f32, Color)>,
) {
    let Some(report) = report.filter(|r| r.generation == generation) else {
        return;
    };
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

/// A lesson's verdict and its legs (F21-B): passed with the summed
/// clear time, or failed naming the leg and why; each cleared leg with
/// its time and the failed one marked. Read from the sequencer, which
/// is the only thing that decides a lesson — there is no placing, no
/// field and no "race" here.
fn lesson_lines(driver: &LessonDriver, lines: &mut Vec<(String, f32, Color)>) {
    let run = driver.run();
    let secs = |ticks: u64| ticks as f32 / RACE_TICK_HZ as f32;
    let legs = run.leg_count();
    lines.push((
        "Crash Course".to_string(),
        34.0,
        Color::srgb(0.95, 0.9, 0.6),
    ));
    let (headline, color) = match run.phase() {
        LessonPhase::Passed => {
            let total = driver
                .pass()
                .map_or_else(|| run.cleared().iter().sum(), |pass| pass.total_ticks());
            (
                format!("Lesson passed - {:.1}s", secs(total)),
                Color::srgb(0.55, 0.9, 0.5),
            )
        }
        LessonPhase::Failed { leg, failure } => {
            let why = match failure {
                LegFailure::TimedOut => "out of time",
                LegFailure::Disabled => "car disabled",
                LegFailure::Objective => "objective missed",
            };
            (
                format!("Lesson failed - {why} on leg {} of {legs}", leg + 1),
                Color::srgb(1.0, 0.75, 0.35),
            )
        }
        LessonPhase::Running { .. } | LessonPhase::Abandoned => {
            ("Lesson over".to_string(), Color::srgb(1.0, 1.0, 1.0))
        }
    };
    lines.push((headline, 24.0, color));
    lines.push((String::new(), 8.0, Color::NONE));
    let name_of = |leg: usize| {
        driver
            .leg(leg)
            .map_or_else(|| format!("leg {}", leg + 1), |l| l.filename.clone())
    };
    for (i, ticks) in run.cleared().iter().enumerate() {
        lines.push((
            format!("{}. {} - {:.1}s", i + 1, name_of(i), secs(*ticks)),
            20.0,
            Color::srgb(0.85, 0.85, 0.9),
        ));
    }
    if let LessonPhase::Failed { leg, .. } = run.phase() {
        lines.push((
            format!("{}. {} - failed", leg + 1, name_of(leg as usize)),
            20.0,
            Color::srgb(1.0, 0.75, 0.35),
        ));
    }
}

/// (Re)draw the results overlay while `Results` and despawn it
/// otherwise. The root is `SessionEntity`-stamped so teardown removes
/// it with the session, and the whole tree carries [`ResultsUi`] so
/// `camera::retarget_hud` follows the active camera like the HUD and pause
/// overlay do.
#[allow(clippy::too_many_arguments)]
pub fn results_present(
    mut commands: Commands,
    session: Res<Session>,
    mut results: ResMut<ResultsMenu>,
    menu_shell: Option<Res<MenuShell>>,
    ledger: Option<Res<ResultLedger>>,
    report: Option<Res<SessionReport>>,
    cnr: Option<Res<CnrHost>>,
    replica: Option<Res<CnrReplica>>,
    lesson: Option<Res<LessonDriver>>,
    cars: Query<(&Player, Option<&NetPlayer>)>,
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
    // Redraw on entry, on input, when the report lands — the report
    // is written after the ledger entry exists, so the rewards block
    // can arrive a frame late — and when an opponent still racing
    // behind the screen records its result (DSN-11).
    let report_changed = report.as_ref().is_some_and(|r| r.is_changed());
    let ledger_changed = ledger.as_ref().is_some_and(|l| l.is_changed());
    if !results.dirty && !report_changed && !ledger_changed && !roots.is_empty() {
        return;
    }
    results.dirty = false;
    for root in &roots {
        commands.entity(root).despawn();
    }

    let kind =
        ResultsKind::of(&session, cnr.as_deref(), replica.as_deref()).or_lesson(lesson.is_some());
    let mut lines: Vec<(String, f32, Color)> = Vec::new();
    if let (ResultsKind::Lesson, Some(driver)) = (kind, lesson.as_deref()) {
        lesson_lines(driver, &mut lines);
        reward_lines(report.as_deref(), session.generation(), &mut lines);
    } else if matches!(kind, ResultsKind::CnrLocal | ResultsKind::CnrNetworked) {
        // A Cops & Robbers match: its own verdict and standings, read
        // from the decided match (the authority's, or a client's
        // replica of it). No rewards block — nothing
        // here is recorded to the profile (the mode has no authored
        // progression).
        let me = cars
            .iter()
            .find(|(p, _)| p.control == PlayerControl::Local)
            .map(|(p, net)| participant_id(p, net));
        lines.push((
            "Cops & Robbers".to_string(),
            34.0,
            Color::srgb(0.95, 0.9, 0.6),
        ));
        let body = match_view(cnr.as_deref(), replica.as_deref())
            .and_then(|view| result_lines(&view, me, RACE_TICK_HZ))
            .unwrap_or_default();
        for (i, text) in body.into_iter().enumerate() {
            let (size, color) = if i == 0 {
                (24.0, Color::srgb(1.0, 1.0, 1.0))
            } else {
                (20.0, Color::srgb(0.85, 0.85, 0.9))
            };
            lines.push((text, size, color));
        }
    } else {
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
        // outcome, unresolved ones still racing behind the screen (DSN-11)
        // until their own result lands.
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

        reward_lines(report.as_deref(), generation, &mut lines);
    }
    lines.push((String::new(), 8.0, Color::NONE));
    for (i, row) in results_rows(menu_shell.is_some(), kind).iter().enumerate() {
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

#[cfg(test)]
mod cnr_tests {
    use super::*;
    use mm2_game::SessionConfig;
    use mm2_game::gold::{CarrierLoad, CnrVariant, EndRule, GoldMatch, GoldRules};

    fn session(authority: SessionAuthority) -> Session {
        let mut session = Session::new();
        session
            .begin(SessionConfig {
                authority,
                ..SessionConfig::default()
            })
            .unwrap();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Playing).unwrap();
        session.transition(SessionPhase::Results).unwrap();
        session
    }

    /// A match that has already run out its clock.
    fn decided(session: &mut Session) -> GoldMatch {
        let rules = GoldRules {
            variant: CnrVariant::FreeForAll,
            end: EndRule::Ticks(5),
            load: CarrierLoad {
                added_mass_kg: 250.0,
                handling_scalar: 0.9,
            },
            pickup_points: 25,
            delivery_points: 100,
            pickup_radius: 5.0,
            delivery_radius: 12.0,
            drop_lockout_ticks: 120,
        };
        let pool = (0..4)
            .map(|i| Vec3::new(i as f32 * 90.0, 0.0, 20.0))
            .collect();
        let mut game = GoldMatch::new(
            session.generation(),
            session.mint_object_id(),
            rules,
            pool,
            3,
            &[],
        )
        .unwrap();
        for _ in 0..10 {
            game.tick();
        }
        assert!(game.outcome().is_some());
        game
    }

    fn shown(app: &mut App) -> Vec<String> {
        let mut q = app.world_mut().query_filtered::<&Text, With<ResultsUi>>();
        q.iter(app.world()).map(|t| t.0.clone()).collect()
    }

    #[test]
    fn the_rows_follow_who_owns_the_restart() {
        let labels = |kind| {
            results_rows(true, kind)
                .into_iter()
                .map(|r| r.text)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            labels(ResultsKind::Race),
            ["Continue to menu", "Restart race"]
        );
        assert_eq!(
            labels(ResultsKind::CnrLocal),
            ["Continue to menu", "Play again"]
        );
        // A lesson retries; it does not "restart a race".
        assert_eq!(
            labels(ResultsKind::Lesson),
            ["Continue to menu", "Retry lesson"]
        );
        // The lobby mints the next generation: one way on, no fake
        // "play again" that the lifecycle would swallow.
        assert_eq!(labels(ResultsKind::CnrNetworked), ["Back to lobby"]);
    }

    #[test]
    fn a_match_is_networked_unless_the_session_is_local() {
        let mut s = session(SessionAuthority::Local);
        let game = decided(&mut s);
        let replica = CnrReplica(game.view());
        let host = crate::cnr::CnrHost::new(game);
        for (authority, kind) in [
            (SessionAuthority::Local, ResultsKind::CnrLocal),
            (SessionAuthority::Host, ResultsKind::CnrNetworked),
            (SessionAuthority::Remote, ResultsKind::CnrNetworked),
        ] {
            let s = session(authority);
            assert_eq!(ResultsKind::of(&s, Some(&host), None), kind);
            assert_eq!(ResultsKind::of(&s, None, Some(&replica)), kind);
            assert_eq!(ResultsKind::of(&s, None, None), ResultsKind::Race);
        }
    }

    #[test]
    fn a_joined_client_sees_the_hosts_verdict_and_one_way_back() {
        let mut s = session(SessionAuthority::Remote);
        let replica = CnrReplica(decided(&mut s).view());
        let mut app = App::new();
        app.insert_resource(s)
            .insert_resource(replica)
            .init_resource::<ResultsMenu>()
            .add_systems(Update, results_present);
        app.update();
        let shown = shown(&mut app);
        assert!(shown.iter().any(|t| t == "Cops & Robbers"), "{shown:?}");
        assert!(
            shown.iter().any(|t| t.starts_with("time - played")),
            "{shown:?}"
        );
        assert!(
            shown.iter().any(|t| t.contains("Back to lobby")),
            "{shown:?}"
        );
        assert!(
            !shown
                .iter()
                .any(|t| t.contains("Restart race") || t.contains("Play again")),
            "{shown:?}"
        );
    }
}
