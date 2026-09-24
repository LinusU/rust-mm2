# Last iteration — F15-B.7: per-opponent progress/recovery disclosure

Task slice on `ralph/night` (baseline `824defd`; the F15-B.6 external
review passed with no blocking findings). Selected the reviewer's named
verification gap: F15-B.6's "all four opponents clear gates (2/4/3/2)"
evidence came from temporary instrumentation removed before commit —
the committed smoke record could not say *which* opponents progressed
or what the recovery machinery did for them (F15 spec req 6). This
slice makes that evidence reproducible from committed code.

## What changed

`crates/mm2_app/src/scripted.rs`:

- `ScriptedBot.escapes` — counted each time the two-phase
  reverse-and-turn escape fires.
- The scripted player's route re-anchor preserves `escapes` across its
  `ScriptedBot` reset (previously dropped by
  `*bot = ScriptedBot::default()`).

`crates/mm2_app/src/opponents.rs`:

- `OpponentDriver.index` — the authored roster slot; a skipped vehicle
  load does not renumber the field.
- `OpponentDriver.stuck_peak` — the longest continuous spell inside one
  displacement bubble. A cumulative total was implemented first and
  rejected: a moving car's bubble re-seats every `REANCHOR_DIST` of
  travel, so a cumulative count approaches the race duration regardless
  of health. The peak is the meaningful signal — a progressing car
  peaks at a handful of frames, a penned one at `REANCHOR_FRAMES` —
  and it survives both the displacement-window reset and the re-anchor.
- The opponent re-anchor now preserves `recovery.escapes` the same way.

`crates/mm2_app/src/smoke.rs`:

- `opps=` — one row per spawned opponent sorted by authored roster slot:
  `<slot>:<vehicle>/<cleared>c[/<lap>l][/F|/T][/<escapes>e][/<reanchors>r][/<stuck>w]`.
  `cleared` is `RaceProgress::cleared_count` (per-lap under `Ordered`),
  `lap` uses the record's `lap+1` convention, `w` counts update frames
  inside the displacement bubble. Recovery subfields print only when
  nonzero.
- `p_rec=<reanchors>r/<escapes>e` — the scripted player's bounded
  recovery counters (previously `info!`-only), printed only on activity.
- A run with no opponents or no recovery activity emits a bit-identical
  record.

## Verification

- Tests +1 (`smoke::opponent_detail_reports_progress_and_recovery_per_slot`
  — roster-slot sorting over out-of-order entity collection, AnyOrder
  no-lap, `T` timeout, empty roster) plus new assertions in four
  existing tests: the penned-car end-to-end asserts the peak reached
  `REANCHOR_FRAMES` with counted escapes, the dispatch-path test asserts
  the peak survives the reset while `stuck_frames` restarts, the
  progressing-field test asserts peaks stay small-but-live, and the bot
  escape test counts the fired escape.
- Retail (`fnv1a64:e91e6cd4b2ae30d9`, headless):
  - `sf circuit:0 --bot --frames 9000` → `status=pass cp=4/9 lap=2/3
    pos=1/5 opp_rec=4 cu=4` with
    `opps=0:vpbug/6c/1l/13e/3r/900w,1:vpbug/6c/1l/4e/697w,2:vpbug/0c/2l/10e/872w,3:vpbug/6c/1l/15e/1r/900w`
    `p_rec=3r/2e` — `opp_rec` (4) equals the rows' `r` sum (3+1); slot 2
    wrapped to lap 2 while two penned cars show exactly
    `REANCHOR_FRAMES` peaks and slot 1 freed itself short of the bound.
  - `london circuit:0 --bot --frames 1500` → `status=pass cp=3/6
    pos=5/8 opp_rec=1` — identical headline to the pre-slice regression
    disclosure (the slice is observability-only) with all seven slots
    reporting progress and `p_rec=0r/1e`.
- Gates (this tree): `cargo fmt --all -- --check` clean;
  `cargo clippy --workspace --all-targets --all-features -- -D warnings`
  clean; `cargo test --workspace` — 67 suites, 0 failures.

## Not done / open

- F15-B parent's remaining items stand: AC06 measured-difficulty leg,
  representative avoidance matrix, authored-tail columns
  (`weirdPathfinding`/`distancePadding`/`cornerBrakingThreshold`),
  `avoidOpponents` polarity.
- The `w` field measures update frames inside the displacement bubble,
  not wall time; opponent recovery behavior itself is unchanged by this
  slice and remains a designed policy, not verified original.
- The London pacing regression stands (`cp=3/6` vs the pre-densification
  `cp=4/6`); this slice did not attempt to address it.
- GPU/rendered output not exercised — headless physics evidence only.
- Whether retail opponents actually route on the BAI network between
  `.opp` anchors remains an unverified inference (the densified line is
  our implementation choice, recorded as such).
