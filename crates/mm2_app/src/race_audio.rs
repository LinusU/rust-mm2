//! Listener-side race effects from the mounted install. The sample names are
//! shipped assets; their trigger mapping and gain are designed (see audio notes).
use bevy::{audio::Volume, prelude::*};
use mm2_game::{
    CheckpointRule, Mm2Vfs, ParticipantState, Player, PlayerControl, PlayerId, RACE_TICK_HZ,
    RacePhase, RaceProgress, RaceState, Session, SessionEntity, SessionPhase,
};

use crate::{
    audio::{AudioReport, AudioVoice, PcmAudio, VoiceKind, WaveBank},
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
    warned: bool,
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
    vfs: Option<Res<Mm2Vfs>>,
    bank: Option<ResMut<WaveBank>>,
    mut waves: ResMut<Assets<PcmAudio>>,
    mut report: ResMut<AudioReport>,
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
            }
            ParticipantState::TimedOut { .. } => {
                cues.push("youlose");
                watch.terminal = true;
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
        if !watch.terminal
            && !watch.warned
            && matches!(race.phase, RacePhase::Running)
            && race
                .time_remaining()
                .is_some_and(|ticks| ticks > 0 && ticks <= LOW_TIME_TICKS)
        {
            watch.warned = true;
            cues.push("timerwarning");
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
