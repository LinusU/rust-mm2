# Last iteration — F07-B.7: authored siren programs

Iteration 44 on `ralph/night` (baseline `27d284d`, F07-B.6 ambient
engine voices — external verify + review green). One coherent slice
of the F07-B remainder: the authored `*policesiren.csv` programs now
drive a bounded looping siren on `flags & 4` cars, toggled by the
horn control.

## Task selection

No failing gate or review finding to repair. TASKS.json's F07-B row
lists siren programs as a ready sub-slice: the `SirenProgram`/
`SirenSample`/`SirenStep` grammar already parsed (F07-A.1), the
voice/bank/report machinery exists (F07-A.2/B.1–B.6), and the retail
evidence leg is concrete — `vpcop` alone authors `flags=4` on its
horn row and the exe names `sfpolicesiren`/`londonpolicesiren`/
`policesiren` + `sirens\%s` (AUD-10). Sustained-scrape semantics and
the weather→surface-variant binding remain.

## What landed

- `mm2_formats::cardata` — doc corrections only: `CarAudio.flags` is
  a per-vehicle bitmask, not always 0 (retail census: `vpbus` 1,
  `vpcentury` 2, `vpcop` 4, `vpddbus`/`vpsemi` 8 — identical on both
  sides); `SirenStep.next_index` is a *program sample* position
  (verified: london's `0↔1` ping-pong and sf's cross-sample chain
  only make sense that way).
- `mm2_game::audio` — `SIREN_FLAG` (bit 4), `SirenSpec::from_program`
  (explosion binding carried verbatim with no consumer; absent/
  non-finite/negative authored volume → 1.0 like the horn; empty
  program → `None`), and `SirenPlayback`: enters at sample 0, picks
  one step per entry through the session `NavRng` (multi-step rows =
  authored dwell/branch variety — designed), dwells `play time`
  seconds, follows `next index` to the next *sample*. Negative or
  out-of-range targets, empty target samples and non-finite `dt` end
  it; banked overshoot carries into the next step; `MAX_SIREN_HOPS`
  16 bounds a malformed zero-time chain. All three retail programs
  cycle forever — `End` is a malformed-data guard.
- `mm2_app::audio` — `SirenAudio` session resource (player spec keyed
  by the session city's PSDL stem — `sf`/`london` →
  `sfpolicesiren`/`londonpolicesiren`, the naming the exe's strings
  imply; opponent spec from the shared `policesiren.csv`; absent or
  malformed warns once and leaves that side programless — never a
  fabricated substitute). `WaveBank` gains a `sirens/` subtree index
  + `load_siren` (the `sirens\%s` scope the exe names; global stem
  fallback). `siren_toggle`: each `HornRequest` press on a flagged
  `PlayerVehicle` toggles its `Siren` — press-to-toggle is the
  designed reading (the flag lives on the horn row and the exe
  carries no other siren binding; hold-vs-toggle vs pursuit state is
  unverified, UNK-25); flagged presses never play the horn sample
  (`horn_voices` skips them), unflagged cars keep the ordinary horn,
  `MAX_SIRENS` 8 concurrent programs with `+Nd` drops. `siren_drive`:
  the program clock is `Session::tick` × the fixed timestep — pause/
  countdown freeze it like `sync_audio_pause` holds the sinks — one
  `PlaybackMode::Loop` `AudioVoice{Siren}` child of the car per
  current sample, respawned on each authored `Switch`, non-spatial
  for the local player / spatial for others (DSN-37), `SessionEntity`
  + child-of-car teardown; sentinel names are authored silence,
  unresolvable stems warn once per activation while the program
  keeps walking. `AudioReport` gains `sirens` (voices spawned) +
  `siren_live` (programs live); `aud=` gains `/<n>w/<n>y` when
  nonzero.
- `mm2_app::session`/`main`/`smoke` — `SirenAudio` inserted at load,
  removed at teardown; `(siren_toggle, siren_drive).chain().after(
  session::drive_session)` in both schedules.

## Docs

`docs/original-rules.md` — AUD-10 (flag census, exe strings, wave
placement, in-range chain finding; AUD-7's `Explosion wave name`
corrected to `Explosion sample`), DSN-42 (the designed toggle/entry/
step-pick/voice/bound policies), UNK-25 (bit-4 clause bound, the
remaining siren unknowns enumerated). `docs/research/audio.md` —
F07-B.7 runtime paragraph, the siren section records the flag census
+ exe naming + `FIRETRUCKSIREN` observation, open semantics updated.
`crates/mm2_formats::cardata` — `flags`/`next index` doc comments.

## Evidence

Synthetic tests (device-independent): +6 `mm2_game` unit tests
(retail sf/london/opponent program resolution + explosion-only
`None`, authored chain walk + banked overshoot + london ping-pong,
opponent closed-cycle soak, out-of-range/negative `End`, zero-time
`MAX_SIREN_HOPS` + non-finite `dt`, flag detection) — fixtures are
verbatim retail bytes. +12 `mm2_app` integration tests (program
loading by city/role, toggle on/off + re-activation, authored switch
respawns the voice on the session tick, frozen-tick hold, missing
wave warns once per activation while the program walks, sentinel
silence, flagged press with no program → `failed`, malformed table →
no substitute, opponent spatial vs player non-spatial, 8-program
bound, teardown sweep, unflagged car keeps the horn). Audio suite
now 64 tests green.

Retail (`fnv1a64:e91e6cd4b2ae30d9`, `--car vpcop --horn --headless`):

- sf `--frames 1500` (ticks=3000 ≈ 25 s): `aud=1h/67v/0s/4l/3a/1r/
  9i/8c/1k/0g/26e/16n/17w/1y` — one press activated the program;
  `17w` = 17 loop voices as the authored sf chain walked its 0.15–
  5.25 s steps across all four samples; `1y` live at record.
- london `--frames 1500`: `aud=1h/42v/0s/4l/3a/1r/5i/8c/1k/0g/20e/
  16n/2w/1y` — the authored 15 s ping-pong switched exactly once in
  25 s (enter `siren_london` → `siren_london2`), matching the
  program's authored dwell.
- Negative leg, unflagged car: sf `--car vpbug --horn --frames 300`
  → `aud=1h/29v/0s/4l/4a/1r/3c/1k/0g/18e/16n` — the press played the
  ordinary `VWHORN` voice; `w`/`y` stay absent (activity-gated).

`0s` honestly reports no output device headless; no audible A/B
against the original was performed — that remains F07-AC05 scope.

## Classification

A **designed/inferred runtime policy** over verified authored data:
the files, the flag census, the exe naming and the in-range chain
shape are retail-verified (AUD-10); press-to-toggle on the horn
control, sample-0 entry, per-entry random step pick, loop-per-sample
voices and the 8-program bound are designed readings recorded under
DSN-42/UNK-25. The original's trigger, step-selection rule,
`Explosion sample` consumer and termination semantics stay open.

## Remaining open items

- F07-B continues: sustained-scrape semantics (AC04 second leg),
  weather→{dry,wet,ice} surface-variant binding, AC02 scripted
  drive-sequence evidence, AC05 audible capture.
- Siren unknowns now under UNK-25: trigger semantics, step pick,
  `Explosion sample` consumer, program termination, `flags` bits
  1/2/8 (vpbus/vpcentury/vpddbus/vpsemi).
- Police AI that would activate opponent sirens is F20 scope; the
  opponent program is loaded and the drive handles `!PlayerVehicle`
  cars, but nothing spawns a flagged ambient car yet.
- Headless `0s` reports no output device honestly; no windowed
  sink-attach leg was run this iteration.
