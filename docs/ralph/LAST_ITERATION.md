# Last implementation iteration

- Task ID and title: F16-B.2 review repair — `crash_gate` tag-number
  overflow. The external review of `76a3c22` (F16-B.2, event
  availability derived state) failed on one blocking finding; this
  iteration repairs it. No unrelated feature work.
- Starting commit: `76a3c22da4f85b3857d9c37992fb933e79e9409a` on
  `ralph/night` (the failed candidate); tree was clean.

## Root cause

`crates/mm2_content/src/availability.rs` `crash_gate()` computed the
midterm's lesson group as `3 * n.saturating_sub(1) + 1` in u32, where
`n` is the authored `midtrm<N>` tag parsed from `mmcrashdata.csv`'s
`Description` column — reachable via any modded/corrupt crash table
through the VFS. For `N >= 1431655765` the multiply/add overflows:
`midtrm1431655765` panics under debug overflow checks, and in release
`midtrm2863311533` wraps to `first = 5`, silently gating the midterm
on lessons 5–7 with no diagnostic. Both violate the module's contract
that malformed structure lands in `diagnostics` and fails open.

## What changed

- `crates/mm2_content/src/availability.rs`: the group bounds are now
  computed in u64 (`3 * u64::from(n.saturating_sub(1)) + 1`, range
  `first..first + 3` matched against `u64::from(lesson)`). A tag whose
  `3N` exceeds the u32 lesson numbers can never match a row, so it
  falls through to the existing "found no lesson rows" diagnostic and
  `EventGate::Open` — no arbitrary cutoff, and `midtrm0` still gates
  on lessons 1–3 exactly as before. No behavior change for any tag
  that fit u32 group arithmetic.

## Tests

- `mm2_content` +1 (`tests/availability.rs`, 5 total):
  `an_out_of_range_midterm_tag_is_open_and_diagnosed` authors
  `lesson5`–`lesson7` plus `midtrm2863311533`. Under the old code this
  panics in checked builds and wraps into a silent `AfterAll` gate on
  the three authored lessons in wrapping builds; the test asserts
  `Open` plus exactly one diagnostic naming the row, so it fails both
  ways.

## Commands actually run and results

- `cargo test -p mm2_content --test availability` — PASS (5/5).
- `cargo fmt --all -- --check` — PASS.
- `cargo clippy --workspace --all-targets --all-features -- -D
  warnings` — PASS.
- `cargo test --workspace` — PASS, all suites green (0 failures).
- Retail `mm2-inspect events` was not re-run: the change cannot alter
  any retail row (all authored `midtrm` tags are 1–3, far below the
  overflow boundary); the previous iteration's retail audit stands.

## Still open

- Everything from the F16-B.2 handoff still stands: availability is
  reported, not enforced (F17 menu); whether the original keys
  crash-course gating off tags vs row positions is unobservable
  (designed reading); lesson groups possibly gating behind earlier
  midterms is UNK-classified; vehicle/paint selectability remains the
  open F16-B derived-state leg.
- Candidate pending external check.
