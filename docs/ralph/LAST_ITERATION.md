# Last iteration — F13-C.1: Checkpoint-catalog runtime matrix + honest outcome field

Task slice on `ralph/night` (baseline `55017f8`, the reviewed F02-C.5
commit). Selected the F13-C remainder — the spec's last open leg is
"validate the full catalog and playable races with actual opponents":
the runtime pieces (progress, results, opponents, restart) already
existed with synthetic coverage, so the highest-value slice was the
catalog-level behavioral matrix on the real install, not another
rewrite.

## What changed

`crates/mm2_app/src/smoke.rs` (commit `1791df0`):

- **Smoke `outcome=`/`place=` could borrow another participant's
  result.** `result_outcome`'s fallback surfaced the field leader's
  ledger entry when the local participant was still racing — observed
  live on `sf checkpoint:0` reporting `cp=2/6 pos=7/7` *and*
  `outcome=finished place=1`. The record now resolves the result by
  local participant identity and omits the field when the local
  driver hasn't resolved; opponent finishes remain visible via
  `opp=`/`opps=`. Unit regression coverage for the leader-fallback
  case rides along.

## Verification

Evidence on the fingerprinted retail install
(`fnv1a64:e91e6cd4b2ae30d9`, read-only), Apple M1 / Metal, commit
`1791df0`, 2026-09-24 — full write-up in `docs/race-coverage.md`:

- **Structural legs:** `mm2-inspect events` — 45 rows/city, all 24
  Checkpoint rows `ready`; `race-defs` — 64 builds/city, 0 failed;
  `opponents` — 271 + 246 slots wired, `--strict` exits 2 on 79
  authored anomalies (77 orphan `.opp` routes, sf/race0 `6opp/7tbl`,
  stunt0 dead ref) — disclosed, not filtered.
- **Runtime matrix:** 48 legs = 24 events × scripted/`Hold` drivers,
  `--headless --frames 12000`, Amateur. **48 `status=pass`** — the
  sf-8 scripted leg was a wall-clock outlier (~24 min at ~8-12
  updates/s under load; `dropped=53955`, one transient 17 km/s
  velocity spike, finite pose).
- **Opponents race:** 18 opponent finishes with ledger results across
  7 events (london-0/2, sf-0/2/3/4/5); progress on every
  uninterrupted generation; 3 local finishes (london-0 place 3,
  sf-3/sf-4 place 1).
- **Restart soak:** restart loops up to `rs=25` (hold london-10) with
  `dup=0` — the authored damage→restart path exercised hard with no
  result duplication; countdown-loop rows (scripted london-6/8, hold
  sf-8) are driver wrecks, kept in the table.
- **Gates:** `cargo fmt --all -- --check` clean; `cargo clippy
  --locked --workspace --all-targets --all-features -- -D warnings`
  clean; `cargo test --locked --workspace` green at `1791df0`.

## Not done / open

- Amateur difficulty only; Professional rosters audited, not driven.
- `Hold` drives blind at full throttle — not a parked control; a true
  stationary-player leg needs a new driver mode.
- London 4-11 opponents grind (omax 2-5 gates, heavy escapes) even
  uncontaminated — F15-B skill residual; retail-difficulty comparison
  unverified.
- Physics-step drops under contention on five runs (max `dropped`
  55917); three end-of-run cars not fully grounded — disclosed in the
  matrix.
- No original-fidelity claim: engine self-metrics only.
