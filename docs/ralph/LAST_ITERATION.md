# Last iteration — F07-B.5 review repair: ledger rows for the clutch trigger

Iteration 42 on `ralph/night` (baseline `9de86e6`, the F07-B.5 clutch
one-shot commit — external verify green, review **fail** on one doc
finding). Repair iteration: doc-only, no code or test changes.

## Root cause

The external review verified the F07-B.5 implementation itself (watch
semantics, bindings, gates) but rejected the candidate because
`docs/original-rules.md` was not updated in the same commit: UNK-25's
clutch clause still read "the clutch wave name's trigger (reverse loop
vs shift blip — unbound)" after the trigger had been bound to a
designed reading, and no DSN row recorded it — every sibling audio
slice (F07-A.2 through B.4 → DSN-35..39) had recorded its designed
binding in the ledger the same commit. Per AGENTS.md the ledger must
be updated when a rule's status changes.

## Actions

- `docs/original-rules.md` — added `DSN-40`: the `GearWatch` /
  committed-`(gear, direction)` clutch trigger — one one-shot per
  committed pair change, first sight silent, watch advances while not
  `Playing` (no stale flush), remote watched-not-voiced, sentinel =
  authored silence, unresolvable stem = `failed` per change, same-frame
  double shift collapses to one voice, mid-`Playing` `ResetVehicle`
  voices a clunk, `PlaybackMode::Despawn` `SessionEntity` children,
  spatial non-local (DSN-37), `MAX_CLUTCH_VOICES` 8 with `+Nd`,
  `aud=` `/Nc`, `authored_volume` shared with the horn.
- `docs/original-rules.md` — UNK-25 updated: F07-B.5 joined the
  implemented-slice enumeration (DSN-40) and the clutch clause now
  reads "designed committed-`(gear, direction)`-change trigger
  implemented (DSN-40); which transitions the original voices stays
  unverified".
- `docs/ralph/PLAN.md` — recorded the repair in the run narrative.

## Gates

Doc-only diff; `cargo fmt --all -- --check` and `cargo clippy --locked
--workspace --all-targets --all-features -- -D warnings` re-run clean;
`cargo test --locked --workspace` unchanged (no code touched — the
review independently confirmed 68 suites / 1089 tests / 0 failures at
`9de86e6`, which this commit only amends in docs).

## Remaining open items (unchanged from B.5)

- F07-B continues: ambient-traffic engines, siren programs,
  sustained-scrape semantics (AC04 second leg), AC02 scripted drive
  sequence, AC05 audible capture, weather→{dry,wet,ice} surface
  variant binding.
- UNK-25 designed readings: which transitions the original voices and
  whether multi-gear jumps multi-voice; `MAX_CLUTCH_VOICES` 8.
- No audible A/B against the original; headless `0s` reports no output
  device honestly (F07-C scope).
