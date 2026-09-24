# Last iteration — F07-B.5: authored clutch one-shots on drivetrain changes

Iteration 41 on `ralph/night` (baseline `05a7e99`, the F07-B.4
surface-voices leg — externally checked clean). Selected the next F07-B
remainder slice: wire the parsed-but-unused `clutch wave name`/`clutch
volume` cardata binding to the sim's committed gear/direction changes
(spec req 2's "shifts and reverse" leg; AC02's shift/reverse event leg).
F07 is *not* complete — ambient engines, sirens, scrape semantics and
audible capture remain open.

## What landed

- `mm2_vehicle` — `DriveDirection` joins the crate-root re-exports (it
  was already the public field type of `VehicleState::direction`; the
  watch needs to compare it).
- `mm2_app::audio` — `GearWatch` component (the last observed
  `(gear, direction)` per car — the trigger's dedup), `clutch_voices`
  after `session::drive_session` in both schedules, `VoiceKind::Clutch`,
  `MAX_CLUTCH_VOICES` 8, `AudioReport::clutch`. Semantics: first sight
  inserts the watch silently (a spawned car did not "shift into" its
  initial gear); a differing committed pair plays the authored clutch
  sample at its authored volume — retail cars bind `REVERSE` (~0.93),
  the trucks `TRUCKGEARSHIFT` (0.87), dumped across the whole
  player-side roster this iteration. Which transitions the original
  voices is unverified (UNK-25): the designed trigger is one one-shot
  per committed change — a multi-gear selector jump is a single
  actuation, and the watch updates while not `Playing` so a
  countdown/reset change never flushes as a stale clunk
  (`impact_voices`' drain contract applied to state). Remote
  participants are watched but never voiced; sentinel/empty names are
  authored silence (no `failed`); an unresolvable stem counts `failed`
  per change (per-event semantics like horn presses). Voices are
  `PlaybackMode::Despawn` children of the car (ride its transform,
  sweep with the session), spatial for non-local cars / non-spatial
  for the local player (DSN-37). `horn_volume` renamed
  `authored_volume` — both bindings share the sanitizer.
- `mm2_app::smoke` — the `aud=` record gains `/{n}c` clutch voices,
  activity-gated like `/Ni`.

## Tests

`mm2_app` (`tests/audio.rs`, +8 → 44 total) — gear change voices the
authored wave at authored volume (`Despawn`, child of car,
non-spatial local / spatial AI, session stamp); first sight + held
gear stay silent; 0→3 multi-jump is one actuation; Forward↔Reverse
flips voice each way; remote car shifts silently (watch still
advances); `NOSOUND` clutch = authored silence, unresolvable stem =
`failed` per change; `MAX_CLUTCH_VOICES` caps a 10-car same-frame
shift burst at 8 (+2 dropped); a `Paused` shift updates the watch
without voicing and never flushes stale; teardown sweeps the voices.

## Gates

`cargo fmt --all -- --check` clean; `cargo clippy --locked --workspace
--all-targets --all-features -- -D warnings` clean; `cargo test
--locked --workspace` green — 68 suites, 1089 tests, 0 failures
(+8 over B.4's 1081). `Cargo.lock` unchanged.

## Retail evidence

```
headless:  mm2 --mm2-path retail --event circuit:0 --city london
           --bot --headless --frames 3000
           → status=pass, impacts=212,
             aud=0h/78v/0s/18l/17a/8r/12i/8c/8k/1g+438d
```

`78v` = 18 engine loops + 12 impacts + 8 clutch + 40 surface; `8c`
clutch one-shots spawned — one per car in the 8-car field, `REVERSE`
resolved for every roster car (vpbug + 7× vpcoop), zero `+x` failures.
The `+d` growth (125 → 438) is honest bound accounting, not a
regression: headless `PlaybackMode::Despawn` voices never release, so
after the 8-voice bound saturates every later committed shift counts
one drop — the same semantics the impact bound's +125 already
reported. Headless `0s` stays honest: no output device attaches.

## Not done / open

- F07-B continues: ambient-traffic engines (`AmbientEngine` grammar),
  siren programs (`SirenProgram`), sustained-scrape semantics
  (AC04's second leg), AC02's scripted drive sequence, AC05 audible
  capture, weather→{dry,wet,ice} surface-variant binding.
- Designed readings under UNK-25: the `(gear, direction)`-change
  clutch trigger (which transitions the original voices, whether
  multi-gear jumps multi-voice), `MAX_CLUTCH_VOICES` 8.
- No audible A/B against the original; headless `0s` reports no output
  device honestly (F07-C scope).
