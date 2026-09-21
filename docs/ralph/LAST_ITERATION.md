# Last implementation iteration

- Task ID and title: F15-A.2 — opponent spawn/drive: the authored
  `[Opponent]` lineup spawned as real AI participants driving their own
  `.opp` routes through the shared `VehicleInput` → physics →
  `advance_race` path.
- Starting commit: `7328362d3887d1822ac92e1251132d40c11c18f9`
  (externally checked F15-A.1; branch `ralph/night`).
- Why this slice: F15-A.1 landed the roster contract; this is its
  named remaining half — spawn opponent entities into valid start
  slots and drive the authored routes through the production vehicle
  sim. The F13-B/F14-B remainders block on real opponent participants.
- Retail install: `/Users/linus/coding/rust-mm2/retail`
  (`fnv1a64:e91e6cd4b2ae30d9`).

## What changed

- **`mm2_content::load_opponent`** (`assemble.rs`): each roster
  vehicle loads with its authored `tune/vehicle/<id>_opp.vehcarsim`
  merged over the base tune, not substituted for it. Retail evidence
  forced this: all 23 `_opp` files are *sparse overrides* — authored
  values differ (inertia box, drivetrain, horsepower, top speed) but
  most omit fields the base file carries, and several
  (`vp4x4_opp`, `vpbus_opp`, `vpcab_opp`…) author transmission data in
  an alternate schema (`NumGears`/`GearRatios`/`UpshiftRPM`/
  `DownshiftRPM`/`DownshiftBias`) the player files' `ManualNumGears`/
  `Low`/`High` band schema does not use. The first retail run failed
  all six wired `vpbug` opponents on `missing required field
  "TireDragCoefLong"` before the merge existed. Merge mechanics live
  in `mm2_formats::tune::TuneBlock::merge_overlay` (generic AST
  overlay, recursive); the policy and docs stay in `assemble.rs`.
  `_opp`-only fields our schema doesn't model surface through the
  existing unrecognised-field warnings — preserved diagnostics, not
  silent drops. Missing variant → base tune (unchanged fallback);
  every other dependency (`.info`/`.pkg`/`.bnd`/`.mtx`/`.asnode`/
  `.vehtrailer`) stays the vehicle's own.
- **`mm2_app::opponents`** (new): `OpponentDriver` component (authored
  `OpponentSpec` verbatim + chase index + `ScriptedBot` recovery
  state); `spawn_opponents` — one session-owned entity per authored
  roster entry with its own `VehicleDef` (opponent tuning preferred →
  per-vehicle character, not player clones), minted `ObjectId`/
  `PlayerId`, `PlayerControl::Ai`, the session's authority role,
  `DamageSignals`, `RaceProgress` on the shared `RaceDefinition`, the
  same `vehicle_bundle` + model path as the player. Roster issues log
  at load; a vehicle that fails to load warns and skips only its
  authored slot; a dead `.opp` ref spawns and holds still. Trailer
  rigs spawn without the trailer (no retail roster wires a hauler).
- **`opponent_drive`** (Update in `main.rs` + `smoke.rs`): gates on
  `session.is_playing()`, the countdown's `input_locked`, and the
  participant's own `RaceProgress` state; `route_target` advances
  past reached anchors (reach radius or the passed-the-plane test —
  no U-turns back), wraps closed routes (retail circuit `.opp`s
  close onto their start), and bounds retries on degenerate routes;
  `scripted_input` — the same normalized-input control law the
  scripted evidence driver uses — produces steer/throttle/brake with
  its bounded reverse-and-turn stuck recovery. Route missing or
  complete → zeroed input (open-route completion means coast; the
  race progress lives in checkpoints, not the polyline).
- **`spawn_pose`** (provisional — UNK-16/17): authored `_strtpnts`
  slot `index+1` when the event ships a grid (slot 0 is the player by
  convention), else the route's first point, else a designed stagger
  behind the player; facing always from the route's first leg, since
  the `a` columns' conventions are unverified. Same hull-clearance
  lift as the player spawn.
- **Wiring**: `EventSetup.roster` carries the built roster (build
  failure degrades to empty with a warning — the race still runs);
  `load_session_world` spawns opponents before `RaceState` so
  `advance_race` owns countdown/release for every participant; smoke
  record gains `opp={resolved}/{spawned}` only when a roster exists
  (records without a roster stay bit-identical).
- **Ledger**: RACE-13 new (`_opp` sparse-override shape + alternate
  Trans schema, measured on all 23 retail files); UNK-11 updated
  (`.opp` columns measured all-zero → polyline treatment stated;
  `_opp` consumption noted as designed merge).

## Tests

- `mm2_app/tests/opponents.rs` — +11 through `load_session_world` →
  `advance_race` on a synthetic install (PKG3 geometry + ASCII `.bnd`
  + base/`_opp` tunes + event/aimap/opp records): distinct entities
  spawn with `PlayerControl::Ai`/session owner/`RaceProgress`;
  unloadable vehicle skips only its slot; dead route ref spawns and
  holds still; countdown locks inputs; `route_target` advance, skip-
  passed-point (incoming-leg direction), open-route completion,
  closed-route wrap, degenerate-route bound; `spawn_pose` grid→
  anchor→stagger preference; two opponents drive their routes and
  resolve `Finished` through shared validation; restart despawns and
  respawns the lineup under the new generation.
- `mm2_content` merge behavior is exercised by the app suite's
  `_opp`-variant fixture; the `TuneBlock::merge_overlay` mechanics are
  covered transitively (`_opp` tune omits no fields the app tests
  depend on — the retail run below is the load-bearing evidence).

## Commands actually run and results

- `cargo fmt --all -- --check` PASS (after `cargo fmt --all`).
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` PASS.
- `cargo test --locked --workspace` — all groups, 0 failures
  (incl. all 11 new opponent tests).
- `cargo test -p mm2_app --test opponents` — 11/11 ok.
- Retail `sf checkpoint:0 --headless --bot --frames 1800` — first run
  *before* the merge: all 6 wired `vpbug` opponents failed to load
  (`missing required field "TireDragCoefLong"`), `opp=` absent.
  After the merge: `opponent roster spawned opponents=6`,
  `pos=5/7` mid-race — roster issues surface in-run (`race0` 6-wired-
  vs-7-authored count mismatch + `race0-a-6.opp` unreferenced).
- Retail `sf checkpoint:0 --headless --bot --frames 5400`:
  `opp=3/6` — three opponents resolved `Finished` through shared
  `advance_race` validation; `results=4`, `outcome=finished
  place=4`, `pos=4/7`. Smoke `status=fail`/`fell through the world`
  is the scripted bot's pre-existing course limitation (present on
  the same run before opponents spawned, and in the F12-C bot
  matrix), not an opponent failure.
- `mm2-inspect list`/`dump` audit of all 23 retail `_opp` files —
  the RACE-13 sparse-override/alternate-schema measurement.

## What this proves / does not prove

- Proves: authored opponent lineups spawn as real participants on
  retail data — own vehicles with `_opp` opponent tuning where
  authored, own `.opp` driving lines, shared race validation,
  countdown gating, session-scoped teardown/restart; opponents
  complete a retail course ahead of the scripted player (place 4 of
  7). F15-AC01's spawn leg, AC02 (own vehicles/routes), AC03
  (countdown/release + recovery law) and AC05's progress leg have
  retail-backed evidence; the `opp=` smoke field exposes it.
- Does not prove: exact retail AI behavior — the control law is the
  shared scripted law (designed), the difficulty/param-tail model is
  untouched (UNK-11), `.opp` brake/offset/speed columns are preserved
  but uninterpreted (measured all-zero on sampled routes), `_opp`
  merge semantics are a designed policy (the alternate Trans schema
  decodes nowhere), grid-slot assignment stays provisional
  (UNK-16/17), and `status=fail` on the smoke line is a bot/course
  limitation. Opponents finishing 3/6 in 90 s is honest evidence of
  competence, not parity with the original AI.
- Acceptance IDs: F15-AC01 (spawn + drive legs), AC02, AC03, AC05 —
  candidate evidence as above; AC04/AC06 (full-mode competitiveness/
  representative matrix) still open; F15-A stays `active` pending
  external review; F15-B (difficulty model) queued.

This is a candidate handoff. External code-gate and separate review
results live in the runner state directory and are not implied by
this report.
