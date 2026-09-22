# Last iteration — F17-A.6 menu condition options (RACE-3/RACE-4)

Iteration 45 on `ralph/night`, continuing from `f13bd3f` (the F18-A.2
lighting-binding candidate — external review verdict **pass**, gaps:
rendered captures not independently reproduced, no original-executable
comparison, selector→slot semantics beyond the measured name grid stay
UNK-24). TASKS.json's F17-A remainder needed per-event weather/time/
density controls — unblocked by F18-A.2's session-legal writers — so
this slice lands the menu's condition-options screen plus the
`SessionConfig::customization` contract that carries picks into the
session. F17-A and F18-A stay **active** — see the deferred lists.

## What changed

- `mm2_game::config`: `SessionConfig::customization:
  Option<SessionCustomization>` — the player's picked
  `SessionConditions` + `Densities` for a session. `TimeOfDay::name()`
  / `Weather::name()` expose the WLD-21-measured selector names
  (morning/noon/evening/night, clear/cloudy/foggy/rainy) for display.
  `validate()` checks the customization densities like the base ones.
- `mm2_game::race::effective_conditions` — resolution order is now
  customization → authored `EventParams` → `SessionConfig::conditions`,
  so an explicit pick beats the authored event definition (the point
  of RACE-3's options) while an unchanged event launch still gets its
  authored conditions (RACE-2 preserved).
- `mm2_game::progression`: `Ineligible::Customized` — DRV-6's
  default-conditions rule mapped onto the field's *presence*:
  `record_eligibility` refuses any session carrying a customization.
- `mm2_app::environment`: `ConditionsSource::Customized`; `session.rs`
  reports it when the bound preset came from the player's picks.
- `mm2_app::traffic`: `load_ambient_traffic`'s density chain takes
  `customization.densities.traffic` first — before the event aimap,
  authored table dial, city aimap and `SessionConfig` fallback (the
  authored layering stays an implementation choice, UNK-12).
- `mm2_app::menu`: `Screen::Customize{target, conditions, densities,
  seed_conditions, seed_densities}` — rows `Weather:` / `Time of day:` /
  `Traffic density:` / `Start cruise|race`; Left/Right and Enter cycle
  in place (`step4` wraps 0-3 selectors, `step_density` quarters the
  0..=1 range, snapping an authored between-steps seed to the nearest
  step on first press). Cruise city rows gain an always-open
  `  options` row (RACE-4); every EventList event gains a `  options`
  row gated on `EventAvailability.customizable` — RACE-3's beaten
  flag — disabled with its reason (no bound driver, `beat this race…`,
  the event's own launch gate, or out-of-range authored values).
  Event seeds are distilled from the selected difficulty's authored
  `RaceParams` (`authored_seed` — same range checks `event_params`
  performs; a bad authored value disables the row rather than
  fabricating a default). `Action::LaunchCustomize` builds the session
  mode from the target and sets `customization` only when picks differ
  from the seed — an unchanged visit launches a default run, so DRV-6
  eligibility is unaffected by opening the screen. All input reaches
  it through the shared `MenuCommand`/`apply`/`MenuEffect` path.

## Deferred (kept open deliberately)

- Pedestrian/cop density pickers — no runtime consumers exist
  (F19/F20); `densities.pedestrians` rides the authored seed so a
  future picker lands on the field.
- Circuit laps/opponents customization — no authored writers; DRV-6
  names them among the excluded customizations.
- Quick Race customization; condition persistence/replication
  (F18 req 5); `.sky`/fog/PVS/precipitation (UNK-24, F18-A/B/C).
- The original's exact option-row layout/step granularity — the
  quarter-step density dial is an enhanced choice (authored data is
  continuous); no original menu captures exist to compare.

## Verification (this tree)

- `cargo test -p mm2_app --test menu --test environment` — pass
  (menu 25, environment 5).
- `cargo test -p mm2_game` — pass (all suites; the two new tests among
  them).
- Tests +7: `tests/menu.rs` ×4 —
  `event_options_unlock_only_after_the_race_is_beaten` (options row
  names no-driver/unbeaten/CHK-3-gated/incomplete reasons),
  `customized_event_launch_carries_the_picks` (seeded screen → cycled
  tod/density → launched `SessionConfig::customization` picks,
  `EnvironmentReport` slot 4/`Customized`, `Ineligible::Customized`),
  `unchanged_options_launch_a_default_run` (`customization: None`,
  `record_eligibility` Ok), `cruise_options_launch_a_customized_session`
  (RACE-4 profile-less open, weather pick lands on the config);
  `tests/environment.rs` ×1 — `customized_event_conditions_take_precedence`
  ((3,3) picks bind `lt15` over authored (1,2), source `Customized`);
  `mm2_game` ×2 — `effective_conditions_prefers_the_player_customization`,
  `record_eligibility_refuses_customized_conditions`.
- `cargo fmt --all -- --check` — pass.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — pass.
- `cargo test --locked --workspace` — pass, 64 suites / 0 failures.

## Retail evidence (fingerprinted install `fnv1a64:e91e6cd4b2ae30d9`)

- `sf --headless --frames 60` → `status=pass wheels=4/4
  env=lt00(clear-morning) traf=16/16` — the default cruise path
  unchanged (no customization → configured conditions, authored
  fallback chain).
- `sf --event checkpoint:0 --headless --frames 60` → `status=pass
  env=lt00(clear-morning)` with `density=0.1` — authored event
  conditions/density still win when no customization exists (RACE-2).
- `--menu --frames 30` → `smoke=visual world=menu status=pass`
  (Metal/Apple M1) — menu boots with the new rows; driving the
  options screen itself is interactive-only (input is frozen under
  `--frames`), so the screen's rendered leg stays with F17-AC03.

## Ledger / docs

- `docs/original-rules.md`: new DSN-29 (the slice's design
  classifications — DRV-6's presence-mapping, the quarter-step dial
  and the authored-seed refusal are recorded as implementation
  choices/enhanced, not original claims).
- `docs/research/menu.md`: race condition options → implemented
  (partial), deferred fields listed; DRV-6 note updated to the
  implemented `Ineligible::Customized` gate.
- `docs/ralph/PLAN.md`: F17-A.6 row; F17-A/F18-A parent rows updated.

## Not done / blockers

- No rendered/interactive capture of the new screen (menu tests drive
  the production `apply` path headlessly; F17-AC03's rendered
  evidence leg stays open).
- No original-executable comparison of the options screen or its
  exact cycling semantics.
