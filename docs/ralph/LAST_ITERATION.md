# Last iteration — F07-B.9: scripted drive-sequence evidence (AC02)

Iteration 46 on `ralph/night` (baseline `93d79cc`, F07-B.8
weather-bound surface tables — external verify + review green). One
coherent slice of the F07-B remainder: a reproducible scripted
driving sequence through the production input/audio paths, producing
the drivetrain/audio evidence F07-AC02 asks for.

## Task selection

No failing gate or review finding to repair. F07-B's remaining
candidates were sustained-scrape semantics (AC04's second leg — still
no authored scrape sample to bind, entirely designed policy), the
AC05 audible capture (needs a real output device/windowed session —
not available headless), and the AC02 scripted sequence. AC02 won:
it produces concrete drivetrain→audio evidence through the real
`VehicleInput`/`VehicleState`/`EngineMix`/`AudioReport` pipeline on
both the dev world and a retail city, device-independently.

## What landed

- `mm2_app::sequence` (new) — `SequenceDrive` resource + the
  `sequence_drive` system: a staged `idle → accelerate → coast →
  brake → reverse` input program written through the production
  `VehicleInput` component, the same resource-gated driver pattern
  `ScriptedDrive`/`ParkedDrive` use. Fixed stage timers (idle 3 s,
  accel 7 s, coast 4 s, reverse 5 s); the brake stage ends when the
  car reaches the drivetrain's own `≤ 0.25 m/s` nearly-stopped band
  (bounded 10 s — a penned car still reaches reverse rather than
  stalling the program). One `SeqSample` banks at every stage
  boundary: RPM, gear, direction, forward speed, the loudest engine
  loop's computed `EngineMix` volume/pitch (the same fields
  `engine_drive` writes — headless runs carry them on the component,
  no sink needed), and the stage's clutch one-shot delta off
  `AudioReport::clutch`. The AC's "shift" leg has no stage of its own
  — the gearbox is automatic, so committed `(gear, direction)`
  changes land inside accelerate/coast and the brake→reverse hand-off
  and show as the per-stage `+Nc` deltas. Stage timers run only while
  `Playing` and not countdown-locked (the same gate `scripted_drive`
  honors); a session-generation change resets the program and clears
  its samples so a restart never mixes two sessions.
- `mm2_app::smoke` — `Driver::Sequence` variant; the record gains
  ` seq=` with comma-joined stage rows
  (`acc:4840r/F4/24.8m/0.90v/1.32p+4c`). Runs without `--seq`, or a
  program that banked no samples, emit a bit-identical record.
- `mm2` CLI — `--seq` flag (conflicts with `--bot`/`--parked`), wired
  in the windowed schedule too; `sequence_drive` sits in its own
  `add_systems` call ordered after `vehicle_input` and
  `clutch_voices` (so a boundary sample attributes a same-frame shift
  to the stage that produced it) — the main Update tuple was at
  Bevy's system-config size limit.
- `README.md` — the `--seq` evidence command documented alongside
  `--bot`/`--parked`.

## Docs

`docs/research/audio.md` — F07-B.9 runtime paragraph with both retail
`seq=` records. `docs/ralph/PLAN.md` updated. No `original-rules.md`
row: the program is an evidence driver, not a game rule — classified
implementation/evidence choice with no original-behavior claim.

## Evidence

Synthetic tests (device-independent, `tests/sequence.rs`, +4):

- countdown lock holds the program (no input writes, no stage burn);
- a mid-run session restart resets stage/samples (generation-scoped);
- stage boundaries bank exact drivetrain mix and per-stage clutch
  deltas (component-level attribution through a synthetic engine
  rig + `WaveBank`);
- the program drives the real dev-world physics end-to-end —
  settle → upshift run → coast-down → stop → reverse, samples banked
  in order.

Retail (`fnv1a64:e91e6cd4b2ae30d9`, vpbug, headless):

- dev world `--seq --frames 2200`:
  `seq=idle:750r/F0/-0.0m/0.82v/0.97p,acc:4840r/F4/24.8m/0.90v/1.32p+4c,
  coast:3324r/F3/12.3m/0.90v/1.07p+1c,brake:1272r/R0/0.2m/0.83v/1.07p+3c,
  rev:5817r/R0/-13.4m/0.90v/1.48p` — idle band, four upshifts on the
  throttle run, coast downshift, the brake→reverse hand-off voicing
  the direction change, then a held reverse band at −13.4 m/s.
- sf `--seq --frames 2400`:
  `seq=idle:750r/F0/0.4m/0.82v/0.97p,acc:4897r/F5/34.6m/0.90v/1.32p+5c,
  coast:750r/F0/0.2m/0.82v/0.97p+3c,brake:750r/F0/0.2m/0.82v/0.97p,
  rev:5814r/R0/-13.5m/0.90v/1.48p` — five upshifts on a real-city
  throttle run (the car met props mid-coast — honest staged evidence
  on authored geometry, impacts counted separately in the record).

`aud=` reports the clutch voices under the bound (`8c` + `+Nd` honest
bound-refusals on a headless run, the same semantics B.5 disclosed).

## Gates

`cargo fmt --all -- --check` clean; `cargo clippy --workspace
--all-targets --all-features -- -D warnings` clean; `cargo test
--workspace` — 69 test-result summaries, 0 failures (sequence suite
4/4).

## Classification

Implementation/evidence choice end to end: the stage table, timer
bounds, stopped-speed hand-off (mirroring the drivetrain's own 0.25
m/s edge) and the sample schema are ours — the program demonstrates
the rig answers the drivetrain; it claims nothing about the
original's audio beyond the already-ledgered designed readings
(DSN-36/DSN-40). Headless `0s` reports no output device; no audible
capture was performed — F07-AC05 stays open.

## Remaining open items

- F07-B continues: sustained-scrape semantics (AC04's second leg) and
  the AC05 audible/offline-mix capture. AC02 now has staged retail
  evidence but stays candidate until external review checks the diff.
- UNK-25 unchanged: surface selection state, `%s` stem content,
  `Tunnel sound index`, divisor-schema rolling formula, siren trigger
  semantics.
- No windowed/sink-attach leg was run this iteration.
