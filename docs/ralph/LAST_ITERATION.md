# Last implementation iteration

- Task ID and title: F16-B.1 — reward/unlock import + authoritative
  result consumption. F16-A.2 passed external review (`80bebaf`);
  F16-B was the plan's selected slice. This iteration lands the first
  coherent half: authored `<city>_rewards.csv` rules normalized into a
  `RewardTable`, plus the authoritative-result → profile pipeline that
  grants them. Event *availability* (CHK-2/CHK-3, CC-3, RACE-3) is
  derived state with no consumer until F17's menus — kept open on the
  parent.
- Starting commit: `80bebafb890b5bbe9eee4fbc441c38d7d2088178` on
  `ralph/night`; tree was clean.
- Retail install: `/Users/linus/coding/rust-mm2/retail` — untouched;
  read for reward-table evidence only.

## What changed

- `crates/mm2_game/src/progression.rs` (new): `Unlock`
  (`vehicle:<id>` / `paint:<id>:<variant>` — variant kept verbatim,
  index base unverified), `RewardRequirement` (`Half`/`All`/indexed
  `Event`), `RewardRule`/`RewardTable` (per-event rules, milestones,
  per-family denominators, diagnostics), `place_requirement` (top-3
  Amateur / 1st Professional), `record_eligibility` (dev world, dev
  car, gameplay-affecting `DevOverrides`, `mods_active` →
  `Ineligible`), `apply_result` (finish → `EventRecord` + idempotent
  grants).
- `crates/mm2_game/src/profile.rs`: `EventRecord` gains `finishes`,
  `best_race_ticks`, `best_place`, `beaten_amateur`,
  `beaten_professional`; `record_finish` keeps minima and sets the
  per-rank beaten flags; `is_beaten()`.
- `crates/mm2_game/src/result.rs`: generation-scoped
  `standings_in`/`place_of_in` — the ledger outlives a session and a
  restart's stale results must not re-rank the live race.
- `crates/mm2_game/src/config.rs`: `SessionConfig.mods_active`;
  `EventTableKind::stem_prefix`/`from_reward_token` shared helpers.
- `crates/mm2_content/src/rewards.rs` (new): `reward_table(&catalog)`
  — milestone rows against authored family sizes, indexed rows bound
  to `<stem_prefix><N>` events, unmatched/malformed rows surfaced in
  `diagnostics` (never silently dropped — AC05 coverage).
- `crates/mm2_content/src/events.rs`: indexed reward attachment now
  decodes the row's `race_type` (a `race,N` row binds `race<N>`, not
  `crash<N>`); local stem-prefix helper replaced by the shared one.
- `crates/mm2_app/src/progression.rs` (new): `EventRewards` resource
  (event key + reward table, inserted only on successful event setup,
  removed on session teardown) and `record_session_results` — drains
  the authoritative `ResultLedger` into the bound `ActiveProfile`,
  filtered to this generation's `Finished` results for the local
  participant with an event id, each `ResultId` applied once; saves
  only when progress changed; grants logged.
- `crates/mm2_app/src/{session,race,main,smoke,lib}.rs`: `EventSetup`
  carries the `RewardTable`; the system runs in the real app and the
  headless smoke schedule.
- `tools/mm2_inspect/src/main.rs`: `events` audit prints the
  normalized reward-table summary (rules, family denominators,
  diagnostics).

## Design decisions

- Places come from generation-scoped standings; a solo finish is
  place 1, so Amateur/Professional both reduce to "finish" on solo
  events (the documented criterion is only distinguishable in
  multi-participant races).
- Eligibility = profile kind (`records_progress` — sandbox excluded),
  `record_eligibility(config)` (DRV-6 default-conditions extension),
  `ScriptedDrive` absent (`--bot` is evidence tooling, not the
  player), `Finished` only. Mods are conservatively ineligible until
  F29 classifies per-mod impact. All designed policy — classified in
  DSN-16.
- Milestones measure *beaten* events (place criterion met), not
  finishes — `half` on an odd family is `div_ceil` ("at least half";
  every retail family is even, so the odd case is a designed choice).
- No availability query ships: CHK-2/CHK-3/CC-3/RACE-3 gating is
  derived state over `beaten` flags whose consumer is F17.
- Ledger promoted VEH-3/VEH-4/CC-6 to `verified_original`: the
  authored `*_rewards.csv` rows match the help text exactly
  (`crash,3/7/11` = the three midterm rows, `crash,12` = the final).

## Tests

- `mm2_game` (tests/progression.rs, tests/profile.rs): place
  criterion per rank, milestone math (`half`/`all` denominators),
  indexed grants, dedup/repeat-finish idempotency, `TimedOut` inert,
  eligibility variants, `EventRecord` best-place/best-ticks/flags.
- `mm2_content` (tests/events.rs): family-aware indexed attachment,
  stray-index → diagnostic, `reward_table` denominators + rule
  counts.
- `mm2_app` (tests/progression.rs — 8): real `load_session_world` →
  drive-to-finish → profile saved on disk with event record + both
  authored unlocks; sandbox/timeout/modded/dev-world/non-local/bot
  finishes all record nothing; restart + repeat finish grows
  `finishes` but never re-grants.

## Commands actually run and results

- `cargo test -p mm2_app --test progression` — 8/8 pass.
- `cargo fmt --all -- --check` — PASS.
- `cargo clippy --workspace --all-targets --all-features -- -D
  warnings` — PASS.
- `cargo test --workspace` — all suites green, 0 failures.
- Retail evidence: `mm2-inspect events` on
  `/Users/linus/coding/rust-mm2/retail` — london and sf each
  normalize to 4 event-bound + 6 milestone rules over authored
  families Blitz=10 / Checkpoint=12 / Circuit=10 / CrashCourse=13,
  0 reward diagnostics (install untouched).

## Still open

- F16-B parent stays `active`: no availability consumer (F17 scope),
  `vpmoonrover` has no authored rule (UNK-3), Pro points unimplemented
  (DRV-4/UNK-8).
- F16-AC01's two-profile restart observation is now exercisable but
  not yet run as an end-to-end scenario; AC06 (deliberate-delete UI)
  is F17; AC05's coverage claim rests on the synthetic + retail audit
  above.
- Paint `VariantNum` index base unverified (stored verbatim).
- No GUI/manual playtest this iteration; no GPU/audio evidence
  applies.
