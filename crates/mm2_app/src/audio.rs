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
//! spawns one looping voice as a child of the vehicle, and
//! [`engine_drive`] re-mixes every loop's volume/pitch from the sim's
//! engine RPM through the authored fade windows (`mm2_game`'s
//! [`EngineLoopSpec`] — the interpretation is inferred, UNK-25).
//! Voices are children so they despawn with their car and sit at its
//! transform; they loop at volume 0 outside their RPM band rather than
//! attaching/detaching.
//!
//! F07-B.2 extends the rig to every `VehicleAudio` car (opponents now
//! spawn with the component) and puts the mix in space: non-player
//! loops are [`PlaybackSettings::spatial`] emitters heard through a
//! single [`SpatialListener`] that [`audio_listener`] keeps on the
//! active 3-D camera — chase, free or cockpit all hear the field from
//! their own viewpoint. The player's own rig stays non-spatial: the
//! local car *is* the listener's anchor, so its engine does not fade
//! with camera distance (designed policy, DSN-37 — Bevy/rodio spatial
//! is inverse-square plus stereo panning, scaled by
//! [`ENGINE_SPATIAL_SCALE`]).
//!
//! F07-B.3 consumes the deduplicated [`ImpactEvent`] stream: each
//! event's *vehicle* participants each earn one bounded one-shot at the
//! contact point (a remote participant's voice belongs to its own
//! client, the same skip every F05 system applies). The sample comes
//! from the session's [`ImpactAudio`] — the player-side
//! `default_impacts.csv` — resolved through [`impact_category`] and
//! [`pick_impact`] (`mm2_game`): the struck side's `dgBangerData`
//! `AudioId` selects the authored `ID` category (everything else,
//! including all retail props at `AudioId` 0, reads the `WALL`
//! catch-all), `severity × striker mass` — the same impulse estimate
//! the knock pipeline weighs — selects the `min,max force` band,
//! `frequency` weights the pick and the authored volume range supplies
//! the gain. Voices are spatial emitters at the impact point for
//! everyone but the local player, whose impacts stay non-spatial like
//! its engine rig (DSN-37).
//!
//! F07-B.4 adds the surface rig: every `Vehicle` car resolves its
//! grounded wheels through [`SurfaceTables`] — `SurfaceMaterial` → the
//! material's authored `sound` class → a row of the session's
//! [`SurfaceAudio`] (the player-side `default_surfacedry.csv`; the
//! weather→variant binding is unverified). The loudest covering
//! `skid wave` band and the loudest `surface wave` rolling loop each
//! earn lazily-spawned `PlaybackMode::Loop` voices, mixed from
//! [`tire_slippage`] (designed quantity — longitudinal over-demand or
//! lateral utilization, UNK-25) and `|forward_speed|` respectively.
//! A held brake at rest and airborne wheels resolve nothing
//! (F07-AC03). Ambient engines, the clutch trigger, siren programs
//! and the scrape semantics remain F07-B/C work; the opponent-side
//! tables' divergent values and the `flags` word remain UNK-25.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use bevy::audio::{
    AudioPlayer, AudioSink, AudioSinkPlayback, ChannelCount, Decodable, PlaybackMode,
    PlaybackSettings, Sample, SampleRate, Source, SpatialAudioSink, SpatialListener, SpatialScale,
    Volume,
};
use bevy::prelude::*;
use bevy::reflect::TypePath;
use tracing::warn;

use avian3d::prelude::{ComputedMass, Mass};
use mm2_assets::Vfs;
use mm2_content::SurfaceTables;
use mm2_formats::cardata::{self, CardataBody, ImpactTable, SurfaceTable};
use mm2_formats::wav::{FORMAT_PCM, Wav, lookup_stem};
use mm2_game::{
    Banger, EngineLoopSpec, EngineMix, ImpactEvent, Mm2Vfs, NavRng, ObjectId, ObjectIdentity,
    Player, PlayerControl, PlayerVehicle, Session, SessionEntity, SessionPhase, SkidUnit,
    SurfaceMaterial, SurfaceSpec, VehicleAudio, impact_category, pick_impact, tire_slippage,
};
use mm2_vehicle::{Vehicle, VehicleState};

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
/// Engine rigs one session will build — retail rosters top out around
/// a dozen participants; the cap bounds total voice count against a
/// mod roster fielding hundreds of `VehicleAudio` cars (designed
/// bound, F07-B.2). Cars past it report once and stay silent.
const MAX_ENGINE_RIGS: usize = 16;
/// Position scale applied to spatial engine emitters (F07-B.2,
/// designed — DSN-37). Rodio's spatial panner attenuates per ear by
/// `min(1, 1/d²)` over the scaled distance, so 0.25 reads a car 4 m
/// off at ~full authored volume, a racing pack (5–15 m) clearly
/// audible, and a 50 m straggler near silence.
const ENGINE_SPATIAL_SCALE: f32 = 0.25;
/// Live one-shot impact voices the mixer will hold at once — a pile-up
/// beyond this is counted and dropped rather than stacking voices
/// (F07-AC04's bounded-voices requirement; designed bound).
const MAX_IMPACT_VOICES: usize = 12;
/// The impact table the session reads: the player-side
/// `default_impacts.csv` — the local listener's authored mix
/// (designed choice: the opponent file authors the same categories
/// but `WALL` bands two orders of magnitude smaller — an authored
/// inconsistency recorded under UNK-25, not a second binding).
const IMPACT_TABLE: &str = "aud/cardata/player/default_impacts.csv";
/// The surface table the session reads: the player-side
/// `default_surfacedry.csv` — the local listener's authored mix
/// (designed choice: which weather selector binds the wet/ice
/// variants is unverified — UNK-25; dry is the neutral default).
const SURFACE_TABLE: &str = "aud/cardata/player/default_surfacedry.csv";
/// Skid band voices one car's rig will hold — retail tops out at 3
/// authored bands per entry; the cap bounds a giant mod table
/// (designed bound, same contract as `MAX_ENGINE_VOICES`).
const MAX_SKID_VOICES: usize = 4;
/// Cars one session will give a surface rig — the same bound the
/// engine rigs take (designed, F07-B.4). Cars past it report once
/// and stay silent.
const MAX_SURFACE_RIGS: usize = 16;
/// Frames a different surface entry must hold before a rig rebuilds
/// its voices — a car straddling a surface boundary can't churn
/// attach/detach every frame (designed debounce).
const SURFACE_DWELL: u8 = 4;
/// Below this `|forward_speed|` a car authors no rolling noise — a
/// parked car on grass does not hum (designed gate; the authored
/// `min surface volume` is the moving car's floor, not a parked
/// drone). Matches the sim's low-speed clamp epsilon.
const ROLL_MIN_SPEED: f32 = 0.5;

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

/// Session-scoped impact table + selection stream (F07-B.3): the
/// parsed player-side `default_impacts.csv` plus the seeded draw the
/// band/frequency picks share. Inserted by `load_session_world` when
/// the table resolves and parses — a missing or malformed table
/// degrades to no resource (warned once, never fabricated), the same
/// absence policy every authored record applies. The draw is seeded
/// per session generation like the spark/texel rigs seed per object,
/// so a replayed session repeats the same picks.
#[derive(Resource)]
pub struct ImpactAudio {
    /// The authored categories in authored order.
    table: ImpactTable,
    /// Deterministic draw stream for band/volume picks.
    rng: NavRng,
}

impl ImpactAudio {
    /// Resolve and parse [`IMPACT_TABLE`] through the VFS; `None` (with
    /// a warn) when the file is absent or the grammar rejects it.
    pub fn load(vfs: &Vfs, generation: u64) -> Option<Self> {
        let bytes = match vfs.read_logical(IMPACT_TABLE) {
            Ok(b) => b,
            Err(e) => {
                warn!("audio: {IMPACT_TABLE}: {e}");
                return None;
            }
        };
        match cardata::parse(IMPACT_TABLE, &bytes) {
            Ok(file) => match file.body {
                CardataBody::Impacts(table) => Some(Self {
                    table,
                    rng: NavRng::new(generation),
                }),
                other => {
                    warn!("audio: {IMPACT_TABLE} parsed as {other:?} — no impact table");
                    None
                }
            },
            Err(e) => {
                warn!("audio: {IMPACT_TABLE}: {e}");
                None
            }
        }
    }
}

/// Session-scoped surface table (F07-B.4): the parsed player-side
/// `default_surfacedry.csv` plus every row's resolved
/// [`SurfaceSpec`], computed once at load so the drive loop stays a
/// numeric read. Inserted by `load_session_world` when the table
/// resolves and parses — same absence policy as [`ImpactAudio`]:
/// absent/malformed warns once and inserts nothing rather than
/// fabricating a surface row.
#[derive(Resource)]
pub struct SurfaceAudio {
    /// The authored rows — band lists live on `surfaces[i].skids`.
    table: SurfaceTable,
    /// `SurfaceSpec::from_entry` per row, parallel to
    /// `table.surfaces` (the index space `sound_index` produces).
    specs: Vec<SurfaceSpec>,
}

impl SurfaceAudio {
    /// Resolve and parse [`SURFACE_TABLE`] through the VFS; `None`
    /// (with a warn) when the file is absent, the grammar rejects it
    /// or it parses as a different cardata kind.
    pub fn load(vfs: &Vfs) -> Option<Self> {
        let bytes = match vfs.read_logical(SURFACE_TABLE) {
            Ok(b) => b,
            Err(e) => {
                warn!("audio: {SURFACE_TABLE}: {e}");
                return None;
            }
        };
        match cardata::parse(SURFACE_TABLE, &bytes) {
            Ok(file) => match file.body {
                CardataBody::Surfaces(table) => Some(Self {
                    specs: table.surfaces.iter().map(SurfaceSpec::from_entry).collect(),
                    table,
                }),
                other => {
                    warn!("audio: {SURFACE_TABLE} parsed as {other:?} — no surface table");
                    None
                }
            },
            Err(e) => {
                warn!("audio: {SURFACE_TABLE}: {e}");
                None
            }
        }
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
    /// One `default_impacts.csv` sample a deduplicated impact spawned.
    Impact,
    /// One `skid wave` band loop of a surface rig (F07-B.4).
    Skid,
    /// One `surface wave` rolling loop of a surface rig (F07-B.4).
    Rolling,
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

/// Per-car surface voice state (F07-B.4): which `sound`-index entries
/// the skid and rolling halves are committed to, plus the voice slots
/// of the committed entry. Band loops are spawned lazily — a `skid
/// wave` row's voice exists only once that band has covered — and an
/// entry rebuild waits for the new entry to hold [`SURFACE_DWELL`]
/// frames, so a car straddling a surface boundary can't churn sink
/// attach/detach every frame. Voices are children of the car like the
/// engine rig's, so they despawn with it and ride its transform.
#[derive(Component, Default)]
pub struct SurfaceRig {
    /// The committed skid entry (`sound` index into the table) —
    /// `None` until a band first covers.
    skid_entry: Option<u16>,
    /// `(entry, consecutive frames)` a different winning entry has
    /// held — the dwell counter for the swap.
    skid_pending: Option<(u16, u8)>,
    /// One slot per committed-entry band, capped at
    /// [`MAX_SKID_VOICES`]. `tried` stops a resolve/decode failure
    /// re-warning every frame.
    skid_slots: Vec<SkidSlot>,
    /// The committed rolling entry, `None` until a rolling surface
    /// first resolves under a moving car.
    rolling_entry: Option<u16>,
    /// Same dwell counter for the rolling half.
    rolling_pending: Option<(u16, u8)>,
    /// Whether the committed rolling sample was attempted (success or
    /// counted failure — never retried per frame).
    rolling_tried: bool,
    /// The rolling voice entity, when spawned.
    rolling_voice: Option<Entity>,
    /// [`MAX_SURFACE_RIGS`] refused this car a rig — counted once.
    muted: bool,
}

/// One band's voice slot inside a committed skid entry.
#[derive(Default)]
struct SkidSlot {
    /// The band's sample was attempted (spawn or counted failure).
    tried: bool,
    /// The spawned band-loop voice.
    voice: Option<Entity>,
}

/// A looping surface voice — a skid band or a rolling loop, a child
/// of the car like [`EngineVoice`]. `mix` is rewritten every drive
/// pass whether or not a sink exists, which is what headless runs and
/// tests read for evidence.
#[derive(Component)]
pub struct SurfaceVoice {
    /// Which half of the surface row this voice plays.
    pub role: SurfaceRole,
    /// The mixer state [`surface_voices`] last computed.
    pub mix: EngineMix,
}

/// Which half of a committed surface entry a [`SurfaceVoice`] plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceRole {
    /// The `skid wave` band at this index in the committed entry.
    Skid(usize),
    /// The entry's `surface wave` rolling loop.
    Rolling,
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
    /// Engine rigs built this session (one per `VehicleAudio` car).
    pub rigs: u64,
    /// Impact voices spawned this session (a subset of `voices`).
    pub impacts: u64,
    /// Skid band voices whose last computed mix is audible — a gauge
    /// rewritten every drive pass, not a cumulative count (F07-B.4).
    pub skids: u64,
    /// Rolling-loop voices whose last computed mix is audible — the
    /// same gauge for the rolling half.
    pub rolling: u64,
}

impl AudioReport {
    /// Any activity worth reporting (`aud=` stays absent otherwise).
    pub fn active(&self) -> bool {
        self.horns
            + self.voices
            + self.dropped
            + self.failed
            + self.loops
            + self.rigs
            + self.skids
            + self.rolling
            > 0
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

/// Deduplicated impact → one-shot voices (F07-B.3, spec req 4). Each
/// event's *vehicle* participants each earn a voice: the original
/// drives impact audio off the car's own impact callback, so a
/// two-car crash sounds once per car, each at its own impulse. A
/// remote participant belongs to its authority's client — the same
/// skip every F05 consumer applies. A participant that is not a
/// vehicle (a knocked prop meeting the world, banger-on-banger)
/// produces no voice — the authored table is car-audio data and there
/// is no striker to weigh.
///
/// Per striker: the *other* side's `dgBangerData` `AudioId` selects the
/// authored category (`ID` column; anything without a record falls to
/// the id-0 `WALL` catch-all — binding designed, UNK-25), the impulse
/// `severity × striker mass` picks the `min,max force` band (the same
/// estimate `impulse_estimate` feeds the knock pipeline; whether the
/// original weighs this quantity is unverified), `frequency` weights
/// the covering samples and the authored volume range draws the gain —
/// all through `mm2_game`'s [`pick_impact`] on the session's seeded
/// [`ImpactAudio`]. The voice is a spatial emitter at the impact
/// point for everyone but the local player (DSN-37 anchor), bounded
/// by [`MAX_IMPACT_VOICES`] so a pile-up counts drops instead of
/// stacking voices; a sub-floor force resolves no band and stays
/// authored-silent. Events on a stale generation are skipped; while
/// not `Playing` the reader drains without emitting (spark_fx's
/// contract — a buffered stale impact never flushes sound into a
/// pause or the next session).
#[allow(clippy::too_many_arguments)] // Bevy system — the borrows are the contract.
pub fn impact_voices(
    mut commands: Commands,
    mut reader: MessageReader<ImpactEvent>,
    session: Res<Session>,
    table: Option<ResMut<ImpactAudio>>,
    vfs: Option<Res<Mm2Vfs>>,
    bank: Option<ResMut<WaveBank>>,
    mut waves: ResMut<Assets<PcmAudio>>,
    mut report: ResMut<AudioReport>,
    identities: Query<(Entity, &ObjectIdentity, Option<&Player>)>,
    cars: Query<(Option<&Vehicle>, Option<&ComputedMass>, Option<&Mass>)>,
    bangers: Query<&Banger>,
    voices: Query<&AudioVoice>,
) {
    if !session.is_playing() {
        reader.read().for_each(drop);
        return;
    }
    let (Some(mut table), Some(vfs), Some(mut bank)) = (table, vfs, bank) else {
        // No authored table (or no session world yet): the stream is
        // still consumed next frame — an absent record degrades to
        // silence rather than fabricating a category.
        return;
    };
    // Disjoint field borrows (`&table.table` vs `&mut table.rng`) do
    // not split through `ResMut`'s Deref — reborrow the inner struct.
    let table = &mut *table;
    let generation = session.generation();
    let index: HashMap<ObjectId, (Entity, Option<PlayerControl>)> = identities
        .iter()
        .map(|(entity, id, player)| (id.0, (entity, player.map(|p| p.control))))
        .collect();
    let mut live = voices
        .iter()
        .filter(|v| v.kind == VoiceKind::Impact)
        .count();
    for event in reader.read() {
        if event.generation != generation {
            continue;
        }
        for side in 0..2 {
            let (me, other) = if side == 0 {
                (event.participants.0, event.participants.1)
            } else {
                (event.participants.1, event.participants.0)
            };
            let Some(&(entity, control)) = index.get(&me) else {
                continue;
            };
            if control == Some(PlayerControl::Remote) {
                continue;
            }
            let Ok((vehicle, computed, mass)) = cars.get(entity) else {
                continue;
            };
            let Some(vehicle) = vehicle else {
                continue;
            };
            // The struck side's authored selector — its banger record's
            // `AudioId`; world geometry and recordless bodies read the
            // id-0 catch-all.
            let audio_id = index
                .get(&other)
                .and_then(|(e, _)| bangers.get(*e).ok())
                .map(|b| b.def.audio_id)
                .unwrap_or(0);
            // The designed force quantity: approach speed × the
            // striker's resolved mass (computed → authored → config),
            // matching `impulse_estimate`'s contract. Each source is
            // checked before the fallback — a not-yet-computed
            // `Mass(0)` must fall through to the config mass, not
            // collapse the pick to a 1 kg touch.
            let valid = |m: f32| (m.is_finite() && m > 0.0).then_some(m);
            let striker_mass = computed
                .and_then(|m| valid(m.value()))
                .or_else(|| mass.and_then(|m| valid(m.0)))
                .or_else(|| valid(vehicle.config.mass))
                .unwrap_or(1.0);
            let force = event.severity * striker_mass;
            let Some(category) = impact_category(&table.table, audio_id) else {
                // The table cannot answer this selector at all — a
                // data failure, counted once per event side.
                report.failed += 1;
                warn!("audio: no impact category for AudioId {audio_id}");
                continue;
            };
            let Some(pick) = pick_impact(category, force, &mut table.rng) else {
                // No band covers the force — authored silence for a
                // sub-floor touch, not a failure.
                continue;
            };
            if live >= MAX_IMPACT_VOICES {
                report.dropped += 1;
                continue;
            }
            match bank.load(&vfs.0, &mut waves, &pick.sample.name) {
                Ok(handle) => {
                    // The local player's hits anchor the mix
                    // non-spatially like its engine rig (DSN-37);
                    // everyone else is a world emitter at the contact.
                    let spatial = control != Some(PlayerControl::Local);
                    commands.spawn((
                        AudioVoice {
                            kind: VoiceKind::Impact,
                        },
                        SessionEntity(generation),
                        Transform::from_translation(event.point),
                        AudioPlayer(handle),
                        PlaybackSettings {
                            mode: PlaybackMode::Despawn,
                            volume: Volume::Linear(pick.volume),
                            spatial,
                            spatial_scale: spatial.then(|| SpatialScale::new(ENGINE_SPATIAL_SCALE)),
                            ..Default::default()
                        },
                    ));
                    report.voices += 1;
                    report.impacts += 1;
                    live += 1;
                }
                Err(e) => {
                    report.failed += 1;
                    warn!("audio: {e}");
                }
            }
        }
    }
}

/// Build engine rigs on every `VehicleAudio` car: one
/// [`PlaybackMode::Loop`] voice per `Engine wave name` row that
/// resolves both a fade-window spec and a wave, parented to the car so
/// it despawns with it and rides its transform as the emitter.
/// Non-player rigs (race opponents — ambient traffic carries no
/// `VehicleAudio`) are spatial emitters heard through the camera
/// listener; the player's own rig stays non-spatial because the local
/// car anchors the mix (DSN-37). A row that cannot drive (no
/// fade-window schema) or cannot decode is counted and warned, never
/// silently skipped; cars past [`MAX_ENGINE_RIGS`] report once and
/// stay silent rather than flooding the mixer.
#[allow(clippy::too_many_arguments, clippy::type_complexity)] // Bevy system signature — the filtered query is the system's real input
pub fn engine_rigs(
    mut commands: Commands,
    session: Res<Session>,
    vfs: Option<Res<Mm2Vfs>>,
    bank: Option<ResMut<WaveBank>>,
    mut waves: ResMut<Assets<PcmAudio>>,
    mut report: ResMut<AudioReport>,
    cars: Query<(Entity, &VehicleAudio, Has<PlayerVehicle>), Without<EngineRig>>,
    rigs: Query<(), With<EngineRig>>,
) {
    if cars.is_empty() {
        return;
    }
    let (Some(vfs), Some(mut bank)) = (vfs, bank) else {
        // No mounted content or no session world yet — retry next
        // frame rather than stamping a half-built rig.
        return;
    };
    let mut live_rigs = rigs.iter().count();
    for (car, audio, player) in &cars {
        if live_rigs >= MAX_ENGINE_RIGS {
            report.dropped += 1;
            warn!("audio: engine rig bound reached, {car:?} stays silent");
            commands.entity(car).insert(EngineRig);
            continue;
        }
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
                            spatial: !player,
                            spatial_scale: (!player)
                                .then(|| SpatialScale::new(ENGINE_SPATIAL_SCALE)),
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
        live_rigs += 1;
        report.rigs += 1;
    }
}

/// Re-mix every engine loop from its parent vehicle's RPM and apply it
/// to whichever sink the device attached (plain for the player rig,
/// [`SpatialAudioSink`] for opponent emitters). Ungated by phase:
/// voices only exist inside a live session, `Paused` sinks are held by
/// [`sync_audio_pause`], and `Countdown`/`Results` legitimately keep
/// the engines sounding — the cars are live, just not drivable.
pub fn engine_drive(
    mut report: ResMut<AudioReport>,
    cars: Query<&VehicleState>,
    mut voices: Query<(
        &ChildOf,
        &mut EngineVoice,
        Option<&mut AudioSink>,
        Option<&mut SpatialAudioSink>,
    )>,
) {
    report.audible = 0;
    for (parent, mut voice, sink, spatial) in &mut voices {
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
        if let Some(mut sink) = spatial {
            sink.set_volume(Volume::Linear(voice.mix.volume));
            sink.set_speed(voice.mix.speed);
        }
    }
}

/// Fresh band slots for a committed skid entry — capped at
/// [`MAX_SKID_VOICES`], the overflow counted once per commit rather
/// than re-dropped every frame.
fn skid_slots(audio: &SurfaceAudio, idx: u16, report: &mut AudioReport) -> Vec<SkidSlot> {
    let bands = audio
        .table
        .surfaces
        .get(idx as usize)
        .map(|e| e.skids.len())
        .unwrap_or(0);
    let overflow = bands.saturating_sub(MAX_SKID_VOICES);
    if overflow > 0 {
        report.dropped += overflow as u64;
    }
    (0..bands.min(MAX_SKID_VOICES))
        .map(|_| SkidSlot::default())
        .collect()
}

/// Push a computed mix onto whichever sink the device attached — a
/// plain sink for the player's own rig, [`SpatialAudioSink`] for
/// everyone else's (the apply half of `engine_drive`'s contract).
fn push_mix(mix: EngineMix, sink: Option<Mut<AudioSink>>, spatial: Option<Mut<SpatialAudioSink>>) {
    if let Some(mut s) = sink {
        s.set_volume(Volume::Linear(mix.volume));
        s.set_speed(mix.speed);
    }
    if let Some(mut s) = spatial {
        s.set_volume(Volume::Linear(mix.volume));
        s.set_speed(mix.speed);
    }
}

/// Wheel contact → surface voices (F07-B.4, spec req 3). Every
/// `Vehicle` car resolves its grounded wheels against the session's
/// [`SurfaceTables`] — `SurfaceMaterial` → the material's authored
/// `sound` class → the [`SurfaceAudio`] row — and voices the loudest
/// result per role:
///
/// - **Skid**: per grounded wheel the designed [`tire_slippage`]
///   quantity (UNK-25 — tire-limit utilization saturating at one)
///   picks a `skid wave` band of that wheel's entry; `min speed`
///   units read `|vel_long|` instead. The loudest covering band wins.
///   A held brake at rest demands nothing (its force is
///   `vel_long`-scaled) and an airborne wheel has no contact — the
///   F07-AC03 legs stay silent by construction, not by a gate.
/// - **Rolling**: the loudest `surface wave` loop under a moving car
///   (`|forward_speed|` ≥ [`ROLL_MIN_SPEED`], so a parked car on grass
///   does not hum), mixed by the authored `max speed` window.
///
/// Band loops spawn lazily per committed entry and idle at volume 0
/// rather than churning attach/detach (the engine rig's policy); a
/// different winning entry rebuilds only after holding
/// [`SURFACE_DWELL`] frames. Non-player cars are spatial emitters at
/// the car's transform under [`ENGINE_SPATIAL_SCALE`]; the local
/// player's stay non-spatial (DSN-37). Cars past [`MAX_SURFACE_RIGS`]
/// report once and stay silent. Ungated by phase like `engine_drive`
/// — voices only exist inside a live session and `sync_audio_pause`
/// holds the sinks.
#[allow(clippy::too_many_arguments, clippy::type_complexity)] // Bevy system — the borrows are the contract.
pub fn surface_voices(
    mut commands: Commands,
    session: Res<Session>,
    surface_audio: Option<Res<SurfaceAudio>>,
    tables: Option<Res<SurfaceTables>>,
    vfs: Option<Res<Mm2Vfs>>,
    bank: Option<ResMut<WaveBank>>,
    mut waves: ResMut<Assets<PcmAudio>>,
    mut report: ResMut<AudioReport>,
    mut cars: Query<(
        Entity,
        &Vehicle,
        &VehicleState,
        Option<&mut SurfaceRig>,
        Has<PlayerVehicle>,
    )>,
    collider_surfaces: Query<&SurfaceMaterial>,
    mut voices: Query<(
        &mut SurfaceVoice,
        Option<&mut AudioSink>,
        Option<&mut SpatialAudioSink>,
    )>,
    rigs: Query<(), With<SurfaceRig>>,
) {
    report.skids = 0;
    report.rolling = 0;
    let (Some(audio), Some(tables), Some(vfs), Some(mut bank)) = (surface_audio, tables, vfs, bank)
    else {
        // No authored surface data (or no session world yet) — a wheel
        // cannot resolve a surface row, so nothing plays.
        return;
    };
    let generation = session.generation();
    let mut live_rigs = rigs.iter().count();
    for (car, vehicle, state, rig_slot, player) in &mut cars {
        // The loudest covering skid band and rolling loop across the
        // grounded wheels — `(sound index, band, gain)` / `(index, _)`.
        let mut skid_win: Option<(u16, usize, f32)> = None;
        let mut roll_win: Option<(u16, f32)> = None;
        let moving = state.forward_speed.abs() > ROLL_MIN_SPEED;
        for (i, w) in state.wheels.iter().enumerate() {
            if !w.grounded {
                continue;
            }
            let material = w
                .contact_entity
                .and_then(|e| collider_surfaces.get(e).ok())
                .copied()
                .unwrap_or_default();
            let Some(idx) = tables.sound_index(material) else {
                continue;
            };
            let (Some(spec), Some(entry)) = (
                audio.specs.get(idx as usize),
                audio.table.surfaces.get(idx as usize),
            ) else {
                continue;
            };
            if let Some(skid) = spec.skid {
                let q = match skid.unit {
                    SkidUnit::Slippage => {
                        let peak = vehicle
                            .config
                            .wheels
                            .get(i)
                            .and_then(|c| c.tires.as_ref())
                            .map_or(vehicle.config.tires.peak_slip_angle, |t| t.peak_slip_angle);
                        tire_slippage(w.traction_demand, w.slip_angle, peak)
                    }
                    SkidUnit::Speed => w.vel_long.abs(),
                };
                if let Some(pick) =
                    skid.pick(&entry.skids[..entry.skids.len().min(MAX_SKID_VOICES)], q)
                    && skid_win.is_none_or(|(.., g)| pick.volume > g)
                {
                    skid_win = Some((idx, pick.band, pick.volume));
                }
            }
            if moving && let Some(rolling) = spec.rolling {
                let gain = rolling.mix(state.forward_speed).volume;
                if roll_win.is_none_or(|(_, g)| gain > g) {
                    roll_win = Some((idx, gain));
                }
            }
        }
        if skid_win.is_none() && roll_win.is_none() && rig_slot.is_none() {
            // Never resolved a surface — no rig to silence either.
            continue;
        }
        let Some(mut rig) = rig_slot else {
            // First resolution — bound the session's rig count. A
            // muted marker keeps the refusal counted once.
            if live_rigs >= MAX_SURFACE_RIGS {
                report.dropped += 1;
                warn!("audio: surface rig bound reached, {car:?} stays silent");
                commands.entity(car).insert(SurfaceRig {
                    muted: true,
                    ..Default::default()
                });
            } else {
                live_rigs += 1;
                commands.entity(car).insert(SurfaceRig::default());
            }
            continue;
        };
        if rig.muted {
            continue;
        }

        // -- skid half: commit the winning entry (dwell-gated swap),
        //    lazily build the winning band's voice, mix every slot.
        match (rig.skid_entry, skid_win) {
            (_, None) => {}
            (None, Some((idx, ..))) => {
                rig.skid_entry = Some(idx);
                rig.skid_slots = skid_slots(&audio, idx, &mut report);
                rig.skid_pending = None;
            }
            (Some(c), Some((idx, ..))) if c == idx => rig.skid_pending = None,
            (Some(_), Some((idx, ..))) => {
                match &mut rig.skid_pending {
                    Some((p, n)) if *p == idx => *n += 1,
                    _ => rig.skid_pending = Some((idx, 1)),
                }
                if rig.skid_pending.is_some_and(|(_, n)| n >= SURFACE_DWELL) {
                    for slot in &mut rig.skid_slots {
                        if let Some(v) = slot.voice.take() {
                            commands.entity(v).despawn();
                        }
                    }
                    rig.skid_entry = Some(idx);
                    rig.skid_slots = skid_slots(&audio, idx, &mut report);
                    rig.skid_pending = None;
                }
            }
        }
        let winning = skid_win.filter(|(idx, ..)| Some(*idx) == rig.skid_entry);
        if let Some(idx) = rig.skid_entry {
            if let Some((_, band, ..)) = winning
                && let Some(slot) = rig.skid_slots.get_mut(band)
                && !slot.tried
            {
                slot.tried = true;
                let name = audio.table.surfaces[idx as usize].skids[band].name.clone();
                match bank.load(&vfs.0, &mut waves, &name) {
                    Ok(handle) => {
                        let v = commands
                            .spawn((
                                AudioVoice {
                                    kind: VoiceKind::Skid,
                                },
                                SurfaceVoice {
                                    role: SurfaceRole::Skid(band),
                                    mix: EngineMix {
                                        volume: 0.0,
                                        speed: 1.0,
                                    },
                                },
                                SessionEntity(generation),
                                ChildOf(car),
                                Transform::default(),
                                AudioPlayer(handle),
                                PlaybackSettings {
                                    mode: PlaybackMode::Loop,
                                    volume: Volume::Linear(0.0),
                                    spatial: !player,
                                    spatial_scale: (!player)
                                        .then(|| SpatialScale::new(ENGINE_SPATIAL_SCALE)),
                                    ..Default::default()
                                },
                            ))
                            .id();
                        slot.voice = Some(v);
                        report.voices += 1;
                    }
                    Err(e) => {
                        report.failed += 1;
                        warn!("audio: {e}");
                    }
                }
            }
            for (b, slot) in rig.skid_slots.iter().enumerate() {
                let target = winning
                    .filter(|(_, band, _)| *band == b)
                    .map(|(.., g)| g)
                    .unwrap_or(0.0);
                if let Some(v) = slot.voice
                    && let Ok((mut voice, sink, spatial)) = voices.get_mut(v)
                {
                    voice.mix = EngineMix {
                        volume: target,
                        speed: 1.0,
                    };
                    if target > 0.0 {
                        report.skids += 1;
                    }
                    push_mix(voice.mix, sink, spatial);
                }
            }
        }

        // -- rolling half: the same commit/dwell/lazy-voice shape.
        match (rig.rolling_entry, roll_win) {
            (_, None) => {}
            (None, Some((idx, _))) => {
                rig.rolling_entry = Some(idx);
                rig.rolling_tried = false;
                rig.rolling_pending = None;
            }
            (Some(c), Some((idx, _))) if c == idx => rig.rolling_pending = None,
            (Some(_), Some((idx, _))) => {
                match &mut rig.rolling_pending {
                    Some((p, n)) if *p == idx => *n += 1,
                    _ => rig.rolling_pending = Some((idx, 1)),
                }
                if rig.rolling_pending.is_some_and(|(_, n)| n >= SURFACE_DWELL) {
                    if let Some(v) = rig.rolling_voice.take() {
                        commands.entity(v).despawn();
                    }
                    rig.rolling_entry = Some(idx);
                    rig.rolling_tried = false;
                    rig.rolling_pending = None;
                }
            }
        }
        if let Some(idx) = rig.rolling_entry {
            let playing = roll_win.is_some_and(|(w, _)| w == idx);
            if playing && !rig.rolling_tried {
                rig.rolling_tried = true;
                let name = audio.table.surfaces[idx as usize].name.clone();
                match bank.load(&vfs.0, &mut waves, &name) {
                    Ok(handle) => {
                        let v = commands
                            .spawn((
                                AudioVoice {
                                    kind: VoiceKind::Rolling,
                                },
                                SurfaceVoice {
                                    role: SurfaceRole::Rolling,
                                    mix: EngineMix {
                                        volume: 0.0,
                                        speed: 1.0,
                                    },
                                },
                                SessionEntity(generation),
                                ChildOf(car),
                                Transform::default(),
                                AudioPlayer(handle),
                                PlaybackSettings {
                                    mode: PlaybackMode::Loop,
                                    volume: Volume::Linear(0.0),
                                    spatial: !player,
                                    spatial_scale: (!player)
                                        .then(|| SpatialScale::new(ENGINE_SPATIAL_SCALE)),
                                    ..Default::default()
                                },
                            ))
                            .id();
                        rig.rolling_voice = Some(v);
                        report.voices += 1;
                    }
                    Err(e) => {
                        report.failed += 1;
                        warn!("audio: {e}");
                    }
                }
            }
            if let Some(v) = rig.rolling_voice
                && let Ok((mut voice, sink, spatial)) = voices.get_mut(v)
            {
                voice.mix = if playing {
                    audio.specs[idx as usize].rolling.map_or(
                        EngineMix {
                            volume: 0.0,
                            speed: 1.0,
                        },
                        |r| r.mix(state.forward_speed),
                    )
                } else {
                    EngineMix {
                        volume: 0.0,
                        speed: 1.0,
                    }
                };
                if voice.mix.volume > 0.0 {
                    report.rolling += 1;
                }
                push_mix(voice.mix, sink, spatial);
            }
        }
    }
}
/// 5): `toggle_camera` flips `Camera::is_active` between the session's
/// chase and free cameras, and Bevy wants exactly one
/// [`SpatialListener`] — this inserts it on the active camera and
/// strips it from the inactive one, so a mode switch moves the ear
/// with the view rather than doubling listeners or leaving it on a
/// camera the session is about to despawn. `Camera3d`-filtered so a
/// menu `Camera2d` surviving a transition frame can't become the ear
/// (the same guard `apply_city_pvs` applies to source rooms). With no
/// active 3-D camera the field simply has no listener.
pub fn audio_listener(
    mut commands: Commands,
    cams: Query<(Entity, &Camera, Has<SpatialListener>), With<Camera3d>>,
) {
    for (entity, cam, listening) in &cams {
        match (cam.is_active, listening) {
            (true, false) => {
                commands.entity(entity).insert(SpatialListener::default());
            }
            (false, true) => {
                commands.entity(entity).remove::<SpatialListener>();
            }
            _ => {}
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
