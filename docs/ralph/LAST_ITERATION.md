# Last implementation iteration

- Task ID and title: F10-A.1 review repair — non-finite BAI lane
  geometry panics `plan_ambient`. External review #12 (verdict fail)
  had exactly one blocking finding; per the iteration contract a
  failing review is repaired before any new feature work.
- Starting commit: `a14263a451def609000878608c528ffc8c05a93b` on
  `ralph/night`; tree was clean.

## Root cause

`Bai` lane vertices are raw `f32::from_bits` with no finiteness check.
`push_lane` (nav.rs) computed `length` from the cumulative distances
and admitted a NaN length because `NaN <= f32::EPSILON` is false — no
`NavIssue` emitted. `plan_ambient` sweeps every eligible lane every
draw, so `along = rng.next_f32() * l.length` became NaN and
`sample_storage`'s `s.clamp(0.0, lane.length)` panicked on the NaN
bound (verified by the reviewer). Sibling case: a `+inf` length gave
`0.0 * inf = NaN` positions that passed the player-bubble check
(`NaN < min` is false). Prior consumers never hit it: `nearest_lane`'s
distance filter never selects a NaN lane and the nav overlay's
`while s < lane.length` loop skips it. A corrupt or modded `.bai`
(mod content is in scope) turned ambient planning into a deterministic
crash.

## What changed

- `crates/mm2_game/src/nav.rs` — new `NavIssue::NonFiniteLane`
  (road/side/kind/index, mirrors `DegenerateLane`). `push_lane` now
  requires authored distances finite + monotone (else recompute from
  vertices with the existing `LaneDistancesRecomputed` issue), finite
  vertices and finite length — offenders drop out of the graph with the
  new issue instead of being admitted.
- `crates/mm2_game/src/traffic.rs` — `plan_ambient`'s eligible-lane
  filter re-checks `l.length.is_finite()` and finite vertices
  (belt-and-braces; the graph already guarantees it).
- `crates/mm2_formats/src/bai.rs` — `Bai::validate` reports the new
  `BaiIssue::NonFiniteCurveVertex` (road/side/kind lane|tram|train/
  curve/first-bad-vertex; one issue per curve) so `mm2-inspect bai`
  audits corrupt geometry instead of reporting the file clean.
- `crates/mm2_formats/src/veh.rs` — non-blocking review notes folded
  in: malformed `CG` now warns like `MaxAng` through a shared
  `opt_vec3` helper (a malformed optional vector is no longer
  indistinguishable from an absent one), and the "All 25 retail
  records" doc is corrected to the measured 23.
- Docs: `docs/ralph/PLAN.md` (F10-A.1 row, F09-A.1 validate coverage,
  selection-policy header).

## Evidence

- `cargo test -p mm2_game --test traffic` — 9 pass, incl. the new
  `plan_skips_non_finite_lane_geometry` regression: a NaN-vertex lane
  with dropped distances (the old panic path) and an inf vertex under
  valid authored distances (the sibling NaN-position path) both surface
  as `NonFiniteLane`, the plan runs, only road 0's two lanes stay
  eligible, every spawn is finite; a non-finite authored distance row
  recomputes instead of dropping.
- `cargo test -p mm2_formats` — 115 unit + 13 vehicle-format tests
  pass, incl. `validate_flags_non_finite_curve_vertices` (lane/tram
  kinds, per-curve reporting) and the new malformed-CG leg in
  `aivehicledata_tolerates_absent_cg_and_flags_garbage`.
- `cargo fmt --all -- --check` — PASS.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — PASS.
- `cargo test --locked --workspace` — PASS, all suites, 0 failures.

## Still open

- Unchanged from the F10-A.1 slice: no runtime ambient entities,
  lane-following or collision yet — F10-A's ACs and F10-B remain unmet.
- `SpawnPolicy`'s pool/distance bounds remain designed values (UNK-12).
- Remaining non-blocking review notes not taken this round:
  `TrafficAudit::discovered()` counts rostered ids whose tune file
  never resolved (slightly inflates "discovered" on partial installs);
  the two thin city-aimap resolve/read/parse wrappers
  (`load_city_aimap` vs `load_nav_overrides`) could share a helper.
- Retail `mm2-inspect bai /Users/linus/coding/rust-mm2/retail --strict`
  re-run: identical to baseline — 5 parsed, 2 unsupported extras, exit
  2 on the same 2 pre-existing `sfai.bai` issues; the new
  `NonFiniteCurveVertex` check fires zero times on retail data (it only
  reports corrupt/modded files).
