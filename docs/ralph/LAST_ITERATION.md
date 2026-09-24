# Last iteration — F07-A.2: runtime audio voices (authored horn playback)

New-task iteration on `ralph/night` (baseline `e819717`, the reviewed
F07-A.1 audio-format leg — external gates + incremental review passed
with no blocking findings). Selected F07-A.2 — the second half of the
F07-A task text: voice lifecycle + the first real runtime consumer.
With every `aud/**` file classified and decoded, the highest-value
ready leg was to wire authored per-vehicle audio into the session and
play it: the horn (ENTER, documented default CTL-1) is the smallest
honest consumer — one sample ref, one press edge, no event wiring.
Engine loops/impacts/surfaces/sirens remain F07-B+ under UNK-25.

## What landed

- `mm2_formats::wav::lookup_stem` — shared sample-name→stem
  normalization (directory, `.wav` and `.NNk` rate suffix stripped,
  case-folded); `mm2-inspect audio` now calls it instead of a private
  copy. `CardataIssue` gained `Display` so diagnostics propagate as
  warnings.
- `mm2_content::VehicleDef.audio: Option<CarAudio>` — the authored
  `aud/cardata/{player,opponent}/<id>.csv` now loads with the vehicle
  (side-dispatched through `load_vehicle`/`load_opponent`), its source
  joins `VehicleDef.sources`, diagnostics/validation findings become
  warnings, and absent or malformed audio leaves `None` — vehicle
  loading never sinks on audio data.
- `mm2_game::audio::VehicleAudio` — the domain component carrying the
  authored `CarAudio` verbatim; `DevOverrides.horn` is the dev evidence
  trigger (fires once at `Playing`, presentation-only, outside
  `record_eligibility`).
- `mm2_app::audio` — the voice layer, all session-scoped:
  - `PcmAudio` Bevy asset + `Decodable` source: decodes through the
    bounded `mm2_formats::wav` parser (16-bit PCM → normalized f32),
    rejecting non-PCM and oversized `data` chunks *before* sample
    materialization. `bevy_audio` is enabled; Bevy's own WAV loader is
    unused — no direct rodio dep (Bevy re-exports the traits).
  - `WaveBank` — one census of `aud/**/*.wav` per session, grouped by
    `lookup_stem`, `aud22` preferred over `aud11` on a tie (designed;
    AUD-2 leaves the original pick unverified), deterministic lexical
    ordering; missing stems and malformed waves are reported, not
    silently dropped.
  - Voice systems — `AudioVoice`/`VoiceKind`/`HornRequest`/`AudioReport`:
    one ENTER press fires one one-shot clip (`PlaybackMode::Despawn`
    self-cleans at end-of-clip; hold/retrigger semantics unverified so
    single-shot-per-press is the designed reading — DSN-35). Bounded at
    `MAX_VOICES` 8, excess presses counted and dropped, sinks pause with
    `SessionPhase::Paused`, and voices despawn with session teardown.
  - `aud=<presses>h/<voices>v/<sinks>s` smoke field — demand vs spawned
    vs mixer-attached, headless-safe (a headless run reports `0s`
    sinks, honestly).
- `--horn` CLI flag → `DevOverrides.horn`; headless + windowed smoke
  paths both register the audio systems.

## Retail evidence

```
headless:  mm2 --mm2-path retail --city sf --headless --horn --frames 300
           → smoke=headless-physics ... aud=1h/1v/0s  (voice spawned,
             no output device — no sink, honest)
windowed:  mm2 --mm2-path retail --city sf --horn --frames 120
           → smoke=visual ... aud=1h/1v/1s  (voice reached the real
             mixer/sink on this machine)
```

vpbug's authored `VWHORN` resolved to `aud/aud22/horns/vwhorn.22k.wav`
through the production VFS and decoded through the bounded parser — no
fabricated binding.

## Tests

- `mm2_formats::wav` — `lookup_stem` suffix/directory rules.
- `mm2_content::assemble` — cardata loads per-side; absent/wrong-grammar
  audio is a warning, not a load failure.
- `mm2_app` (`tests/audio.rs`, 9 tests) — aud22 preference, PCM decode +
  non-PCM/oversized rejection, voice bound drops excess presses, ENTER
  spawns the authored voice (phase-gated to `Playing`), no authored
  audio → no voice, missing stem + malformed wave surface in
  `AudioReport`, teardown despawns live voices, `--horn` fires exactly
  once.

## Gates

`cargo fmt --all -- --check` clean; `cargo clippy --locked --workspace
--all-targets --all-features -- -D warnings` clean; `cargo test
--locked --workspace` green — 68 suites, 1037 tests, 0 failures.
`Cargo.lock` unchanged (all `bevy_audio` deps were already locked).

## Not done / open

- Engine loops, impacts, surfaces, sirens, ambient emitters — F07-B+
  scope; the `Engine wave name` fade-window driver quantity and every
  other runtime semantic stay open under UNK-25.
- Horn hold/retrigger behavior unverified — one clip per press is the
  designed reading (DSN-35). Horn-row `flags` word unbound.
- Opponent/ambient vehicles carry no voices yet; `VehicleAudio` is
  attached but nothing requests their horns.
- `spchdata`/`creaturedata` + DirectMusic remain F08.
- The sink-attach run proves the decode→mixer path on this machine; no
  audible A/B against the original exists or is claimed.
