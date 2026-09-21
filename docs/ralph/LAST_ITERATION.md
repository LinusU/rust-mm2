# Last implementation iteration

- Task ID and title: F16-B.2 — event availability derived state
  (CHK-2/CHK-3 set-of-three checkpoint gating, CC-3
  lesson→midterm→final chain, RACE-3 per-event customization). The
  first ready slice in the selection policy: F16-B's remaining
  availability-query leg.
- Starting commit: `92c8773f35033a0d1bfc73b30e7552b4d136f453` on
  `ralph/night` (also `LAST_CHECKED`); tree was clean.
- Retail install: `/Users/linus/coding/rust-mm2/retail` — read-only,
  audited below (fingerprint `fnv1a64:e91e6cd4b2ae30d9`).

## What changed

- `crates/mm2_game/src/progression.rs`: the availability contract —
  `EventGate` (`Open` / `AfterAll(Vec<EventKey>)` — prerequisites are
  resolved keys, never row indexes), `AvailabilityRow`,
  `AvailabilityTable { rows, diagnostics }`, and
  `EventAvailability { unlocked, customizable, blocked_by }`.
  `evaluate`/`of` derive state per query from the persisted `beaten`
  flags — nothing is stored, so a finish immediately re-opens what it
  unblocks. A sandbox profile (`!records_progress()`) evaluates to the
  unrestricted view (spec req 5's developer access).
- `crates/mm2_content/src/availability.rs` (new): `availability_table`
  maps a catalog's authored rows into gates — Blitz/Circuit always
  open (no authored gating), Checkpoint rows gate in authored-order
  sets of three (CHK-2/CHK-3), Crash Course rows read the authored
  `Description` tag: `lesson<N>` open, `midtrm<N>` gates
  `lesson{3N-2..3N}` (CC-2's authored order matches the tag
  arithmetic), `final…` gates every midterm (CC-3). Unreadable tags,
  midterms with no lesson group and finals with no midterms fail open
  + diagnosed — never silently locked, never silently dropped.
- `crates/mm2_app`: `EventSetup.availability` built in
  `event_race_setup`; `EventRewards` carries it session-scoped beside
  the reward table. `load_session_world` warns when the bound profile
  launches a still-locked event, naming the un-beaten prerequisites —
  the enforcing menu is F17; the CLI is a dev act, not selection.
- `tools/mm2_inspect` `events`: prints the per-city open/gated split
  and each gate's prerequisite stems plus diagnostics.
- `docs/original-rules.md`: DSN-16's "no availability query" tail
  corrected; DSN-17 records the design (including the fail-open
  designed edges and the warn-only interim enforcement).

## Tests

- `mm2_game` +5 (`tests/progression.rs`, 12 total): fresh profile sees
  set 0 open / later sets gated with `blocked_by`; partial beats still
  block; `customizable` rides the event's own beaten flag (RACE-3);
  midterm gates its lesson group; sandbox sees everything; an
  uncatalogued key reports `None`.
- `mm2_content` +4 (`tests/availability.rs`, new): sets-of-three
  chunking, tag arithmetic over a partial lesson group, unrecognized
  tag → open + diagnostic, midterm-less/final-only table fallbacks.
- `mm2_app` +1 (`tests/progression.rs`, 9 total): a locked `race:3`
  launch still reaches Countdown (warn-only) with the gate visible on
  the session's `EventRewards` resource through the production path.

## Commands actually run and results

- `cargo test -p mm2_game --test progression` — PASS (12/12).
- `cargo test -p mm2_content --test availability` — PASS (4/4).
- `cargo test -p mm2_app --test progression` — PASS (9/9).
- `cargo fmt --all -- --check` — PASS.
- `cargo clippy --workspace --all-targets --all-features -- -D
  warnings` — PASS.
- `cargo test --workspace` — PASS, all suites green.
- `mm2-inspect events <retail>` (install `fnv1a64:e91e6cd4b2ae30d9`) —
  both cities `32 open / 13 gated, 0 diagnostics`: `race3-5 ←
  race0-2`, `race6-8 ← race3-5`, `race9-11 ← race6-8`; `crash3 ←
  crash0-2`, `crash7 ← crash4-6`, `crash11 ← crash8-10`, `crash12 ←
  crash3,7,11` — exactly CHK-2/CHK-3/CC-3. London's `race12`/`race13`
  files are extras (no table rows), correctly not gated rows.

## Still open

- Availability is *reported*, not enforced — the Races/Crash Course
  menu that consumes it is F17 scope; a `--event` launch of a locked
  event runs (warned). Whether the original CLI-equivalent (direct
  launch) would even be reachable is moot — no such path existed.
- The `midtrm<N>`→lesson-group binding reads the authored tag numbers
  (designed reading of authored labels — matches CC-2's order on both
  retail tables); whether the original keys off tags or row positions
  is unobservable without the binary's UI.
- Lessons are treated as always-open per CC-3's silence (the
  racingmadness wiki's race list marks only midterms/final "Locked");
  if the original also gates later lesson groups behind earlier
  midterms, that rule is unverified — UNK-classified until evidenced.
- Vehicle/paint *selectability* (unlocks set → roster/garage state)
  is the remaining F16-B derived-state leg for F17.
- Candidate pending external check.
