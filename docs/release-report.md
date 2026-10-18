# Release acceptance and handoff report (F31-C)

The final acceptance pass for the recreation scope: what was run, at which
commit, what it shows and what it does not. It is a *handoff*, not a
completion claim — no row here promotes a feature beyond the evidence it
names, and this page deliberately leaves the F31 acceptance criteria open
where the evidence is partial.

- **Checked commit:** `d42e6ef` ("Record the remaining london circuit
  opponent re-measure on the neural driver"), the tip of `origin/main` at
  the start of this pass.
- **Retail install:** `fnv1a64:e91e6cd4b2ae30d9` (the fingerprint the
  rules ledger and every coverage page were measured against).
- **Machine:** Apple M1 (arm64), macOS; a shared build machine, so run
  times are order-of-magnitude, not a benchmark.

Two documents are the substance of this audit and are not duplicated here:

- `docs/coverage-audit.md` (F31-A.1) — the evidence-level legend, the
  retail catalog denominators and the per-feature F00–F30 gap matrix. This
  page cites its rows rather than restating them.
- `docs/original-rules.md` — the authoritative open set: which rules are
  original, documented, inferred, designed or unknown.

## 1. Evidence levels

From `specs/QUALITY-GATES.md`; every claim below carries one.

| Tag | Shows | Does not show |
|---|---|---|
| code | fmt/lint/tests pass at a commit | content loaded, anything audible/visible |
| synthetic | self-authored fixtures through the production path | every retail variant |
| retail-data | the fingerprinted install was parsed/driven headlessly | a person looked/listened |
| rendered/played | a capture or interactive run was observed | universal platform/content support |
| original-verified | a rule confirmed against retail data/help/binary | any `designed` rule |

## 2. What was run in this pass

Everything below was executed on the checked commit with the retail install
mounted read-only. A skip is named as a skip; nothing is reported as a pass
because a prerequisite was missing.

### 2.1 Code gates (CI-equivalent: no retail data, retail tests skip)

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --locked
```

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | **pass** (exit 0) |
| `cargo clippy … -D warnings` | **pass** (exit 0) |
| `cargo test --workspace --locked` | **pass** — 3479 passed, 0 failed across 63 test targets (`MM2_RETAIL` unset, so the retail-gated tests print a skip) |

### 2.2 Content acceptance (`mm2-inspect`, original-content validation)

All commands exit 0 unless noted. Outputs are kept out of the repository
(`target/f31c/`) because they quote retail paths.

```sh
mm2-inspect inventory "$MM2_RETAIL"
mm2-inspect events    "$MM2_RETAIL"
mm2-inspect race-defs "$MM2_RETAIL"
mm2-inspect event     "$MM2_RETAIL" --all
mm2-inspect audio     "$MM2_RETAIL"
mm2-inspect specials  "$MM2_RETAIL"
```

The measured denominators are in §3; the per-command summaries in §4.

### 2.3 Journeys (the F31-B scripts, re-run here)

The single-player, negative-path, lesson/Cops-and-Robbers and multiplayer
journeys from the F31-B slices were re-run at this commit:

```sh
# single-player + negative paths + lesson/C&R (synthetic fixtures)
cargo test -p mm2_app --test app -- menu::a_named_driver_races_earns_a_reward_and_finds_it_after_a_relaunch
cargo test -p mm2_app --test app -- menu::an_unreadable_profile_degrades_visibly_and_other_profiles_survive
cargo test -p mm2_app --test app -- menu::pausing_or_losing_focus_mid_race_freezes_the_clock_and_awards_nothing
cargo test -p mm2_app --test app -- menu::a_duplicate_result_does_not_double_award_the_reward
cargo test -p mm2_app --test app -- menu::a_lesson_fails_retries_passes_and_returns_to_the_menu
cargo test -p mm2_app --test app -- menu::a_cops_and_robbers_round_is_played_from_the_menu_and_left
# multiplayer lobby → race → results → rematch → leave over a lossy loopback link
cargo test -p mm2_app --test network -- net_app::a_multiplayer_journey_survives_a_lossy_link
# retail single-player menu journey (needs MM2_RETAIL; not human play, see §5)
MM2_RETAIL="$MM2_RETAIL" cargo test -p mm2_app --test app -- menu::a_retail_stock_event
```

Results are in §5. The two multi-process retail legs that exist for the
network path (`net_drive::two_retail_processes_*`) are not re-run here: each
takes 10–25 minutes on this shared machine (see `docs/performance.md`
"Headless fixed tick"). Their last recorded runs are cited in
`docs/multiplayer-coverage.md`; re-running them is a reviewer/owner step.

## 3. Catalog denominators (original-content validation)

From `mm2-inspect inventory` at this commit. Expected is the authored
denominator; rejected rows stay **in** the denominator and are never
silently substituted.

| Family | Expected | Discovered | Accepted | Rejected | Unverified |
|---|---|---|---|---|---|
| cities | 2 | 5 | 5 | 0 | 0 |
| player vehicles | 21 | 29 | 21 | 8 | 0 |
| races | 80 | 111 | 78 | 20 | 111 |
| crash-course lessons | 42 | 42 | 42 | 0 | 42 |
| placement sources | 2 | 13 | 13 | 0 | 157 |
| audio families | 7 | 7 | 7 | 0 | 3293 |
| pedestrian archetypes | 4 | 5 | 4 | 4 | 67 |
| multiplayer variants | 0 | 131 | 0 | 0 | 131 |
| ambient traffic vehicles | 0 | 23 | 0 | 1 | 23 |
| breakable props | 0 | 995 | 0 | 0 | 995 |
| player profiles | 0 | 17 | 0 | 0 | 17 |
| interface & effects data | 0 | 117 | 0 | 0 | 117 |

- The 8 rejected vehicles are incomplete spare/variant ids (`vpdb731`,
  `vpeagle`, `vpftruck`, `vplafrance`, `vpvw_cup`, `vpvw_dune`,
  `vpvwcup_angel`, `vpvwdune`); the roster's 21 stock cars are all accepted.
- The 20 rejected races are two partial circuits (`circuit11` in each
  city, `Opp` but no `.aimap`) and 18 non-content backup/conflict artefacts.
- Families with no authored roster (breakables, traffic, profiles,
  interface data) report `expected 0` and an `unverified` discovered count
  — an honest "we counted files, we did not define completeness" state,
  not a denominator narrowed to what the importer accepts.

## 4. Per-command content results

- **`events`** — both cities: the 45 authored selectable rows (12
  Checkpoint + 10 Blitz + 10 Circuit + 13 Crash Course) are `ready`, with
  their event-metadata tables parsed without row diagnostics; each city
  normalizes to 4 event-bound + 6 milestone rewards and the documented
  availability chain, 0 diagnostics. Extra authored stems the tables do not
  list (london 33, sf 23) are shown as extras, not dropped.
- **`race-defs`** — per city: 45 events, **64 built, 26 unsupported, 0
  failed builds**. The 26 `unsupported` are the Crash Course row ×
  difficulty cases, which run through the F21 lesson path rather than
  `RaceDefinition` (see §6).
- **`event --all`** — london `45 ready, 0 incomplete, 53 strict failures`;
  sf `45 ready, 0 incomplete, 43 strict failures`. "Ready" means
  dependencies resolve, *not* verified playable. The strict failures are
  disclosed authored anomalies (extra stems not in the tables, and the
  retail `AmbDenisty` header typo), kept verbatim.
- **`audio`** — 3256 files classified, **2503 waves decoded** (0
  failures, 0 issues), 1905 drawable speech clips, 1815 resolve, **90 name
  a wave the install does not ship**, plus 3 extra files, 36 quirks, 1
  finding and 1 dead reference. No audible capture is recorded (see §6).
- **`specials`** — expected default pathways 9/9, **20/21 pathset files
  loaded, 0 failures, 0 unresolved actors, 1 partially reproduced** (the
  San Francisco cable car). London authors no cable cars; SF authors no
  Underground train pathsets — listed absent, never invented. The one
  unloaded file is the truncated, unreachable `london_bridge_blitz10`.

## 5. Journey results (F31-AC03 / AC04)

| Journey | Level | Result at this commit |
|---|---|---|
| Single-player: name → locked paint/race → launch (`race0`) → authored finish → results → restart → menu → relaunch | synthetic | pass |
| Negative: unreadable profile degrades visibly, other profiles survive | synthetic | pass |
| Negative: pause/focus loss freezes the clock, awards nothing | synthetic | pass |
| Negative: a duplicate result does not double-award the reward | synthetic | pass |
| Lesson: idle lesson fails, *Retry* passes, returns to menu, midterm opens | synthetic | pass |
| Cops & Robbers: menu → Start match → gold → decided → *Play again* → menu | synthetic | pass |
| Multiplayer: lobby → mode edit → Checkpoint race → `Results` → rematch → leave, each client behind a seeded 20 % loss/duplicate/reorder/jitter proxy | synthetic, loopback | pass |
| Retail single-player: menu → london Blitz `blitz0` (real `city/london.psdl`, 3 gates + finish) → `Finished` → Results | retail-data | pass, **record-ineligible** |

The retail leg drives with the production scripted evidence driver, whose
finish deliberately banks nothing (`DSN-16`); it proves the menu → session →
real authored course → results path, not an eligible reward. An
*eligible* retail reward leg and a retail-data multiplayer journey remain
open (`docs/coverage-audit.md` §4.2, §5.5).

## 6. Open blockers (what keeps F31 open)

Cross-cutting, from `docs/coverage-audit.md` §4 and re-confirmed here:

1. **Crash Course gameplay.** 42 authored lessons parse and the catalog is
   structural, but the evaluators are unrecovered (`UNK-14`/`UNK-35`); the
   26 per-city `race-defs` rows stay `unsupported`. The largest
   scoped-out *gameplay* hole with data already inventoried.
2. **Audio is unlistened.** Decode, cue logic and mixer have code and
   synthetic evidence; no offline mix or captured listen exists
   (F07-AC05, F08-AC05, F18-AC05, F23-AC05).
3. **No recorded human playthrough.** Every result is headless or
   scripted; the retail leg's driver is record-ineligible.
4. **Original-fidelity comparisons are absent.** Placing, pacing, AI
   difficulty, wet traction, damage thresholds and police behaviour are
   engine self-metrics or `designed`; the 45 open `UNK-*` entries in
   `docs/original-rules.md` are the authoritative set.
5. **Multiplayer is loopback only.** No LAN or Internet evidence; the Cops
   & Robbers drop/steal/respawn rules stay unknown (`UNK-10`).
6. **Mod coverage is uneven.** Override infrastructure exists; no audit
   shows every consumer honours it (F29-AC01/AC05; filed as #1495).
7. **One-shot network messages.** A lossy link found `SetReady`/`SetVehicle`
   (#1488) and `Start`/`Cancel` (#1489) were not loss-safe. Both are now:
   the client repeats `SetReady`/`SetVehicle` until the roster echoes them
   (#1488) and the host repeats `Start`/`Cancel` for a bounded window while
   the client ignores a repeat (#1489), so the journey crosses all of them
   on the lossy lane.
8. **Circuit/opponent pacing.** The scripted 597 s sweep left some
   circuits short; the neural-driver re-measure completed every circuit it
   re-ran, but `london circuit:8` Professional is slower than the 12-minute
   cap (slowness, not a stall) — `docs/research/circuit-completability.md`.

## 7. Platform and performance matrix

From `docs/building.md` and `docs/performance.md`; no platform claim is
made that was not actually run.

| Platform | Status |
|---|---|
| macOS arm64 | **tested** — gates, windowed Metal runs, release build/package |
| Linux x86_64 | build + tests **tested in CI only** (headless); windowed *untested* |
| Windows MSVC | `cargo check` **green in CI only**; build/run/`%APPDATA%` *untested* |
| macOS Intel / Linux arm64 | *untested* |

- Performance: the two recorded free-roam runs and the populated soak are
  in `docs/performance.md`. One network soak client failed on a single
  unexplained render stall; entities, voices and queues passed. F30-AC02
  stays open (longer/wider soaks, per-system timing, draw-call/GPU-memory/
  bandwidth columns).
- Artifacts: `scripts/release-build.sh` (private-path-free, stripped) and
  `scripts/package.sh` (flat / `.app` / prefix + archive, allowlist-checked)
  build the distributables; last recorded runs are in `docs/building.md`.
  **No artifact is attached to this report**, and none may carry retail
  content — that is a policy, not only a path check.

## 8. Deliberate differences and unknown originals (F31-AC06)

- **Intentionally changed (enhanced policy, not gaps):** arcade handling
  and steering, modern input, native rendering, engine-to-engine netcode,
  and the designed catch-up/recovery policies. Listed in
  `docs/vehicle-handling.md` and as `designed` rows in
  `docs/original-rules.md` (117 `DSN-*` decision rows at this commit).
- **Unknown originally:** 45 open `UNK-*` entries. Provisional behaviour
  satisfies none of them; the ledger is the open set.
- This report, `docs/coverage-audit.md`, `docs/original-rules.md` and
  `docs/{vehicle,race,traffic,multiplayer}-coverage.md` together keep the
  code-checked, original-data-verified, rendered/played and
  intentionally-changed categories separate.

## 9. F31 acceptance-criteria verdicts

Honest status. "Met" means the evidence named exists and is recorded here
or in the linked page; "open" means a required leg is missing.

| AC | Status | Evidence / what is missing |
|---|---|---|
| **F31-AC01** | met as a mapping | `docs/coverage-audit.md` §3 maps every audited F00–F30 feature to evidence or an explicit remaining gap; re-confirmed at `d42e6ef`. No feature is unaccounted for. |
| **F31-AC02** | partial | Honest expected-vs-discovered-vs-rejected counts exist for vehicles, races, lessons, placement, audio and cities (§3). The per-family expected / discovered / tested / rejected / unverified table is recorded in [coverage-audit.md §2, "Expected versus tested (F31-AC02)"](coverage-audit.md#expected-versus-tested-f31-ac02) (#1496) at retail-data level only (headless parse/build/decode through the production consumer). The crash-course gap, 42 expected against 26 tested (13 per city), is stated there as unreconciled. No driven or played counts exist; families with no authored roster stay `unverified`. |
| **F31-AC03** | partial | Single-player, lesson and Cops & Robbers journeys and the lossy-link multiplayer journey pass on production systems (§5). An *eligible* retail reward leg (the matching OWNER task is #1175) and a retail-data multiplayer journey (#1493) are open. |
| **F31-AC04** | partial | Named negatives exist: the scripted driver's finish is record-ineligible (`DSN-16`); multiplayer legs use separate OS processes, not bots-as-clients; malformed override data is refused, not silently substituted. A full audit that no screen is a placeholder (F21's unrecovered evaluators are a real gap, not a placeholder) is not recorded. |
| **F31-AC05** | partial | The three code gates pass at `d42e6ef` (§2.1); the platform matrix and packaging steps are documented (§7); selected original-content runs are recorded (§2.2, §5). Hosted CI on this exact commit, and rendered/audio playtests, are open. |
| **F31-AC06** | met for the report | This page plus the linked coverage docs separate code-checked, original-data-verified, playtested and intentionally-changed behaviour; the playtested column is empty by fact (no human playthrough). |

F31 stays **open**: AC02–AC05 have missing legs, and §6's blockers are
feature-level, not documentation-level.

## 10. Handoff

- Merge the report alone does not close F31; close it when §6's blockers
  are filed as tasks and their evidence lands (see §9).
- To refresh this report: run §2.1–§2.3 at a new commit and update §3–§5;
  the denominators come from `mm2-inspect`, not from memory.
- Never push this report's retail-derived outputs (`target/f31c/`) or any
  retail asset into the repository.
