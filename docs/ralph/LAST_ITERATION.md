# Last iteration — F15-B.9: AC06 measured-difficulty leg + `diff=` disclosure

Task slice on `ralph/night` (baseline `0d74699`; the F15-B.8 external
review passed with no blocking findings). Selected the F15-B remainder's
AC06 item — "difficulty changes have measured effects, with any catch-up
assistance disclosed and tested" — the last non-research-gated leg under
the parent. The difficulty switch already selects the authored aimap
variant (`.aimap` vs `.aimap_p`, RACE-11) and parameter block (DRV-2/3);
this slice makes that selection self-describing in evidence records and
proves its effects are measurable through the production path.

## What changed

`crates/mm2_game/src/config.rs`:

- `Difficulty::as_str()` — stable lowercase token for evidence records.

`crates/mm2_app/src/smoke.rs`:

- The headless record gains `diff=<amateur|professional>` — an
  unconditional run-config field beside `driver=` (not activity-gated;
  a record now names the difficulty that selected its authored content).
  Previously no record could say which parameter block produced it.

`crates/mm2_app/tests/opponents.rs` (+2, 36 total):

- `session_difficulty_fields_the_authored_variant` — one fixture
  install carrying both `race0.aimap` and `race0.aimap_p` with
  different vehicles, slot order, tuning tails and `-a-`/`-p-` route
  geometry; each difficulty's session spawns exactly its authored
  lineup through `load_session_world` → `event_aimap` →
  `opponent_roster` → `load_opponent`, asserted per roster slot
  (vehicle id, bound `throttle_cap`, route-variant lane offset).
- `session_difficulty_measures_on_track` — the same `vpt` chasing
  identical lane geometry under each difficulty, only authored col-0
  differing (0.55 vs 1.00): the professional field covers measurably
  more road at a fixed tick (>10 m over 480 updates), and both runs
  read `catch_up == 0` — a lone leader earns no assist, so the delta
  is authored tuning, never the disclosed assist (DSN-27).

`crates/mm2_app/tests/smoke.rs` (+1): `record_names_the_configured_
difficulty` — a Professional-configured run records `diff=professional`;
the existing dev-world test now also asserts `diff=amateur`.

## Verification

- Tests: `cargo test --locked --workspace` — all suites green
  (opponents 34→36, smoke 5→6).
- Gates: `cargo fmt --all -- --check` clean; `cargo clippy --locked
  --workspace --all-targets --all-features -- -D warnings` clean.
- Retail A/B (`fnv1a64:e91e6cd4b2ae30d9`, `--bot --headless`):
  - `sf circuit:0` 3000f — amateur `lap=1/3 pos=3/5 opp=0/4 cu=3`
    `opps=0:vpbug/3c,1:vpbug/2c,2:vpbug/5c,3:vpbug/5c` (authored
    throttles 0.57–0.64) vs `--pro` `lap=1/4 pos=1/5 opp=0/4 cu=4`
    `opps=0:vpbug/1c,1:vpcaddie/2c,2:vpcaddie/2c,3:vpcaddie/2c`
    (0.85–1.00). Authored NumLaps (3/4), field composition and
    per-slot progress all measurably differ. Disclosed confound: the
    variants author different routes and vehicles, so the run delta
    is the authored difficulty's whole effect — the single-variable
    isolation lives in `session_difficulty_measures_on_track`.
  - `sf blitz:0` 2000f — amateur `race=Complete tl=0.0s
    outcome=timed-out` at ticks=3600 with `env=lt00(clear-morning)
    traf=0/0` vs pro at ticks=3000 with `env=lt01(cloudy-morning)
    sky=…sky_pa_f fog=600-1000 traf=6/6 crx=11 jq=1`. Authored
    TimeLimit 30 s vs 25 s, Weather 0 vs 1 and Ambient 0.0 vs 0.2 all
    measure end-to-end through the same session path.
- Catch-up assistance remains disclosed (`cu=` on activity) and tested
  (`catch_up_lifts_a_trailing_opponents_demand`, F15-B.4) — AC06's
  second clause.

## Not done / open

- F15-B parent's remaining items: the representative avoidance matrix
  (spec edge cases — faster car behind a bus, overturned opponent,
  competing recovery positions) and `unkFlag`/`cornerBrakingThreshold`/
  `weirdPathfinding` consumption (runtime semantics unverified, UNK-11).
- Whether the original rubber-bands at all stays unverified — the
  bounded assist is a designed policy (DSN-27), disclosed in records.
- GPU/rendered output not exercised — headless physics evidence only.
- The `diff=` field changes every record line — earlier baselines
  quoted in this document predate it.
