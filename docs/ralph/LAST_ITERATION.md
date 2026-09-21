# Last implementation iteration

- Task ID and title: review-finding repair — generation-scope the two
  remaining unscoped `ResultLedger` rank consumers. F16-B.1 passed
  external review (`fb09bda`); its first verification gap was a latent
  defect adjacent to the diff's own invariant, so repair preceded new
  feature work per the selection policy.
- Starting commit: `fb09bdac38fc0e859358d5d84434e6108a81fa31` on
  `ralph/night`; tree was clean.
- Retail install: `/Users/linus/coding/rust-mm2/retail` — untouched this
  iteration (display-path repair, no content claims).

## The defect

`ResultLedger` is `init_resource`'d once and never cleared;
`Session::begin` resets `next_player`, so a restarted session reissues
the same `PlayerId`s while the ledger keeps every generation's results.
F16-B.1 added generation-scoped `standings_in`/`place_of_in` with a doc
contract — "consumers ranking a *live* race must scope" — but two
pre-existing consumers still read unscoped data:

- `update_hud` (`main.rs`) ranked the results-screen placing through
  `ledger.place_of`.
- The smoke record (`smoke.rs`) found the local participant's result
  through `ledger.iter().find` (unordered `HashMap` values — arbitrary
  among several same-participant results), ranked it with `place_of`,
  and fell back to `ledger.iter().next()` — an arbitrary result from
  any finished session.

After an in-process restart + refinish (the flow
`a_repeat_finish_never_reawards` exercises), a slower refinish could
still report the earlier generation's better place — display-only, but
wrong.

## What changed

- `crates/mm2_app/src/main.rs`: `update_hud` ranks through
  `ledger.place_of_in(session.generation(), …)` — the same scoped API
  `record_session_results` uses.
- `crates/mm2_app/src/smoke.rs`: the `outcome=`/`place=` field is a new
  `result_outcome(ledger, generation, local)` helper — the participant
  lookup reads `standings_in(generation)` (the participant's
  best-ranked result, matching `place_of_in` semantics instead of an
  arbitrary `HashMap` hit), the fallback is the generation's leading
  standing (never a prior session's), and the place comes from
  `place_of_in`. `results=` now counts `standings_in(generation)` so
  the record stays internally consistent — bit-identical on every
  single-generation run, which is all a smoke run can be (input is
  frozen under `--frames`).

## Tests

- `smoke::tests::outcome_scopes_to_the_session_generation` (new unit):
  generation-1 opponent beats local (50 vs 100 ticks), generation-2
  local finishes alone slower (150) — unscoped `place_of` still
  reports 2, `result_outcome(gen 2)` reports `place=1`, and a
  resultless generation records no outcome (the fallback cannot reach
  back). Fails on the old wiring.
- `update_hud` is private to the binary — not reachable from
  integration tests; its change is the same `place_of_in` the
  progression consumer's restart test (`a_repeat_finish_never_reawards`)
  already exercises.

## Commands actually run and results

- `cargo test -p mm2_app --lib` — 26/26 pass (incl. the new test).
- `cargo test -p mm2_app --test smoke --test progression` — 11/11 pass.
- `cargo fmt --all -- --check` — PASS.
- `cargo clippy --workspace --all-targets --all-features -- -D
  warnings` — PASS.
- `cargo test --workspace` — all suites green, 0 failures.

## Still open

- The review's other (non-blocking) gaps stand: `mm2-inspect events`
  prints no reward summary when a city's rows are all diagnostics;
  `Unlock` car ids are not cross-checked against the vehicle catalog;
  `record_eligibility` does not gate `SessionAuthority` (unreachable
  today, F24 scope); F16-AC01's two-profile restart observation and
  AC06's delete UI remain F16-C/F17.
- No GUI/manual playtest this iteration; no GPU/audio evidence
  applies.
