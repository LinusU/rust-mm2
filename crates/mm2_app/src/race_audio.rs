//! Listener-side race effects from the mounted install. The sample names are
//! shipped assets; their trigger mapping and gain are designed (see audio notes).
use bevy::{audio::Volume, prelude::*};
use mm2_game::{
    CheckpointRule, Mm2Vfs, ParticipantState, Player, PlayerControl, PlayerId, RACE_TICK_HZ,
    RaceDefinition, RacePhase, RaceProgress, RaceState, ResultLedger, Session, SessionEntity,
    SessionPhase,
};

use crate::{
    audio::{
        AudioReport, AudioVoice, CommentaryAudio, EventCue, PcmAudio, ResultsTier, VoiceKind,
        WaveBank,
    },
    race::LOW_TIME_TICKS,
};

/// Traceable even without an audio output device.
#[derive(Component, Debug)]
pub struct RaceCueVoice {
    pub stem: &'static str,
}

/// Tracks authoritative progress, so both local simulation and replicated
/// snapshots use the same cue path. Scoped to generation plus local player.
#[derive(Default)]
pub struct RaceAudioWatch {
    identity: Option<(u64, PlayerId)>,
    progress: u32,
    countdown: Option<u32>,
    started: bool,
    terminal: bool,
    /// The displayed second the low-time beep last sounded for; it only
    /// ever counts down, so a clock that wobbles back across a boundary
    /// cannot repeat a beep.
    warned_second: Option<u32>,
    final_gate: bool,
}

/// Playback lives in Update after session teardown and network snapshots.
/// Pause holds all edge tracking. Results remains eligible for the final cue,
/// even when the finish transitioned the session before this system ran.
#[allow(clippy::too_many_arguments)] // Bevy system borrows independent audio/session resources.
pub fn race_cue_voices(
    mut commands: Commands,
    session: Res<Session>,
    race: Option<Res<RaceState>>,
    participants: Query<(&Player, &RaceProgress)>,
    ledger: Option<Res<ResultLedger>>,
    vfs: Option<Res<Mm2Vfs>>,
    bank: Option<ResMut<WaveBank>>,
    mut waves: ResMut<Assets<PcmAudio>>,
    mut report: ResMut<AudioReport>,
    mut commentary: Option<ResMut<CommentaryAudio>>,
    mut watch: Local<RaceAudioWatch>,
) {
    let Some(race) = race.filter(|race| !race.is_stale(session.generation())) else {
        *watch = RaceAudioWatch::default();
        return;
    };
    if matches!(session.phase(), SessionPhase::Paused) {
        return;
    }
    if !matches!(
        session.phase(),
        SessionPhase::Countdown | SessionPhase::Playing | SessionPhase::Results
    ) {
        *watch = RaceAudioWatch::default();
        return;
    }
    let Some((player, progress)) = participants
        .iter()
        .find(|(player, _)| player.control == PlayerControl::Local)
    else {
        return;
    };
    let total = progress.crossings.saturating_add(progress.route_clears);
    let identity = (session.generation(), player.id);
    if watch.identity != Some(identity) {
        *watch = RaceAudioWatch {
            identity: Some(identity),
            progress: total,
            ..default()
        };
    }
    let mut cues = Vec::with_capacity(2);
    match race.phase {
        RacePhase::Countdown { remaining } => {
            // One low beep per displayed second in the final 3 seconds.
            // Skipped render frames coalesce instead of replaying old beeps.
            let seconds = remaining.div_ceil(RACE_TICK_HZ);
            if seconds > 0 && seconds <= 3 && watch.countdown != Some(seconds) {
                cues.push("startracelow");
            }
            watch.countdown = Some(seconds);
        }
        RacePhase::Running | RacePhase::Complete => {
            if !watch.started {
                watch.started = true;
                if watch.countdown.is_some() {
                    cues.push("startracehigh");
                }
            }
        }
    }
    if !watch.terminal {
        match progress.state {
            ParticipantState::Finished { .. } => {
                cues.push("endofracetag");
                watch.terminal = true;
                // The announcer's verdict (DSN-87): the standing the
                // ledger ranks. A finish with no ranked place says nothing.
                let place = ledger
                    .as_deref()
                    .and_then(|l| l.place_of_in(session.generation(), player.id));
                if let Some(tier) = ResultsTier::for_standing(place, participants.iter().count()) {
                    request_results(commentary.as_deref_mut(), tier);
                }
            }
            ParticipantState::TimedOut { .. } => {
                cues.push("youlose");
                watch.terminal = true;
                request_results(commentary.as_deref_mut(), ResultsTier::Poor);
            }
            _ if total > watch.progress => {
                let last = match race.definition.rule {
                    CheckpointRule::AnyOrder => {
                        progress.cleared_count() == race.definition.checkpoints.len()
                    }
                    CheckpointRule::Ordered => {
                        progress.lap + 1 >= race.definition.laps
                            && progress.next + 1 == race.definition.checkpoints.len()
                    }
                };
                cues.push(if last { "lastwaypoint" } else { "waypoint" });
            }
            _ => {}
        }
        // The announcer's closing-gate line (F08-A): spoken while one
        // checkpoint is still to be crossed — before the race can end,
        // so it lands inside the window commentary plays in. A designed
        // reading of the `FINALCHECKPOINT` section name.
        if !watch.terminal
            && !watch.final_gate
            && matches!(race.phase, RacePhase::Running)
            && final_gate_is_next(&race.definition, progress)
        {
            watch.final_gate = true;
            if let Some(commentary) = commentary.as_deref_mut() {
                commentary.request(EventCue::FinalCheckpoint);
            }
        }
        // One beep each time the displayed second drops (10, 9 … 1) while
        // the clock is inside the low-time window (DSN-65). A render hitch
        // that spans several seconds coalesces to the current one.
        if !watch.terminal && matches!(race.phase, RacePhase::Running) {
            let second = race
                .time_remaining()
                .filter(|&ticks| ticks > 0 && ticks <= LOW_TIME_TICKS)
                .map(|ticks| ticks.div_ceil(RACE_TICK_HZ));
            if let Some(second) = second
                && watch.warned_second.is_none_or(|last| second < last)
            {
                watch.warned_second = Some(second);
                cues.push("timerwarning");
            }
        }
    }
    watch.progress = total;
    let (Some(vfs), Some(mut bank)) = (vfs, bank) else {
        return;
    };
    for stem in cues {
        match bank.load(&vfs.0, &mut waves, stem) {
            Ok(handle) => {
                commands.spawn((
                    SessionEntity(session.generation()),
                    AudioVoice {
                        kind: VoiceKind::RaceCue,
                    },
                    RaceCueVoice { stem },
                    AudioPlayer(handle),
                    PlaybackSettings {
                        mode: bevy::audio::PlaybackMode::Despawn,
                        volume: Volume::Linear(0.85),
                        ..default()
                    },
                ));
                report.voices += 1;
                report.race_cues += 1;
            }
            Err(error) => {
                report.failed += 1;
                tracing::warn!("audio: race cue {stem}: {error}");
            }
        }
    }
}

/// Ask the announcer for the race-end line of `tier`.
fn request_results(commentary: Option<&mut CommentaryAudio>, tier: ResultsTier) {
    if let Some(commentary) = commentary {
        commentary.request(EventCue::Results(tier));
    }
}

/// Whether exactly one checkpoint is left to cross: the final lap's
/// closing gate under `Ordered`, the last uncleared gate under
/// `AnyOrder` (a separate finish trigger does not count — it is not a
/// checkpoint).
fn final_gate_is_next(definition: &RaceDefinition, progress: &RaceProgress) -> bool {
    match definition.rule {
        CheckpointRule::AnyOrder => progress.cleared_count() + 1 == definition.checkpoints.len(),
        CheckpointRule::Ordered => {
            progress.lap + 1 >= definition.laps && progress.next + 1 == definition.checkpoints.len()
        }
    }
}
