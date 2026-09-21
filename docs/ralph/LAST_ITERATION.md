# Last implementation iteration

- Task ID and title: F10-A.1 — ambient-traffic catalog + seeded
  spawn-policy planner. Chosen from TASKS.json: F10-A is the
  highest-priority ready feature (priority 4; deps F01-B checked,
  F02-A/F09-B implemented) and the nav groundwork (`NavGraph` lanes,
  `NavOverrides` closures, `NavRng`, `choose_exit`) was built for
  exactly this. The plan's named remainders were all blocked (F17-B
  needs F17-C's mode, F17-A's weather controls need F18, F15-B is
  research-gated, F16-C's AC01 leg needs interactive play).
- Starting commit: `6a37ccad898c26dfe9570380656e15d8629c66a4` on
  `ralph/night`; tree was clean (F17-A.3 externally checked pass).

## What changed

- `crates/mm2_formats/src/veh.rs` — `AiVehicleData` typed decoder for
  `tune/vehicle/*.aivehicledata` (23 retail records): required scalars
  Mass/Size/Elasticity/Friction/MaxDamage/PtxThresh/Spring/Damping/
  Limit/RubberSpring/RubberDamp + `MaxAng`; optional `CG` (absent on 3
  records). MSVC non-finite literals (`1.#QNAN0`, `-1.#INF000`,
  `1.#IND`) decode via `msvc_float` — `va_garbagetruck` authors a NaN
  MaxAng. Unknown fields and malformed optional vectors are warnings,
  not silently zeroed; missing required fields fail.
- `crates/mm2_game/src/nav.rs` — `NavRng::next_f32` (top 24 bits of the
  seeded u64, deterministic `[0,1)`).
- `crates/mm2_game/src/traffic.rs` — pure domain layer:
  `AmbientSpec`/`AmbientRoster` keep the authored cumulative-weight
  table verbatim (duplicate ids are legitimate — london authors
  `va_compact_s` on two weight bands); `SpawnPolicy` holds the
  designed pool/distance bound (the original's values are UNK-12);
  `plan_ambient` draws a density-scaled target of spawns: class by
  cumulative pick, position over routable vehicle lanes the
  `NavOverrides` leave open, bounded retries against
  `min_player_distance`, unresolved-class draws drop and report once
  per id — the authored weight band is never rebalanced. Empty or
  malformed rosters report issues instead of panicking; same seed →
  identical plan, different seeds can diverge.
- `crates/mm2_content/src/traffic.rs` — VFS producer + audit:
  `ambient_roster` loads `city/<city>.aimap` and decodes each rostered
  id's `.aivehicledata`; `ambient_roster_from_aimap` serves
  event-level tables the same way (measured on retail:
  `race/london/roam.aimap{,_p}` and `roambak` carry their own 12-row
  rosters). `TrafficAudit::scan` resolves `geometry/<id>.pkg`,
  `bound/<id>_bound.bnd` and counts `.mtx` parts per class, reports
  unrostered ids and undiscovered `EXPECTED_AMBIENTS` (23), and
  censuses every `race/<city>/*.aimap{,_p}` carrying
  `[Density]`/`[Speed Limit]`/`[Exceptions]`/own rosters. Roster
  slots survive failed tuning loads — failures are reported, not
  dropped.
- `tools/mm2_inspect/src/main.rs` — `mm2-inspect traffic <install>
  [--city] [--strict]` prints the roster table, per-class asset
  status, unrostered/missing ids, the event-override census and all
  diagnostics; `--strict` exits 2 on any failed check.
- Docs: `docs/research/aimap.md` ambient-rosters section,
  `docs/original-rules.md` WLD-20 + UNK-12 narrowed,
  `docs/ralph/PLAN.md` F10-A/F10-A.1 rows.

## Evidence

- `cargo test -p mm2_formats --test vehicle_formats` — pass (6 new
  `AiVehicleData` cases: field decode, MSVC NaN/-inf, absent CG,
  malformed MaxAng warning, missing-required, wrong root).
- `cargo test -p mm2_game --test traffic` — 8 pass (determinism,
  seed sensitivity, empty roster, closed-road filtering,
  player-distance filtering, target/drop accounting, duplicate-row
  cumulative weights).
- `cargo test -p mm2_content --test traffic` — 3 pass (synthetic-VFS
  roster/audit, unrostered + missing reporting, event aimap).
- `cargo fmt --all -- --check` — PASS.
- `cargo clippy --workspace --all-targets --all-features --
  -D warnings` — PASS.
- `cargo test --workspace` — PASS, all 48 suites, 0 failures.
- Retail audit: `mm2-inspect traffic /Users/linus/coding/rust-mm2/
  retail` → london `23 discovered, 12 rostered, 11 unrostered, 0
  failed checks, 0 diagnostics`; sf `23 discovered, 11 rostered, 12
  unrostered, 0 failed, 0 diagnostics`; 101 london + 104 sf event
  overrides counted (35/53 exceptions, 3/6 density, 3/0 own rosters).
  `--strict` exits 0.

## Still open

- This slice is cataloging/planning/auditing only — no runtime
  ambient entities, no lane-following, no player collision. F10-A's
  ACs and F10-B remain unmet until those exist.
- `SpawnPolicy`'s pool size and distance bound are designed values —
  the original's active-pool size, spawn ring and despawn rules are
  UNK-12 (recorded).
- Ambient flag semantics, `[Density]` count-vs-rate, and the
  event-roster replacement rule (replace vs merge with the city
  table) are unverified authored-data questions.
- The planner only sees `NavOverrides` closures; it does not yet
  weight lane choice by `effective_speed` or intersection turn rules
  beyond the authored lane set.
- `sf --event checkpoint:0` headless fall-through and the other
  pre-existing F15/F16/F17 remainders are unchanged.
