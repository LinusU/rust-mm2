# Last iteration — smoke-record restart repair + `--restart` (F00-C.1)

Iteration 39 on `ralph/night`, continuing from `bcaacae` (the
externally checked F15-B.4 catch-up candidate — review verdict
**pass**). Task id: `F00-C.1` — a repair of the evidence runner,
chosen over feature work because the external review flagged a real
failing observation on the production path.

## The defect being repaired

The reviewer's spot-check found `london --event checkpoint:0 --bot
--headless --frames 1500` reporting `status=fail "non-finite pose"`
with `race=Countdown{36}` and `ticks=0`, and classified it as a
pre-existing instability outside the F15-B.4 diff. Reproduced
deterministically at `bcaacae`; root cause is in the smoke harness,
not the physics:

- `headless_smoke` resolved the `PlayerVehicle` entity **once** before
  the frame loop. Mid-run the scripted driver accumulated damage to
  `DamageTier::Disabled`; for a Checkpoint event `resolve_disabled`
  applies the documented `RestartEvent` outcome (RACE-5/DMG-2), so the
  session travelled the production `Playing → Unloading → Menu →
  begin` path — which despawns every `SessionEntity` and spawns a new
  car.
- Every post-restart `world.get::<Position>(car)` then returned
  `None`; `unwrap_or(f32::NAN)` formatted `final=(NaN,NaN,NaN)` and
  the `finite` check failed. The rest of the record already told the
  truth: `ticks=0` (the clock only counts `Playing` ticks of the
  current generation), `race=Countdown{36}` (the *second* session's
  countdown), `impacts=0` (the per-session filter reset), no `pos=`
  (the stale `Player` lookup returned `None`).

So the flagged run was a legitimate disabled→restart lifecycle, not a
NaN — the record mislabeled it.

## What changed

- `crates/mm2_app/src/smoke.rs`: `headless_smoke` keeps a
  `PlayerVehicle` `QueryState` and re-resolves the live player entity
  every frame — the `Hold` driver's input write and the telemetry
  sample follow the respawned car. End-of-run `pos`/`vel`/`rot`,
  `local` participant, `progress`/`cleared` and `gyr=` all read the
  live entity. `rs=<n>` counts session restarts (`Session::begin`'s
  generation delta over the run), emitted on activity only so
  restart-free records stay bit-identical. When the frame cap lands
  inside the teardown window (no player entity at all) the record
  says `moved=none`/`final=none` — a lifecycle state, not a pose —
  while a live entity with missing or non-finite components still
  fails `non-finite pose`, and a *live* phase
  (`Countdown`/`Playing`/`Paused`/`Results`) with no player entity at
  all fails `no player vehicle` (absence is only legitimate inside
  `Unloading → Menu → Loading`).
- `crates/mm2_game/src/config.rs` + `crates/mm2_app/src/session.rs` +
  `crates/mm2_app/src/main.rs`: `DevOverrides::restart` (`--restart`),
  quarantined like `--pause`/`--finish`. `dev_restart_once` queues the
  session's own restart intent once on the first `Playing` frame —
  the same production lifecycle a disabled-in-event restart or a
  Backspace restart takes — giving evidence runs a reproducible
  restart trigger. Scheduled ahead of `drive_session` in both the
  windowed app and the headless smoke's Update chains; excluded from
  menu-mode's direct-launch tests.
- `crates/mm2_game/src/progression.rs`: `record_eligibility` rejects
  `dev.restart` (`DevOverride("restart")`) — a restarted run is not a
  continuous recorded run.

## Tests

- `tests/smoke.rs` +1 (4 total):
  `dev_world_headless_smoke_follows_player_across_restart` — a
  dev-world `headless_smoke` under `dev.restart` passes with `rs=1`,
  no `NaN`, `phase=playing` and a live `final=` pose at the cap.
- `mm2_game/tests/progression.rs` +1 leg: `dev.restart` →
  `Err(Ineligible::DevOverride("restart"))`.

## Gates

- `cargo fmt --all -- --check` — pass.
- `cargo clippy --locked --workspace --all-targets --all-features
  -- -D warnings` — pass.
- `cargo test --locked --workspace` — pass (63 suites, 0 failures).

## Evidence (retail `fnv1a64:e91e6cd4b2ae30d9`, dev build at `bcaacae+diff`)

- `london --event checkpoint:0 --bot --headless --frames 1500` (the
  flagged run): `status=pass updates=1500 ticks=0 rs=1 phase=countdown
  … final=(-448,-0.2,-187) race=Countdown{36} cp=0/5 pos=5/5 opp=0/4`
  — the second session sits mid-countdown on the authored slot; the
  live running order is populated again. Was `status=fail "non-finite
  pose"` on the same install before this diff.
- Same event `--frames 3000`: `status=pass rs=2 phase=playing
  ticks=288 pos=5/5 cu=3` — the bot disables repeatedly (damage →
  `RestartEvent` per the authored rule), each restart plays out, and
  opponent/catch-up counters work on the third generation.
- `sf --car vpbug --bot --headless --frames 600`: `status=pass
  impacts=12 dmg=3a/0d/0r rej=3 dup=0 vsk=6a/0d/0r spk=6b/20e/18x` —
  bit-identical to the F15-B.4 record (no `rs=` without a restart).
- `london --bot --headless --frames 600`: `status=pass impacts=7
  dmg=1a/0d/0r rej=4 dup=0 vsk=5a/0d/0r spk=5b/23e/23x` — bit-identical.
- `--dev-world --headless --restart --frames 600`: `status=pass
  ticks=1196 rs=1 … moved=157m` vs the unmodified `ticks=1200`
  baseline — the one-shot restart costs ~2 frames of session clock.

## Disclosures and gaps

- `rs=` is a generation delta over the whole run — it counts
  teardown/begin cycles regardless of cause (disabled restart,
  `--restart`, a queued restart intent). The record does not attribute
  the cause; `dmg=`/`vsk=`/`rcv=` counters describe the current
  session only (they reset on teardown, per the AC03 no-stale-timer
  rule).
- `final=none` covers only the teardown window itself; no observed
  run lands there deterministically, so that formatting leg is
  review-visible rather than run-proven.
- Whether the bot *should* disable this often on london
  `checkpoint:0` is a tuning question, not a defect: impacts are
  real collisions on the course and the disabled→restart outcome is
  the documented rule. The event never progresses under the scripted
  driver because it keeps wrecking — F15-B's difficulty/soak scope
  owns whether that is acceptable bot behavior.
- The `sf checkpoint:0` "fell through the world" altitude-threshold
  false-positive class (noted in earlier iterations) is untouched —
  a different, still-open record-honesty issue.

Files: `crates/mm2_game/src/config.rs`,
`crates/mm2_game/src/progression.rs`,
`crates/mm2_game/tests/progression.rs`,
`crates/mm2_app/src/session.rs`, `crates/mm2_app/src/smoke.rs`,
`crates/mm2_app/src/main.rs`, `crates/mm2_app/tests/smoke.rs`,
`docs/ralph/PLAN.md`, `docs/ralph/LAST_ITERATION.md`.
