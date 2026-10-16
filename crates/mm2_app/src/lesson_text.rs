//! The Crash Course lesson messages on the session HUD (F21 req 4, the
//! subtitle leg).
//!
//! `mm2_content::lesson_text` binds the executable's per-step string
//! ids; this module reads the strings out of the install's `mmlang.dll`
//! and shows them: a leg's pre-race sequence (instruction / Ready / Set /
//! Go, [`PRERACE_MESSAGE_SECONDS`] each) from the moment the leg is in
//! play, and the outcome line ([`OUTCOME_MESSAGE_SECONDS`]) on the leg's
//! verdict, which [`drive_lesson`](crate::lesson::drive_lesson) records
//! on the [`LessonDriver`].
//!
//! Game text is read from the install at session load and never stored
//! in the repository. A string the table lacks, or a DLL that cannot be
//! read, shows nothing and is counted ([`LessonMessages::missing`]) and
//! logged — never replaced with text of our own.
//!
//! Designed, not recovered (`docs/original-rules.md` DSN-92): *where* the
//! line is drawn (the original's display is `0x42e1e0` on the car
//! `+0x288`, layout unrecovered) — a centred line low on the screen —
//! and how a new line meets one still showing: messages queue behind the
//! one on screen, so a cleared leg's outcome is read out before the next
//! leg's instruction. The pre-race sequence runs on its own 1.5 s clock,
//! not on the race countdown's length.

use std::collections::VecDeque;

use bevy::prelude::*;
use mm2_assets::Vfs;
use mm2_content::LessonObjective;
use mm2_content::lesson_text::{
    LessonLine, OUTCOME_MESSAGE_SECONDS, PRERACE_MESSAGE_SECONDS, message_id,
};
use mm2_formats::langdll::StringTable;
use mm2_game::SessionEntity;

use crate::lesson::LessonDriver;

/// Logical path of the language DLL in the install.
pub const LANGUAGE_DLL: &str = "mmlang.dll";

/// Read the install's string table.
pub fn load_string_table(vfs: &Vfs) -> Result<StringTable, String> {
    let bytes = vfs
        .read_logical(LANGUAGE_DLL)
        .map_err(|e| format!("{LANGUAGE_DLL}: {e}"))?;
    StringTable::parse(&bytes).map_err(|e| format!("{LANGUAGE_DLL}: {e}"))
}

/// One message waiting for, or holding, the screen.
#[derive(Debug, Clone, PartialEq)]
struct Shown {
    text: String,
    seconds: f32,
}

/// The session's lesson message line: the string table, what is queued
/// and what is showing. Inserted with the [`LessonDriver`], removed with
/// the session.
#[derive(Resource, Debug)]
pub struct LessonMessages {
    table: Result<StringTable, String>,
    queue: VecDeque<Shown>,
    /// The `(attempt, leg)` whose pre-race sequence was last queued.
    leg_seen: Option<(u32, u32)>,
    /// Lines whose string was absent from the table (or the table
    /// unreadable) and so were not shown.
    pub missing: u32,
}

impl LessonMessages {
    /// A message line over `table`, or the reason it could not be read.
    pub fn new(table: Result<StringTable, String>) -> Self {
        if let Err(error) = &table {
            warn!(%error, "lesson messages unavailable — no instruction text will show");
        }
        Self {
            table,
            queue: VecDeque::new(),
            leg_seen: None,
            missing: 0,
        }
    }

    /// Queue `line` of a leg of `objective` for `seconds`. An unbound
    /// pairing is silent by design; a bound id the table lacks is counted.
    fn push(&mut self, objective: LessonObjective, line: LessonLine, seconds: f32) {
        let Some(id) = message_id(objective, line) else {
            return;
        };
        let text = match &self.table {
            Ok(table) => table.get(id).filter(|t| !t.is_empty()),
            Err(_) => None,
        };
        match text {
            Some(text) => self.queue.push_back(Shown {
                text: text.to_string(),
                seconds,
            }),
            None => {
                self.missing += 1;
                if self.table.is_ok() {
                    warn!(
                        id,
                        ?objective,
                        ?line,
                        "lesson message string missing from the table"
                    );
                }
            }
        }
    }

    /// Queue the pre-race sequence of a leg of `objective`.
    pub fn begin_leg(&mut self, objective: LessonObjective) {
        for line in LessonLine::prerace(objective) {
            self.push(objective, *line, PRERACE_MESSAGE_SECONDS);
        }
    }

    /// Queue a verdict's outcome line.
    pub fn outcome(&mut self, objective: LessonObjective, line: LessonLine) {
        self.push(objective, line, OUTCOME_MESSAGE_SECONDS);
    }

    /// Let `dt` seconds pass over the showing line.
    pub fn tick(&mut self, dt: f32) {
        if let Some(front) = self.queue.front_mut() {
            front.seconds -= dt;
            if front.seconds <= 0.0 {
                self.queue.pop_front();
            }
        }
    }

    /// The line on screen, if any.
    pub fn current(&self) -> Option<&str> {
        self.queue.front().map(|s| s.text.as_str())
    }
}

/// Marker on the session-owned message root (a full-screen flex node).
#[derive(Component)]
pub struct LessonSubtitle;

/// Marker on the message's text child.
#[derive(Component)]
pub struct LessonSubtitleText;

/// Spawn the message line, hidden until [`drive_lesson_subtitles`] has a
/// line. Session-owned like every HUD node.
pub fn spawn_lesson_subtitle(commands: &mut Commands, owner: SessionEntity) {
    commands
        .spawn((
            owner,
            LessonSubtitle,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(0.0),
                left: Val::Px(0.0),
                right: Val::Px(0.0),
                bottom: Val::Px(0.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::End,
                padding: UiRect::bottom(Val::Percent(18.0)),
                ..default()
            },
            Visibility::Hidden,
        ))
        .with_child((
            LessonSubtitleText,
            Text::new(""),
            TextFont {
                font_size: bevy::text::FontSize::Px(34.0),
                ..default()
            },
            TextColor(Color::srgb(1.0, 0.96, 0.8)),
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
        ));
}

/// Queue the lesson's lines as they arise, advance the showing one and
/// draw it. A leg is "in play" the first frame the sequencer reports it,
/// so a retry (a fresh driver, attempt and leg 0) starts its sequence
/// again.
pub fn drive_lesson_subtitles(
    time: Res<Time>,
    hud: Res<crate::hud::HudVisible>,
    driver: Option<ResMut<LessonDriver>>,
    messages: Option<ResMut<LessonMessages>>,
    mut root: Query<&mut Visibility, With<LessonSubtitle>>,
    mut text: Query<&mut Text, With<LessonSubtitleText>>,
) {
    let (Some(mut driver), Some(mut messages)) = (driver, messages) else {
        return;
    };
    // Verdict lines first: a cleared leg's line is read out before the
    // next leg's sequence queues behind it.
    for (objective, line) in driver.take_lines() {
        messages.outcome(objective, line);
    }
    if let Some(leg) = driver.run().current_leg() {
        let key = (driver.run().attempt(), leg);
        if messages.leg_seen != Some(key) {
            messages.leg_seen = Some(key);
            if let Some(objective) = driver.leg(leg as usize).map(|l| l.objective) {
                messages.begin_leg(objective);
            }
        }
    }
    messages.tick(time.delta_secs());
    let line = messages.current().filter(|_| hud.0);
    for mut vis in &mut root {
        *vis = if line.is_some() {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
    if let Some(line) = line {
        for mut t in &mut text {
            if t.0 != line {
                *t = Text::new(line);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic table: the test never needs game text.
    fn table(ids: &[u32]) -> StringTable {
        StringTable::from_pairs(ids.iter().map(|id| (*id, format!("line {id}"))))
    }

    #[test]
    fn prerace_sequence_runs_one_and_a_half_seconds_each() {
        let mut m = LessonMessages::new(Ok(table(&[193, 194, 195])));
        m.begin_leg(LessonObjective::CopChase);
        assert_eq!(m.current(), Some("line 193"));
        m.tick(1.4);
        assert_eq!(m.current(), Some("line 193"));
        m.tick(0.2);
        assert_eq!(m.current(), Some("line 194"));
        m.tick(1.5);
        assert_eq!(m.current(), Some("line 195"));
        m.tick(1.5);
        assert_eq!(m.current(), None);
        assert_eq!(m.missing, 0);
    }

    #[test]
    fn outcome_holds_five_seconds_and_queues_behind_the_showing_line() {
        let mut m = LessonMessages::new(Ok(table(&[241, 196, 209])));
        m.outcome(LessonObjective::Jump, LessonLine::Cleared);
        m.begin_leg(LessonObjective::Maneuver);
        assert_eq!(m.current(), Some("line 209"));
        m.tick(4.9);
        assert_eq!(m.current(), Some("line 209"));
        m.tick(0.2);
        assert_eq!(m.current(), Some("line 241"));
    }

    #[test]
    fn a_missing_string_is_counted_not_substituted() {
        let mut m = LessonMessages::new(Ok(table(&[194])));
        m.begin_leg(LessonObjective::CopChase);
        assert_eq!(m.current(), Some("line 194"));
        assert_eq!(m.missing, 2);
    }

    #[test]
    fn an_unreadable_table_shows_nothing_and_counts_every_line() {
        let mut m = LessonMessages::new(Err("mmlang.dll: not found".into()));
        m.begin_leg(LessonObjective::Follow);
        m.outcome(LessonObjective::Follow, LessonLine::Cleared);
        assert_eq!(m.current(), None);
        assert_eq!(m.missing, 4);
    }

    #[test]
    fn an_unbound_family_is_silent_and_not_counted() {
        let mut m = LessonMessages::new(Ok(table(&[])));
        m.begin_leg(LessonObjective::Corner);
        m.outcome(LessonObjective::CrossTraffic, LessonLine::Cleared);
        assert_eq!(m.current(), None);
        assert_eq!(m.missing, 0);
    }
}
