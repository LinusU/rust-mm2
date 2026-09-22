# Last implementation iteration

- Task ID and title: F17-A.5 — Race Records screen (DRV-5's first leg)
  plus the F17-AC05 original-menu capability audit. Also folds in the
  F17-A.4 review's non-blocking right-click/focus quirk.
- Starting commit: `b5ff6839acbdb17d496964fa41a182542cd48b4b` on
  `ralph/night`; tree was clean, previous external review verdict pass
  (F17-A.4), so this is feature work plus a review-repair fold-in.
- Why this slice: the F10-B remainder is research-gated (UNK-12) and
  F17-A's other open leg (per-event weather/time/density) is F18-gated.
  The ledger documents Race Records / Driver's Stats (DRV-5) and the
  profile already persists per-event records — real data for a real
  screen, no new persistence format.

## What changed

- `mm2_app::menu` — new `Screen::Records { city, table }`, pushed from
  a new `Race Records` root row (disabled without a bound profile —
  records are per-driver):
  - Two filter rows on top (`City:`, `Race type:`) cycling `all` plus
    the values actually present in the bound driver's records —
    Left/Right step back/forward, Enter cycles forward, all through
    `cycle_record_filter`/`cycle_choice` (authored table order, not
    lexicographic). The filters ride the screen like `NewProfile`'s
    buffer.
  - Record rows sorted deterministically (city → authored table order
    → stem — never finish chronology). Each shows the persisted
    numbers: `best <m:ss.t> place <n> x<finishes>` plus `[A]`/`[P]`/
    `[A+P]` beaten marks. `fmt_race_time` renders ticks (`s.s` under a
    minute, `m:ss.t` above).
  - A record row re-launches its event through the same `Session::begin`
    path when it still resolves and clears the gates — the stem-keyed
    `EventKey` resolves back through the live catalog like Quick Race's
    `last_event`. Gated rows name the unbeaten prerequisites,
    `Incomplete` rows their missing files, catalog-absent stems report
    themselves, Crash Course stays F21-gated — disabled rows still show
    the stored numbers.
  - Shared refactor: `availability_reason` now backs the EventList,
    Quick Race and record rows identically.
  - Deliberately absent: the documented screen's Amateur Times / Pro
    Times / Pro Points sort keys — `EventRecord` stores one
    difficulty-agnostic best time and no Pro-points field exists
    (DRV-4's formula is UNK-8). The screen shows persisted data rather
    than fabricating columns; the gap is recorded in
    `docs/research/menu.md`.
- `mm2_app::menu` root — `Driver's Stats` added as a disabled row
  naming the audit doc: the capability is tracked, not a dead-end
  placeholder (nothing persists aggregate stats to display).
- `docs/research/menu.md` (new) — the F17-AC05 capability denominator:
  every documented original menu capability (UI-1..5, DRV-1..8, CTL-8)
  mapped to implemented / tracked / open, with the design
  classifications spelled out (enhanced-policy layout choices vs
  original requirements vs unknowns). Open items stay explicit:
  Driver's Stats fields, per-screen Help "?", original art/audio,
  UI-3's stats/transmission detail, DRV-6's customization leg when F18
  lands (the eligibility site is named).
- `mm2_app::menu::menu_mouse` — review-repair: a right-click now
  returns after queueing `Back`. Previously it fell through and also
  queued the hover `FocusAt`, which applied *after* the pop and
  clobbered the parent screen's restored focus with a stale child
  index.

## Evidence

- `cargo test -p mm2_app --test menu` — 21 pass (+5):
  - `the_records_screen_shows_persisted_results_and_relaunches` —
    seeded finishes surface as `1:30.5` / `place 1` / `x2` / `[A]`;
    activating the record lands `SessionMode::Event(checkpoint:0)`
    through the real launch path.
  - `unresolvable_records_stay_listed_with_their_reasons` — gated,
    incomplete, catalog-absent and crash-course records all list
    disabled with their reasons and stored numbers, in deterministic
    sorted order; activating one never launches.
  - `records_filters_narrow_and_widen_the_list` — both filters cycle
    `all` + present values and wrap; Left steps back, Enter cycles
    like Right.
  - `a_fresh_profile_opens_records_to_the_empty_state` — a bound
    profile with no records gets the honest empty state; another
    driver's records never show.
  - `a_right_click_backs_out_without_clobbering_the_restored_focus` —
    the F17-A.4 review quirk regression.
- `cargo fmt --all -- --check` — PASS.
- `cargo clippy --locked --workspace --all-targets
  --all-features -D warnings` — PASS.
- `cargo test --locked --workspace` — PASS, 685 tests, 0 failures.

## Still open

- Records display is proven through headless model/system tests only;
  no rendered capture of the new screen was taken (the menu render
  path is unchanged since F17-A.3's screenshot).
- No original-parity claim: DRV-5's documented Amateur Times / Pro
  Times / Pro Points sort keys are absent because nothing persists
  points or per-difficulty times — DRV-5 is not marked verified. The
  original screen's layout/behavior is documented-level only.
- A seeded `EventRecord` bypasses `record_eligibility` — that's
  synthetic test data, matching how the Quick Race tests seed
  `last_event`; the eligibility writer path itself is unchanged.
- F17-A remains active: per-event weather/time/density controls need
  F18's session-legal writers (RACE-3 `customizable`); F17-AC03's full
  keyboard/gamepad + visible-focus evidence leg remains open. AC05's
  denominator now exists but AC05 itself stays open until external
  review agrees every capability is functional or tracked.
