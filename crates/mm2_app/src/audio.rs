//! F07-A.2: runtime audio — bounded PCM decode, authored wave
//! resolution, and the session-scoped voice lifecycle.
//!
//! The retail `aud/**` census is entirely uncompressed 16-bit PCM (F07-A),
//! so playback decodes through the project's own bounded
//! [`mm2_formats::wav`] parser rather than a codec chain: a [`PcmAudio`]
//! asset holds normalized f32 samples and feeds Bevy's mixer through
//! [`Decodable`], giving every voice a provable decode path and explicit
//! unsupported-format errors instead of a runtime "unrecognized format".
//!
//! Voices are entities stamped [`SessionEntity`] so the session
//! teardown's `despawn_session_entities` cleans them like every other
//! session object — a restart or unload can never strand a playing
//! sound (F07-AC06's lifecycle half). One-shot voices use
//! [`PlaybackMode::Despawn`]: the mixer removes the entity when the clip
//! ends, so nothing accumulates between presses.
//!
//! The first wired driver is the authored horn (CTL-1's ENTER): the
//! cardata `Horn wave name` resolves through the VFS stem index and
//! plays non-spatial at the authored volume.
//!
//! F07-B.1 adds the engine rig: each drivable `Engine wave name` row
//! spawns one looping voice as a child of the player vehicle, and
//! [`engine_drive`] re-mixes every loop's volume/pitch from the sim's
//! engine RPM through the authored fade windows (`mm2_game`'s
//! [`EngineLoopSpec`] — the interpretation is inferred, UNK-25).
//! Voices are children so they despawn with their car and already sit
//! at its transform for the spatial leg; they loop at volume 0 outside
//! their RPM band rather than attaching/detaching. Impacts, skids,
//! opponent/ambient engines and the spatial mix remain F07-B work;
//! which side's cardata a horn should read for opponents, the clutch
//! sample's trigger, `flags` and the aud11 variants remain UNK-25.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use bevy::audio::{
    AudioPlayer, AudioSink, AudioSinkPlayback, ChannelCount, Decodable, PlaybackMode,
    PlaybackSettings, Sample, SampleRate, Source, SpatialAudioSink, Volume,
};
use bevy::prelude::*;
use bevy::reflect::TypePath;
use tracing::warn;

use mm2_assets::Vfs;
use mm2_formats::wav::{FORMAT_PCM, Wav, lookup_stem};
use mm2_game::{
    EngineLoopSpec, EngineMix, Mm2Vfs, PlayerVehicle, Session, SessionEntity, SessionPhase,
    VehicleAudio,
};
use mm2_vehicle::VehicleState;

/// Decode bound: samples (per channel-interleaved count) beyond this
/// are refused — retail waves top out under ~2.5 M samples; the cap
/// exists so a malformed size field can never drive a giant allocation
/// or an effectively endless voice.
const MAX_DECODE_SAMPLES: usize = 8 * 1024 * 1024;
/// Live one-shot horn voices the mixer will hold at once — past this a
/// press is counted and dropped rather than stacking voices (F07's
/// bounded-voices requirement; designed bound).
const MAX_HORN_VOICES: usize = 8;
/// Engine loops one vehicle's rig will spawn — retail tops out at 4
/// authored rows; the cap keeps a malformed giant table from flooding
/// the mixer (designed bound, same contract as `MAX_HORN_VOICES`).
const MAX_ENGINE_VOICES: usize = 8;

/// A decoded, playback-ready wave: normalized interleaved f32 samples
/// plus the authored rate/channel shape. Produced only by
/// [`decode_wave`], so a live `PcmAudio` always decodes.
#[derive(Asset, TypePath, Clone, Debug)]
pub struct PcmAudio {
    /// Interleaved samples normalized to `[-1.0, 1.0]` (`i16` → f32).
    pub samples: Arc<[f32]>,
    /// Channel count.
    pub channels: ChannelCount,
    /// Frames per second.
    pub sample_rate: SampleRate,
}

impl PcmAudio {
    /// Whole frames in the clip.
    fn frames(&self) -> usize {
        self.samples.len() / usize::from(self.channels.get())
    }
}

/// Streaming view over a [`PcmAudio`] — what the mixer pulls samples
/// from. Cheap to build: shares the `Arc`'d sample buffer.
pub struct PcmDecoder {
    audio: PcmAudio,
    pos: usize,
}

impl Iterator for PcmDecoder {
    type Item = Sample;

    fn next(&mut self) -> Option<Sample> {
        let s = *self.audio.samples.get(self.pos)?;
        self.pos += 1;
        Some(s)
    }
}

impl Source for PcmDecoder {
    fn current_span_len(&self) -> Option<usize> {
        Some((self.audio.samples.len() - self.pos) / usize::from(self.audio.channels.get()))
    }

    fn channels(&self) -> ChannelCount {
        self.audio.channels
    }

    fn sample_rate(&self) -> SampleRate {
        self.audio.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        Some(Duration::from_secs_f64(
            self.audio.frames() as f64 / f64::from(self.audio.sample_rate.get()),
        ))
    }
}

impl Decodable for PcmAudio {
    type Decoder = PcmDecoder;

    fn decoder(&self) -> Self::Decoder {
        PcmDecoder {
            audio: self.clone(),
            pos: 0,
        }
    }
}

/// Decode one wave file into a [`PcmAudio`]. Errors are explicit:
/// structural RIFF failures, non-PCM/unsupported encodings and the
/// decode bound all surface as strings the caller counts and logs —
/// never a panic or a silent skip.
pub fn decode_wave(bytes: &[u8]) -> Result<PcmAudio, String> {
    let w = Wav::parse(bytes).map_err(|e| e.to_string())?;
    if w.fmt.tag != FORMAT_PCM {
        return Err(format!("unsupported wave format tag {}", w.fmt.tag));
    }
    if w.fmt.bits_per_sample != 16 {
        return Err(format!(
            "unsupported {}-bit wave (16-bit PCM only)",
            w.fmt.bits_per_sample
        ));
    }
    let Some(channels) = ChannelCount::new(w.fmt.channels) else {
        return Err("zero-channel wave".into());
    };
    let Some(sample_rate) = SampleRate::new(w.fmt.sample_rate) else {
        return Err("zero-rate wave".into());
    };
    // The bound is checked on the payload before materializing samples
    // — a giant authored data chunk never drives a giant allocation.
    if w.pcm.len() / 2 > MAX_DECODE_SAMPLES {
        return Err(format!(
            "{} samples exceeds the {}-sample decode bound",
            w.pcm.len() / 2,
            MAX_DECODE_SAMPLES
        ));
    }
    let samples = w
        .samples_i16()
        .ok_or_else(|| "no 16-bit PCM payload".to_string())?;
    Ok(PcmAudio {
        samples: samples
            .iter()
            .map(|s| f32::from(*s) * (1.0 / 32768.0))
            .collect(),
        channels,
        sample_rate,
    })
}

/// Session-scoped wave resolver + decoded-asset cache: indexes every
/// `aud/**.wav` the VFS exposes by [`lookup_stem`], preferring the
/// 22 kHz tree when retail ships a sample under both `aud/aud11/` and
/// `aud/aud22/` (designed preference — which tree the original picks
/// per reference is UNK-25; 22 kHz is the higher-fidelity copy of the
/// same authored sample). Inserted by `load_session_world`, removed on
/// teardown.
#[derive(Resource, Default)]
pub struct WaveBank {
    /// Lookup stem → logical wave path.
    stems: HashMap<String, String>,
    /// Logical path → decoded asset (handles outlive the bank; the
    /// asset store is app-global).
    cache: HashMap<String, Handle<PcmAudio>>,
}

impl WaveBank {
    /// Index the VFS. Preference per stem: `aud/aud22/` first, then any
    /// `aud/audNN/`, then the rest of `aud/`; within a tier the higher
    /// declared `.<n>k` rate wins, then the lexicographically smaller
    /// path — deterministic for any mount set.
    pub fn index(vfs: &Vfs) -> Self {
        let mut stems: HashMap<String, String> = HashMap::new();
        for logical in vfs.list() {
            if !(logical.starts_with("aud/") && logical.ends_with(".wav")) {
                continue;
            }
            let stem = lookup_stem(&logical);
            let better = match stems.get(&stem) {
                None => true,
                Some(cur) => {
                    let (new, old) = (wave_rank(&logical), wave_rank(cur));
                    new > old || (new == old && logical < *cur)
                }
            };
            if better {
                stems.insert(stem, logical);
            }
        }
        WaveBank {
            stems,
            ..Default::default()
        }
    }

    /// Resolve a cardata sample name (no directory, no suffix) to a
    /// decoded [`PcmAudio`] handle, decoding on first use.
    pub fn load(
        &mut self,
        vfs: &Vfs,
        waves: &mut Assets<PcmAudio>,
        name: &str,
    ) -> Result<Handle<PcmAudio>, String> {
        let stem = name.to_ascii_lowercase();
        let logical = self
            .stems
            .get(&stem)
            .cloned()
            .ok_or_else(|| format!("no wave matches stem {stem:?}"))?;
        if let Some(h) = self.cache.get(&logical) {
            return Ok(h.clone());
        }
        let bytes = vfs
            .read_logical(&logical)
            .map_err(|e| format!("{logical}: {e}"))?;
        let audio = decode_wave(&bytes).map_err(|e| format!("{logical}: {e}"))?;
        let h = waves.add(audio);
        self.cache.insert(logical, h.clone());
        Ok(h)
    }
}

/// Variant preference for one stem — `(tree tier, declared rate kHz)`.
fn wave_rank(logical: &str) -> (u32, u32) {
    let tier = if logical.starts_with("aud/aud22/") {
        2
    } else if logical.starts_with("aud/aud") {
        1
    } else {
        0
    };
    let rate = logical
        .rsplit_once('.')
        .and_then(|(s, _)| s.rsplit_once('.'))
        .and_then(|(_, sfx)| {
            sfx.strip_suffix(['k', 'K'])
                .and_then(|n| n.parse::<u32>().ok())
        })
        .unwrap_or(0);
    (tier, rate)
}

/// One playing/queued sound entity. Despawns with its session via
/// [`SessionEntity`]; one-shots additionally self-despawn at clip end
/// through [`PlaybackMode::Despawn`].
#[derive(Component)]
pub struct AudioVoice {
    /// Which authored binding drives this voice.
    pub kind: VoiceKind,
}

/// The authored binding a voice plays (grows with F07-B drivers).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceKind {
    /// The cardata horn sample.
    Horn,
    /// One `Engine wave name` loop of a vehicle's engine rig.
    Engine,
}

/// Marker on a vehicle whose engine rig was built — set once whether
/// or not any row produced a voice, so a car whose rows all fail
/// reports once instead of retrying (and re-warning) every frame.
#[derive(Component)]
pub struct EngineRig;

/// One looping voice bound to an authored `Engine wave name` row, a
/// child of the vehicle so it despawns with the car and rides its
/// transform for the spatial leg. `mix` is rewritten every drive tick
/// whether or not a sink exists — headless runs carry the computed
/// mixer state on the component, which is what tests and the smoke
/// record read.
#[derive(Component)]
pub struct EngineVoice {
    /// Index into the parent's `VehicleAudio::spec.engine_samples`.
    pub row: usize,
    /// The resolved fade-window spec, captured at rig build.
    pub spec: EngineLoopSpec,
    /// The mixer state [`engine_drive`] last computed.
    pub mix: EngineMix,
}

/// A horn actuation intent — written by [`horn_input`] (live input) and
/// `dev_horn_once` (`--horn` evidence runs, where capture freezes input
/// upstream), consumed by [`horn_voices`]. One message = one press.
#[derive(Message)]
pub struct HornRequest;

/// Session-scoped audio counters the smoke record reports (`aud=`).
#[derive(Resource, Default, Debug)]
pub struct AudioReport {
    /// Horn presses consumed this session.
    pub horns: u64,
    /// Voice entities spawned this session.
    pub voices: u64,
    /// Voices the output device attached a sink to (0 headless/no-device).
    pub sunk: u64,
    /// Presses/loops refused by a voice bound.
    pub dropped: u64,
    /// Resolves/decodes that failed this session.
    pub failed: u64,
    /// Engine loop voices spawned this session (a subset of `voices`).
    pub loops: u64,
    /// Engine loops whose last computed mix is audible — a gauge
    /// rewritten every drive pass, not a cumulative count.
    pub audible: u64,
}

impl AudioReport {
    /// Any activity worth reporting (`aud=` stays absent otherwise).
    pub fn active(&self) -> bool {
        self.horns + self.voices + self.dropped + self.failed + self.loops > 0
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Session-scoped reset — scheduled `run_if(session::unloading)`
/// alongside the other report resets `drive_session` performs (kept
/// separate so the driver stays under Bevy's system-parameter limit).
pub fn reset_audio_report(mut report: ResMut<AudioReport>) {
    report.reset();
}

/// Horn press → [`HornRequest`] — ENTER per CTL-1, gated like driving
/// input: `Playing` session and a focused window (headless has no
/// windows → focused). Capture runs freeze this system with the other
/// input systems; `--horn` reaches the same consumer through
/// [`dev_horn_once`].
pub fn horn_input(
    keys: Res<ButtonInput<KeyCode>>,
    session: Res<Session>,
    windows: Query<&Window>,
    mut requests: MessageWriter<HornRequest>,
) {
    let focused = windows.iter().all(|w| w.focused);
    if keys.just_pressed(KeyCode::Enter) && session.is_playing() && focused {
        requests.write(HornRequest);
    }
}

/// `--horn` driver: one [`HornRequest`] on the first `Playing` frame,
/// reading the flag through the session config so headless runs reach
/// it. Mirrors `pause::dev_pause_once`.
pub fn dev_horn_once(
    session: Res<Session>,
    mut fired: Local<bool>,
    mut requests: MessageWriter<HornRequest>,
) {
    if *fired || !session.config().is_some_and(|c| c.dev.horn) {
        return;
    }
    if session.is_playing() {
        requests.write(HornRequest);
        *fired = true;
    }
}

/// Consume [`HornRequest`]s: resolve the local player's authored horn
/// through the [`WaveBank`] and spawn one bounded one-shot voice per
/// press. Player-only for now — opponent/ambient horn wiring (and the
/// spatial mix) is F07-B.
#[allow(clippy::too_many_arguments)]
pub fn horn_voices(
    mut commands: Commands,
    mut requests: MessageReader<HornRequest>,
    session: Res<Session>,
    vfs: Option<Res<Mm2Vfs>>,
    bank: Option<ResMut<WaveBank>>,
    mut waves: ResMut<Assets<PcmAudio>>,
    mut report: ResMut<AudioReport>,
    cars: Query<&VehicleAudio, With<PlayerVehicle>>,
    voices: Query<&AudioVoice>,
) {
    let pending = requests.read().count();
    if pending == 0 {
        return;
    }
    report.horns += pending as u64;
    let (Some(vfs), Some(mut bank)) = (vfs, bank) else {
        // No mounted content or no session world: presses are counted,
        // nothing can resolve.
        report.failed += pending as u64;
        return;
    };
    let Some(car) = cars.iter().next() else {
        return;
    };
    let mut live = voices.iter().filter(|v| v.kind == VoiceKind::Horn).count();
    for _ in 0..pending {
        if live >= MAX_HORN_VOICES {
            report.dropped += 1;
            continue;
        }
        match bank.load(&vfs.0, &mut waves, &car.spec.horn.name) {
            Ok(handle) => {
                commands.spawn((
                    AudioVoice {
                        kind: VoiceKind::Horn,
                    },
                    SessionEntity(session.generation()),
                    AudioPlayer(handle),
                    PlaybackSettings {
                        mode: PlaybackMode::Despawn,
                        volume: Volume::Linear(horn_volume(car.spec.horn.volume)),
                        ..Default::default()
                    },
                ));
                report.voices += 1;
                live += 1;
            }
            Err(e) => {
                report.failed += 1;
                warn!("audio: {e}");
            }
        }
    }
}

/// Authored volume sanitized for the mixer — a non-finite or negative
/// cardata value binds 1.0 rather than poisoning the sink (the parse
/// anomaly is already surfaced at load; designed guard).
fn horn_volume(v: f32) -> f32 {
    if v.is_finite() && v >= 0.0 { v } else { 1.0 }
}

/// Build the local player's engine rig: one [`PlaybackMode::Loop`]
/// voice per `Engine wave name` row that resolves both a fade-window
/// spec and a wave, parented to the car so it despawns with it and
/// sits at its transform for the spatial leg. Player-only — opponent
/// and ambient engines need the spatial+listener leg before more
/// voices are worth spawning (F07-B). A row that cannot drive (no
/// fade-window schema) or cannot decode is counted and warned, never
/// silently skipped.
#[allow(clippy::too_many_arguments, clippy::type_complexity)] // Bevy system signature — the filtered query is the system's real input
pub fn engine_rigs(
    mut commands: Commands,
    session: Res<Session>,
    vfs: Option<Res<Mm2Vfs>>,
    bank: Option<ResMut<WaveBank>>,
    mut waves: ResMut<Assets<PcmAudio>>,
    mut report: ResMut<AudioReport>,
    cars: Query<(Entity, &VehicleAudio), (With<PlayerVehicle>, Without<EngineRig>)>,
) {
    if cars.is_empty() {
        return;
    }
    let (Some(vfs), Some(mut bank)) = (vfs, bank) else {
        // No mounted content or no session world yet — retry next
        // frame rather than stamping a half-built rig.
        return;
    };
    for (car, audio) in &cars {
        let mut spawned = 0usize;
        for (i, row) in audio.spec.engine_samples.iter().enumerate() {
            if spawned >= MAX_ENGINE_VOICES {
                report.dropped += 1;
                warn!("audio: engine rig full, dropping row {:?}", row.name);
                continue;
            }
            let Some(spec) = EngineLoopSpec::from_row(row) else {
                report.failed += 1;
                warn!(
                    "audio: engine sample {:?} has no fade-window schema — skipped",
                    row.name
                );
                continue;
            };
            match bank.load(&vfs.0, &mut waves, &row.name) {
                Ok(handle) => {
                    commands.spawn((
                        AudioVoice {
                            kind: VoiceKind::Engine,
                        },
                        EngineVoice {
                            row: i,
                            spec,
                            mix: EngineMix {
                                volume: 0.0,
                                speed: 1.0,
                            },
                        },
                        SessionEntity(session.generation()),
                        ChildOf(car),
                        Transform::default(),
                        AudioPlayer(handle),
                        PlaybackSettings {
                            mode: PlaybackMode::Loop,
                            // Silent until `engine_drive` computes the
                            // real mix — a sink attaching between spawn
                            // and the first drive tick plays nothing.
                            volume: Volume::Linear(0.0),
                            ..Default::default()
                        },
                    ));
                    spawned += 1;
                    report.voices += 1;
                    report.loops += 1;
                }
                Err(e) => {
                    report.failed += 1;
                    warn!("audio: {e}");
                }
            }
        }
        commands.entity(car).insert(EngineRig);
    }
}

/// Re-mix every engine loop from its parent vehicle's RPM and apply it
/// to the sink once the device attaches one. Ungated by phase: voices
/// only exist inside a live session, `Paused` sinks are held by
/// [`sync_audio_pause`], and `Countdown`/`Results` legitimately keep
/// the engine sounding — the car is live, just not drivable.
pub fn engine_drive(
    mut report: ResMut<AudioReport>,
    cars: Query<&VehicleState>,
    mut voices: Query<(&ChildOf, &mut EngineVoice, Option<&mut AudioSink>)>,
) {
    report.audible = 0;
    for (parent, mut voice, sink) in &mut voices {
        let Ok(state) = cars.get(parent.parent()) else {
            // The car despawned and the cascade has not flushed — the
            // voice dies with it; leave the last computed mix.
            continue;
        };
        voice.mix = voice.spec.mix(state.rpm);
        if voice.mix.volume > 0.0 {
            report.audible += 1;
        }
        if let Some(mut sink) = sink {
            sink.set_volume(Volume::Linear(voice.mix.volume));
            sink.set_speed(voice.mix.speed);
        }
    }
}

/// Count voices the output device has attached a sink to — the
/// observable difference between "voice spawned" and "mixer accepted
/// it" (F07-AC05's evidence hook: headless runs stay `0s`, a device
/// run shows the voice sink).
pub fn count_sinks(
    mut report: ResMut<AudioReport>,
    plain: Query<(), (With<AudioVoice>, Added<AudioSink>)>,
    spatial: Query<(), (With<AudioVoice>, Added<SpatialAudioSink>)>,
) {
    report.sunk += (plain.iter().count() + spatial.iter().count()) as u64;
}

/// Pause/resume live voices with the session — voice entities whose
/// sink exists yet follow the phase each update, so a pause between
/// spawn and sink attach still lands paused (F07's pause half; unload
/// is covered by [`SessionEntity`] teardown).
pub fn sync_audio_pause(
    session: Res<Session>,
    voices: Query<(Option<&AudioSink>, Option<&SpatialAudioSink>), With<AudioVoice>>,
) {
    let paused = matches!(session.phase(), SessionPhase::Paused);
    for (sink, spatial) in &voices {
        for s in [
            sink.map(|s| -> &dyn AudioSinkPlayback { s }),
            spatial.map(|s| -> &dyn AudioSinkPlayback { s }),
        ]
        .into_iter()
        .flatten()
        {
            match (paused, s.is_paused()) {
                (true, false) => s.pause(),
                (false, true) => s.play(),
                _ => {}
            }
        }
    }
}
