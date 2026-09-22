# Last iteration — F00-C.1 false-pass edge repair (absent player vs `Failed`)

Iteration 40 on `ralph/night`, continuing from `45b9ff9` (the F00-C.1
restart-follow candidate — external review verdict **fail**, one
blocking finding, on the candidate's own new code). This iteration is
the repair of that finding; no feature work.

## The defect being repaired

The reviewer's blocking finding: iteration 39's absent-player pass
branch failed only when the cap phase was in the live list
(`Countdown`/`Playing`/`Paused`/`Results`), so an absent player was
treated as legitimate for **every other phase — including the terminal
`Failed` phase**:

- `load_session_world` can land the session in `Failed` during a
  mid-run restart's reload (session.rs city-load error / event-load
  error — both run inside `Loading` on every `begin`), and a failed
  load returns before the player spawns (`if !world_ok { return; }`).
- Once at `Failed` the session parks: `drive_session` only takes
  `Failed → Unloading` on a queued quit/restart intent, and nothing
  queues one after a dev/disabled restart (`dev_restart_once` fires
  once, on `Playing`).
- At the frame cap `player` is `None`, `Failed` was not in the
  live-phase match, and `finite = player.is_none() || …` evaluated
  true — so the record emitted `status=pass … phase=failed
  moved=none final=none`, violating the module's own contract ("a
  load failure lands in `Failed` and reports `status=fail`").

## What changed

`crates/mm2_app/src/smoke.rs` only:

- New `absent_player_is_transient(phase, teardown_queued, restarts)`:
  an absent player at the cap is legitimate **only** inside the
  transient teardown/rebuild window — `Unloading` always; `Menu` only
  while a quit/restart intent is still queued (a `Menu` cap with
  nothing pending is parked: a rejected re-begin or a consumed quit
  leaves it there); `Loading` only once a re-begin bumped the
  generation (`restarts > 0` — the first load never reaches the frame
  loop). `Failed`, `Ready` and the live phases are never windows.
- The cap check now fails `status=fail` with `session failed:
  <reason>` on a `Failed` cap (the reason rides the record like the
  initial-load `load:` failure) and `no player vehicle` on any other
  non-transient phase — parked `Menu`, `Ready`, or a live phase
  missing its driver.
- The `finite`/`moved=none`/`final=none` legs are unchanged: absent +
  transient still reports the lifecycle window, and a live entity
  with missing/non-finite components still fails `non-finite pose`.

## Root cause classification

Implementation defect in the evidence runner (not a test-expectation,
dependency or original-rule issue). The prior diff widened
"legitimate absence" from *no cases* to *all non-live cases*; the
correct set is the transient teardown/rebuild window only.

## Tests

- `smoke.rs` `mod tests` +1:
  `absent_player_is_transient_only_in_the_teardown_window` — the
  decision matrix: `Unloading`/`Menu`+intent/`Loading`+restart pass;
  `Failed` (with and without a queued intent), parked `Menu`,
  first-load `Loading`, `Ready`, and all four live phases fail.
- A `Failed`-at-cap or parked-`Menu` cap cannot be produced
  end-to-end in the synthetic suite: a reload is deterministic on the
  same config/VFS, so a session that loaded once cannot fail its
  reload without real data faults; the unit matrix is the regression
  net (disclosed, not disguised).

## Gates

- `cargo fmt --all -- --check` — pass.
- `cargo clippy --locked --workspace --all-targets --all-features
  -- -D warnings` — pass.
- `cargo test --locked --workspace` — pass (all suites, 0 failures).

## Evidence (retail `fnv1a64:e91e6cd4b2ae30d9`, dev build)

- `london --event checkpoint:0 --bot --headless --frames 1500` (the
  originally flagged run): `status=pass updates=1500 ticks=0 rs=1
  phase=countdown final=(-448,-0.2,-187) race=Countdown{34} cp=0/5
  pos=5/5 opp=0/4` — the second session still sits mid-countdown on
  the authored slot; pass legs unaffected.
- `--dev-world --headless --restart --frames 600`: `status=pass
  ticks=1194 rs=1 phase=playing` — matches the reviewer's own
  reproduction (ticks=1194).
- `--dev-world --headless --frames 600` control: `ticks=1200`, no
  `rs=` field — restart-free records stay bit-identical.
- `sf --car vpbug --bot --headless --frames 600`: `status=pass
  impacts=12 dmg=3a/0d/0r rej=3 dup=0 vsk=6a/0d/0r spk=6b/20e/18x` —
  bit-identical to the F15-B.4/F00-C.1 records.
- The fail leg (`status=fail … phase=failed` or a parked `menu` cap)
  has no observed run — it needs a mid-run reload to actually fail,
  which the retail VFS does not do on demand. Verified by the unit
  matrix only.

## Remaining gaps (unchanged from iteration 39)

- The `moved=none`/`final=none` formatting leg still has no observed
  run — a cap landing inside the teardown window needs `--frames` to
  land on a 1–3 update window, and dev-world runs that short fail
  earlier on `never grounded`/`car never drove` (honestly — a
  2-frame run cannot prove grounding).
- Whether the london `checkpoint:0` bot *should* disable this often
  is F15-B's tuning question, not a record defect.
- The `sf checkpoint:0` "fell through the world" altitude-threshold
  false-positive class remains open (separate record-honesty issue).

Files: `crates/mm2_app/src/smoke.rs`, `docs/ralph/PLAN.md`,
`docs/ralph/LAST_ITERATION.md`.
