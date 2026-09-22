# Last iteration — F18-A.3 authored per-preset fog tables

Iteration 46 on `ralph/night`, continuing from `c66922e` (the F17-A.6
menu condition-options candidate — external review verdict **pass**,
non-blocking nit: PLAN.md's "Tests +5" vs 7 itemized, fixed here).
TASKS.json's F18-A remainder listed `.sky` dome/fog mapping; while
surveying the environment files the audit had never counted, two
authored per-city fog tables surfaced — `city/london_fog.csv` and
`city/sf_fog.csv` (plus the `sf_fog_orig.csv` extra) — matching
MM2Hook's recovered `lvlSky::FogColors[16]`/`FogNearClip[16]`/
`FogFarClip[16]` 1:1 on the same `tod*4 + weather` slot grid the
`.ltNN` presets use. That is the F18-A remainder's fog leg, so this
slice lands the parser, the audit census/cross-check, and the session
camera binding. F18-A stays **active** — `.sky` dome, `.cpvs`
variants, precipitation/wetness/audio and replication remain open.

## What changed

- `mm2_formats::fog`: `FogTable`/`FogRow`/`FogIssue` — parses the
  authored header (`fog red,…,description (ignored)`) plus 16
  `r,g,b,start,end,label` rows; malformed lines become
  `TableDiagnostic`s, `validate()` reports row-count/non-finite/
  negative/out-of-range-colour/`end <= start` findings. Verified on
  retail: both cities carry exactly 16 rows and every row's label
  equals the `.ltNN` block name at its slot (16/16, audit-enforced —
  the header's "(ignored)" tag confirms positional reading).
- `mm2_app::environment`: `FogReport`/`FogSpec` on
  `EnvironmentReport`; `spawn_environment` reads
  `city/<stem>_fog.csv` through the VFS at the same effective slot the
  lighting preset uses — authored-event and `SessionCustomization`
  precedence (DSN-29) apply to fog identically, and a fallen-back
  lighting preset still binds its fog row. Failure policy is explicit:
  `missing`/`unparseable`/`no row for slot`/`degenerate` →
  `fog.absent` names the reason and nothing binds — never a
  fabricated default; table anomalies count into `fog.issues`.
- `mm2_app::session`: `load_session_world` attaches
  `DistanceFog{ FogFalloff::Linear{start,end} }` — authored distances,
  inferred curve shape (the recovered field names describe
  near/far clips of a fixed-function linear fog) — plus authored
  RGB → `Color::srgb` to both session cameras (chase + free),
  `SessionEntity`-stamped so it despawns with the session.
- `mm2-inspect weather`: `_fog.csv` joins the census — 105 files
  (expected denominator +2 canonical tables; `sf_fog_orig.csv` is an
  audited extra, parsed but not cross-checked since its labels use a
  different convention and its bands read as an earlier authored
  revision). New cross-check: each canonical row's label vs the
  `.ltNN` name at the same slot — prints
  `fog: city/<stem>_fog.csv — 16/16 row labels match`.
- `env=` smoke detail gains ` fog=<start>-<end>` or ` fog=none`.

## Deferred (kept open deliberately)

- `.sky` dome geometry — the sky clear-colour is not fogged, so
  foggy presets show authored-fog geometry against a designed-blue
  sky (recorded in `environment.md`; the dome leg is the fix).
- `.cpvs` numbered variants (fog/detail-variant hypothesis —
  `lvlSky` may switch them per weather), `.ldef`/`.pvshist`/
  `.lmap`/`.water` consumers — UNK-24 stays open.
- The exact original fog curve — `FogFalloff::Linear` is an
  implementation reading of the recovered clip names, not recovered
  semantics.
- Precipitation, wetness/traction (F06-B's `--traction` override
  remains the interim), weather audio (F18-B/C scope), authoritative
  condition replication (F18 req 5), `sf_fog_orig.csv`'s role.

## Verification (this tree)

- `cargo test -p mm2_app --test environment` — pass, 8 tests.
- Tests +9 total: `tests/environment.rs` ×3 —
  `authored_fog_binds_onto_the_cameras` (slot row → path/RGB/linear
  band asserted, both cameras carry `DistanceFog`),
  `authored_event_conditions_select_the_fog_row` (event slot 6 beats
  configured (0,0)), `degenerate_fog_row_binds_nothing`
  (`end == start` → no fog, `absent="degenerate"`, 1 issue); the
  existing `missing_preset_reports_the_fallback` now also asserts
  `fog=none`/`absent="missing"`; `mm2_formats::fog` unit tests ×6
  (retail shape, positional indexing, headerless data, malformed
  diagnostics, empty error, `validate()` findings).
- `cargo fmt --all -- --check` — pass.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — pass.
- `cargo test --locked --workspace` — pass, 64 suites / 0 failures.

## Retail evidence (fingerprinted install `fnv1a64:e91e6cd4b2ae30d9`)

- `mm2-inspect weather` → 105/105 parsed, 0 failures, 0 issues;
  `fog: city/london_fog.csv — 16/16`, `fog: city/sf_fog.csv — 16/16`
  row-label cross-checks; `sf_fog_orig.csv` counted as an extra.
- `sf --headless --frames 60` → `status=pass
  env=lt00(clear-morning) fog=650-1000` — sf's authored band.
- `london --headless --frames 60` → `status=pass
  env=lt00(clear-morning) fog=220-320` — london's own authored band
  (different data, correct per-city selection).
- Rendered pair at frozen `--cam=-747.5,42.4,275.0,179,-15`
  (Metal/Apple M1): `--weather 2 --time-of-day 1` (sf `foggy-noon`,
  authored 10–120 m) dissolves the skyline into the authored grey
  while `--weather 0 --time-of-day 1` (`clear-noon`, 600–1000 m)
  stays fully resolved — the authored bands visibly discriminate.
  PNGs inspected; not committed (original-content captures stay
  local).

## Ledger / docs

- `docs/original-rules.md`: WLD-24 (the fog table — verified
  original), DSN-30 (the binding's design classifications —
  `FogFalloff::Linear`, both-cameras policy, clamp, explicit
  no-default failure policy); WLD-21 census updated (105/105);
  DSN-28's stale "no fog exists yet" consequence corrected; UNK-24
  narrowed.
- `docs/research/environment.md`: `_fog.csv` section (format,
  measured mapping, retail bands, `sf_fog_orig.csv`), census
  102→105, cross-check list, runtime-consumption section extended
  to A.3 with the honest sky-mismatch note.
- `docs/ralph/PLAN.md`: F18-A.3 row; F18-A parent updated; F17-A.6's
  "Tests +5" → +7 (external-review nit).

## Not done / blockers

- No original-executable comparison of the fog curve or per-weather
  `.cpvs` variant switching (retail binary not runnable; both stay
  UNK-24).
- The foggy-preset screenshots show the known sky/geometry mismatch
  — geometry fogs to the authored colour while the clear-colour sky
  stays designed blue until the `.sky` dome leg lands.
