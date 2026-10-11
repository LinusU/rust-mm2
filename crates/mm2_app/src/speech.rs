//! One speech floor for every dialogue source (F08-B.2).
//!
//! The pre-race announcer and lesson instructor ([`CommentaryAudio`]),
//! the race-end/closing-gate calls and the Cops & Robbers announcer
//! ([`CnrCommentary`]) used to queue and time their lines
//! independently, so two sources could talk over each other. They now
//! only *resolve* a line (registry, cue table, seeded wave draw,
//! decode) and [`SpeechArbiter::submit`] it; [`speech_arbiter`] is the
//! single place a dialogue voice is spawned, which is what makes the
//! rules below hold across sources:
//!
//! - **Priority**: [`SpeechPriority`] orders the queue; the highest
//!   waiting line speaks first, first come first served inside a level.
//! - **No overlap**: one line speaks at a time. A strictly higher
//!   priority line *interrupts* the one in progress (it is despawned
//!   and counted `interrupted`); an equal or lower one waits its turn
//!   and the [`SPEECH_GAP`] after the clip.
//! - **Bounded**: at most [`MAX_QUEUED_LINES`] wait. A full queue
//!   evicts its lowest-priority, oldest line for a strictly higher
//!   one, else refuses the newcomer; either way one `dropped`.
//! - **Staleness**: a line may carry a maximum age; one overtaken by
//!   play is dropped (`dropped`), not spoken late.
//! - **Repeat suppression**: a line that opts in is not started again
//!   while the same wave started within [`REPEAT_WINDOW_SECS`]
//!   (`suppressed`). The Cops & Robbers calls opt out — each pickup is
//!   a distinct event and a single-wave family legitimately repeats.
//! - **Cleanup**: the arbiter is stamped with the session generation;
//!   a line or voice of an older generation never survives into the
//!   next one, and teardown clears it (voices are `SessionEntity`
//!   stamped and despawn with the session).
//!
//! The policy is *designed*: how the original arbitrated speech is
//! unrecovered (UNK-25); see `docs/research/audio.md`.

use std::collections::VecDeque;

use bevy::{audio::PlaybackMode, audio::Volume, prelude::*};
use mm2_game::{Session, SessionEntity, SessionPhase};

use crate::audio::{
    AudioReport, AudioVoice, COMMENTARY_VOLUME, CommentaryVoice, PcmAudio, VoiceKind,
};

/// Silence left between two lines (designed, like the pre-race gap).
pub const SPEECH_GAP: f32 = 0.25;

/// Most lines waiting for the floor, across every source.
pub const MAX_QUEUED_LINES: usize = 6;

/// A wave that opted into repeat suppression is not started again
/// within this many seconds of its last start.
pub const REPEAT_WINDOW_SECS: f32 = 10.0;

/// Who may speak over whom, lowest first. Designed: a result outranks
/// a checkpoint call, which outranks a calls-of-the-match line, which
/// outranks the scene-setting pre-race lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SpeechPriority {
    /// Pre-race weather/time-of-day lines and a lesson's intro.
    Ambience,
    /// A Cops & Robbers call (pickup, drop, stash).
    Call,
    /// The closing-gate line (`FINALCHECKPOINT`).
    Checkpoint,
    /// A race result or a lesson verdict.
    Result,
}

/// One resolved, decoded line asking for the floor.
pub struct SpeechLine {
    /// The resolved bank stem (`as1wearain02`).
    pub stem: String,
    /// The decoded clip.
    pub clip: Handle<PcmAudio>,
    /// The clip's length in seconds.
    pub secs: f32,
    /// How hard this line may push.
    pub priority: SpeechPriority,
    /// Drop the line if it is still waiting after this many seconds.
    pub max_age: Option<f32>,
    /// Whether [`REPEAT_WINDOW_SECS`] applies to this line.
    pub suppress_repeats: bool,
}

struct Queued {
    line: SpeechLine,
    /// Arbiter clock when queued.
    at: f32,
}

struct Playing {
    entity: Entity,
    priority: SpeechPriority,
    /// Arbiter clock the clip ends at.
    ends_at: f32,
    /// Arbiter clock the next equal-or-lower line may start at.
    next_at: f32,
}

/// The session's speech floor. Inserted once with the app; its state
/// belongs to one session generation and resets when that changes.
#[derive(Resource, Default)]
pub struct SpeechArbiter {
    generation: Option<u64>,
    /// Seconds of live session this generation has run.
    clock: f32,
    queue: VecDeque<Queued>,
    playing: Option<Playing>,
    /// `(stem, clock it started at)` of recently started lines.
    recent: Vec<(String, f32)>,
}

impl SpeechArbiter {
    /// Adopt `generation`, discarding every line and clock of another
    /// one — a restart never inherits queued or playing dialogue.
    pub fn begin(&mut self, generation: u64) {
        if self.generation != Some(generation) {
            *self = Self {
                generation: Some(generation),
                ..Self::default()
            };
        }
    }

    /// Forget everything (session teardown).
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Lines waiting for the floor.
    pub fn queued(&self) -> usize {
        self.queue.len()
    }

    /// Lines of exactly `priority` waiting for the floor.
    pub fn queued_at(&self, priority: SpeechPriority) -> usize {
        self.queue
            .iter()
            .filter(|q| q.line.priority == priority)
            .count()
    }

    /// Whether a line is speaking (or inside the gap after one).
    pub fn is_speaking(&self) -> bool {
        self.playing
            .as_ref()
            .is_some_and(|p| self.clock < p.next_at)
    }

    /// Queue `line` for session `generation`. A full queue evicts its
    /// lowest-priority, oldest line for a strictly higher one and
    /// refuses an equal or lower newcomer; either way one `dropped`.
    /// Returns whether the line was accepted.
    pub fn submit(&mut self, generation: u64, line: SpeechLine, report: &mut AudioReport) -> bool {
        self.begin(generation);
        if self.queue.len() >= MAX_QUEUED_LINES {
            let victim = self
                .queue
                .iter()
                .enumerate()
                .min_by_key(|(i, q)| (q.line.priority, *i))
                .map(|(i, q)| (i, q.line.priority));
            report.dropped += 1;
            match victim {
                Some((i, lowest)) if lowest < line.priority => {
                    self.queue.remove(i);
                }
                _ => return false,
            }
        }
        self.queue.push_back(Queued {
            line,
            at: self.clock,
        });
        true
    }

    /// The index of the line that should speak next: highest priority,
    /// then oldest.
    fn best(&self) -> Option<usize> {
        self.queue
            .iter()
            .enumerate()
            .max_by_key(|(i, q)| (q.line.priority, std::cmp::Reverse(*i)))
            .map(|(i, _)| i)
    }
}

/// Give the floor to the waiting lines: the only system that spawns a
/// dialogue voice. Idle outside the live session phases, so a pause
/// holds the clock and a restart's state is discarded by generation.
pub fn speech_arbiter(
    mut commands: Commands,
    session: Res<Session>,
    time: Res<Time>,
    arbiter: Option<ResMut<SpeechArbiter>>,
    mut report: ResMut<AudioReport>,
) {
    let Some(mut arbiter) = arbiter else {
        return;
    };
    let arbiter = &mut *arbiter;
    let generation = session.generation();
    arbiter.begin(generation);
    if !matches!(
        session.phase(),
        SessionPhase::Countdown | SessionPhase::Playing | SessionPhase::Results
    ) {
        return;
    }
    arbiter.clock += time.delta_secs();
    let clock = arbiter.clock;
    let before = arbiter.queue.len();
    arbiter
        .queue
        .retain(|q| q.line.max_age.is_none_or(|max| clock - q.at <= max));
    report.dropped += (before - arbiter.queue.len()) as u64;
    arbiter
        .recent
        .retain(|(_, at)| clock - at <= REPEAT_WINDOW_SECS);

    while let Some(i) = arbiter.best() {
        let priority = arbiter.queue[i].line.priority;
        let may_start = arbiter
            .playing
            .as_ref()
            .is_none_or(|p| clock >= p.next_at || priority > p.priority);
        if !may_start {
            return;
        }
        let Queued { line, .. } = arbiter.queue.remove(i).expect("best() indexes the queue");
        if line.suppress_repeats && arbiter.recent.iter().any(|(stem, _)| *stem == line.stem) {
            report.suppressed += 1;
            continue;
        }
        if let Some(playing) = arbiter.playing.take()
            && clock < playing.ends_at
        {
            commands.entity(playing.entity).try_despawn();
            report.interrupted += 1;
        }
        arbiter.recent.push((line.stem.clone(), clock));
        let entity = commands
            .spawn((
                AudioVoice {
                    kind: VoiceKind::Commentary,
                },
                CommentaryVoice {
                    stem: line.stem.clone(),
                },
                SessionEntity(generation),
                AudioPlayer(line.clip),
                PlaybackSettings {
                    mode: PlaybackMode::Despawn,
                    volume: Volume::Linear(COMMENTARY_VOLUME),
                    ..Default::default()
                },
            ))
            .id();
        report.voices += 1;
        report.commentary += 1;
        arbiter.playing = Some(Playing {
            entity,
            priority: line.priority,
            ends_at: clock + line.secs,
            next_at: clock + line.secs + SPEECH_GAP,
        });
        return;
    }
}
