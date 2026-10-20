//! Menu/UI sound events (F08-B.3): the shell's focus-move and confirm
//! sounds, played through the VFS on the Effects bus.
//!
//! Retail ships its menu clips as plain waves under `aud/aud{11,22}/`:
//! `moveselector` and `selectionmade` (plus per-screen `ui*` stings,
//! `switch` and `messagenote` whose triggers are unrecovered). The
//! binding below is by name and designed, not recovered from the
//! executable: a focus move plays `moveselector`, an enabled `Activate`
//! plays `selectionmade`. No retail clip is identifiable as the *back*
//! sound, so `Back` is an explicit [`UiCue::Back`] that resolves to no
//! stem and is counted as `unmapped` — nothing is invented.
//!
//! There is no UI bus in the Options screen, so these voices ride
//! [`AudioBus::Effects`] (`VoiceKind::Ui`).
//!
//! Rapid navigation is bounded two ways: a focus move within
//! [`MOVE_GAP`] seconds of the last played one is coalesced (the held
//! key's repeat sounds as one steady tick, not a buzz), and no more than
//! [`MAX_VOICES`] UI voices play at once. A confirm is never coalesced
//! but obeys the voice bound.
//!
//! [`AudioBus::Effects`]: crate::settings::AudioBus::Effects

use bevy::prelude::*;
use mm2_game::{EventTableKind, Mm2Vfs};

use crate::{
    audio::{AudioVoice, PcmAudio, VoiceKind, WaveBank},
    menu::{MenuCommand, Screen},
};

/// Seconds within which a second focus move is folded into the first.
pub const MOVE_GAP: f64 = 0.06;

/// UI voices that may sound at once.
pub const MAX_VOICES: usize = 4;

/// Linear gain of a UI voice before the bus level (designed).
const GAIN: f32 = 0.85;

/// Linear gain of a destination sting (designed): under [`GAIN`] so the
/// confirm that opened the screen stays the loudest part of the hit.
const ENTER_GAIN: f32 = 0.6;

/// One UI event the shell produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiCue {
    /// Focus moved to another row or side entry.
    Move,
    /// An enabled row was activated.
    Confirm,
    /// A screen was left.
    Back,
    /// A destination screen was entered going forward.
    Enter(Destination),
}

/// A screen whose forward entry has its own retail sting. The mapping
/// is owner-confirmed (2026-10-19); the clip identity behind each stem
/// is by name until listened to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Destination {
    /// The races hub and Crash Course lessons share one sting.
    Races,
    /// Select Vehicle.
    Vehicles,
}

impl UiCue {
    /// The retail wave stem this cue plays; `None` when the install has
    /// no clip identifiable for it.
    pub fn stem(self) -> Option<&'static str> {
        match self {
            Self::Move => Some("moveselector"),
            Self::Confirm => Some("selectionmade"),
            Self::Back => None,
            Self::Enter(Destination::Races) => Some("uiraces"),
            Self::Enter(Destination::Vehicles) => Some("uivehicles"),
        }
    }

    /// Linear gain before the bus level. A destination sting sits under
    /// the confirm that caused it so the pair does not stack into one
    /// loud hit.
    fn gain(self) -> f32 {
        match self {
            Self::Enter(_) => ENTER_GAIN,
            _ => GAIN,
        }
    }
}

/// The destination a confirmed command entered, if any. Only an
/// activation that moved the shell forward counts: `Back`, focus moves
/// and a paint pick that pops to the garage never do, and the garage
/// reached from its own paints is not a re-entry. `Multiplayer` has no
/// destination to wire (root Multiplayer is disabled; Host/Join are
/// separate C&R screens), so it has no entry here.
pub fn entry(
    cmd: MenuCommand,
    before: &NavSnapshot,
    after: &NavSnapshot,
    activatable: bool,
) -> Option<Destination> {
    if cmd != MenuCommand::Activate || !activatable || before.screen == after.screen {
        return None;
    }
    match (&before.screen, &after.screen) {
        (Screen::Paints { .. }, Screen::Garage) => None,
        (_, Screen::Garage) => Some(Destination::Vehicles),
        (_, Screen::EventTable { .. }) => Some(Destination::Races),
        (
            _,
            Screen::EventList {
                table: EventTableKind::CrashCourse,
                ..
            },
        ) => Some(Destination::Races),
        _ => None,
    }
}

/// What the shell looked like around one command.
#[derive(Debug, Clone, PartialEq)]
pub struct NavSnapshot {
    /// Current screen.
    pub screen: Screen,
    /// Focused row.
    pub focus: usize,
    /// Focus on the row's side entry.
    pub side: bool,
}

/// The cue a command produced, if any. `activatable` is whether the
/// focused row was enabled before the command ran; `effects` how many
/// effects `apply` returned.
pub fn classify(
    cmd: MenuCommand,
    before: &NavSnapshot,
    after: &NavSnapshot,
    activatable: bool,
    effects: usize,
) -> Option<UiCue> {
    match cmd {
        MenuCommand::Up
        | MenuCommand::Down
        | MenuCommand::Left
        | MenuCommand::Right
        | MenuCommand::FocusAt(_)
        | MenuCommand::FocusSide(_) => (before.screen == after.screen
            && (before.focus != after.focus || before.side != after.side))
            .then_some(UiCue::Move),
        MenuCommand::Activate => activatable.then_some(UiCue::Confirm),
        MenuCommand::Back => (before.screen != after.screen || effects > 0).then_some(UiCue::Back),
        _ => None,
    }
}

/// Coalescing and bounded concurrency, free of the ECS so a scripted
/// sequence can be checked exactly.
#[derive(Debug, Default)]
pub struct UiGate {
    last_move: Option<f64>,
}

/// What the gate decided for one cue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Play the clip.
    Play,
    /// Folded into a move that just played.
    Coalesced,
    /// `MAX_VOICES` already sounding.
    Full,
}

impl UiGate {
    /// Decide `cue` at `now` seconds with `live` UI voices sounding.
    pub fn admit(&mut self, cue: UiCue, now: f64, live: usize) -> Verdict {
        if cue == UiCue::Move
            && self
                .last_move
                .is_some_and(|t| now >= t && now - t < MOVE_GAP)
        {
            return Verdict::Coalesced;
        }
        if live >= MAX_VOICES {
            return Verdict::Full;
        }
        if cue == UiCue::Move {
            self.last_move = Some(now);
        }
        Verdict::Play
    }
}

/// Counters a capture or test reads.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct UiSoundReport {
    /// Voices spawned.
    pub played: u64,
    /// Moves folded into a move that just played.
    pub coalesced: u64,
    /// Cues refused by the concurrency bound.
    pub full: u64,
    /// Cues whose event has no retail clip (`Back`).
    pub unmapped: u64,
    /// Stings cut short by a newer destination sting.
    pub cut: u64,
    /// Stems that failed to resolve or decode.
    pub failed: u64,
}

/// The shell's UI sound state; absent in a bare test rig, in which case
/// the menu is silent.
#[derive(Resource, Default)]
pub struct UiSound {
    queue: Vec<UiCue>,
    gate: UiGate,
    bank: Option<WaveBank>,
    /// Counters for this run.
    pub report: UiSoundReport,
}

impl UiSound {
    /// Queue `cue` for the next [`ui_sound_play`] pass.
    pub fn push(&mut self, cue: UiCue) {
        self.queue.push(cue);
    }
}

/// Marks a UI voice; carries the stem for traces.
#[derive(Component, Debug)]
pub struct UiVoice {
    /// The wave stem playing.
    pub stem: &'static str,
    /// A destination sting, replaced by the next one.
    pub enter: bool,
}

/// Play what the shell queued this update.
pub fn ui_sound_play(
    time: Res<Time<Real>>,
    vfs: Res<Mm2Vfs>,
    mut ui: ResMut<UiSound>,
    mut waves: ResMut<Assets<PcmAudio>>,
    live: Query<(Entity, &UiVoice)>,
    mut commands: Commands,
) {
    if ui.queue.is_empty() {
        return;
    }
    let now = time.elapsed_secs_f64();
    let mut live_count = live.iter().count();
    let ui = &mut *ui;
    for cue in std::mem::take(&mut ui.queue) {
        let Some(stem) = cue.stem() else {
            ui.report.unmapped += 1;
            continue;
        };
        // A newer destination sting replaces the one still sounding, so
        // rapid navigation never stacks them.
        if matches!(cue, UiCue::Enter(_)) {
            for (entity, voice) in &live {
                if voice.enter {
                    commands.entity(entity).despawn();
                    live_count = live_count.saturating_sub(1);
                    ui.report.cut += 1;
                }
            }
        }
        match ui.gate.admit(cue, now, live_count) {
            Verdict::Coalesced => {
                ui.report.coalesced += 1;
                continue;
            }
            Verdict::Full => {
                ui.report.full += 1;
                continue;
            }
            Verdict::Play => {}
        }
        let bank = ui.bank.get_or_insert_with(|| WaveBank::index(&vfs.0));
        match bank.load(&vfs.0, &mut waves, stem) {
            Ok(handle) => {
                commands.spawn((
                    AudioVoice {
                        kind: VoiceKind::Ui,
                    },
                    UiVoice {
                        stem,
                        enter: matches!(cue, UiCue::Enter(_)),
                    },
                    AudioPlayer(handle),
                    PlaybackSettings {
                        mode: bevy::audio::PlaybackMode::Despawn,
                        volume: bevy::audio::Volume::Linear(cue.gain()),
                        ..default()
                    },
                ));
                ui.report.played += 1;
                live_count += 1;
            }
            Err(error) => {
                ui.report.failed += 1;
                warn!("ui sound {stem}: {error}");
            }
        }
    }
}
