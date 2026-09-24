# Last iteration — F07-B.2: opponent engine rigs + spatial listener

Recovery iteration on `ralph/night` (baseline `280edb9`, the F07-B.1
engine-loop leg). Two consecutive runs were marked failed for
`dirty_candidate_commit_explicit_task_files_before_verification`:
iteration 36 committed F07-B.1 but left the F07-B.2 work below
uncommitted in the tree, and iteration 37 reviewed the diff, ran the
gates and re-verified the retail leg but exited before creating the
commit. This iteration re-ran the gates and the retail leg on the
unchanged diff (same results) and committed it as one coherent slice
— no implementation repair was needed. Both F07-B.1 and F07-B.2 are
candidates pending external check.

## What landed

- `mm2_app::opponents` — `spawn_opponents` inserts `VehicleAudio` from
  each roster car's opponent-side cardata (`def.audio`, resolved by
  `load_opponent`; same absence policy as the player: no record, no
  component).
- `mm2_app::audio::engine_rigs` — builds rigs on every `VehicleAudio`
  car, not just `PlayerVehicle`, bounded `MAX_ENGINE_RIGS` 16 per
  session; cars past the bound are `EngineRig`-stamped, counted once
  in `AudioReport.dropped`, and stay silent rather than churning.
  Non-player loops spawn `PlaybackSettings::spatial` with
  `SpatialScale::new(ENGINE_SPATIAL_SCALE)` 0.25 — Bevy/rodio spatial
  is inverse-square attenuation plus L/R ear pan (a 4 m opponent reads
  ~full authored volume, a 5–15 m pack clearly audible, a 50 m
  straggler near silence). The player's rig stays non-spatial: the
  local car anchors the mix (designed policy, DSN-37).
- `mm2_app::audio::audio_listener` — keeps exactly one
  `SpatialListener` on the `is_active` `Camera3d`; a chase↔free toggle
  moves the ear the same frame, and the `Camera3d` filter keeps a
  transition-frame `Camera2d` from becoming the listener (same guard
  `apply_city_pvs` applies to source rooms).
- `engine_drive` applies the computed mix to `AudioSink` and
  `SpatialAudioSink` alike; `count_sinks`/`sync_audio_pause` already
  handle both sink kinds. `aud=` gains `<rigs>r`.
- Rig and listener commands order `.after(session::drive_session)` in
  both schedules — a mid-update `despawn_session_entities` flush must
  not have rig/listener inserts queue on dead cars/cameras (the smoke
  restart test surfaced the race).
- Docs: DSN-37 in `docs/original-rules.md`, UNK-25 narrowed (the
  original's spatial/attenuation model and listener placement stay
  unrecovered), `docs/research/audio.md` runtime note.

Ambient traffic carries no `VehicleAudio` (different cardata grammar —
`AmbientEngine`) and stays silent; that is a deliberate bound, not a
gap in this slice.

## Retail evidence

```
headless:  mm2 --mm2-path retail --event circuit:0 --bot --headless
           --frames 3000
           → status=pass, aud=0h/18v/0s/18l/17a/8r
             (8 rigs = player + 7 opponents, 18 loops, 17 audible
             mid-drive, honest 0 sinks — no device headless)
```

Re-run this iteration on `fnv1a64:e91e6cd4b2ae30d9` — same record as
the pre-commit run. Opponent cardata resolves through the production
VFS (`aud/cardata/opponent/*.csv`); no fabricated bindings.

## Tests

`mm2_app` (`tests/audio.rs`, +4 → 19 total) — opponent rig voices are
`spatial`+scaled while the player's stay non-spatial; an opponent's
loops mix off its own `VehicleState.rpm` while the player idles;
`MAX_ENGINE_RIGS` caps silent cars with a single counted report; the
listener migrates between cameras on `is_active` flips.

## Gates

`cargo fmt --all -- --check` clean; `cargo clippy --locked --workspace
--all-targets --all-features -- -D warnings` clean; `cargo test
--locked --workspace` green — 68 suites, 1053 tests, 0 failures.
`Cargo.lock` unchanged.

## Not done / open

- F07-B continues: impact/scrape voices off the deduplicated
  `ImpactEvent` stream (`default_impacts` force→sample binding
  unknown), surface skid/rolling bands (two schemas disagree on the
  trigger column), ambient-traffic engines (`AmbientEngine` grammar),
  stationary-brake/airborne-wheel negatives, clutch sample trigger,
  siren programs.
- The spatial model is Bevy/rodio's, scaled by a designed constant —
  the original's attenuation curve, distance units and listener
  placement are unverified under UNK-25 (DSN-37 discloses this).
- No audible A/B against the original; headless `0s` and windowed
  sink-attach prove the request→voice→mixer path on this machine only
  (F07-C scope).
