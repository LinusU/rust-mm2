# Last iteration — F13-C.2: parked stationary-control driver + first Professional legs

Task slice on `ralph/night` (baseline `598e672`, the externally checked
F13-C.1 commit). Selected the F13-C remainder's two named evidence gaps
from the passed review: no true stationary-player control (the `Hold`
leg drives blind at full throttle) and Professional rosters audited but
never driven. One small code change unlocks both.

## What changed

Commit `a7d2797` — `crates/mm2_app/src/{input,smoke,main}.rs`,
`tests/smoke.rs`, `README.md`:

- **`Driver::Parked` / `--parked`** (conflicts `--bot`): a
  `input::ParkedDrive` resource gates `parked_drive`, which writes
  `VehicleInput { handbrake: 1.0, .. }` on the player vehicle every
  frame — the foot brake is the reverse throttle once stopped, so the
  handbrake is the parked state. Same resource-gated pattern as
  `ScriptedDrive`, identical in the windowed app and headless smoke
  (chained after the scripted driver so a test holding both markers
  stays deterministic).
- The dev-world `car never drove` verdict is inert under Parked — a
  driver that never requests motion cannot fail it legitimately;
  `moved=`/`peak=` staying at zero *is* the parked evidence. Records
  name the leg `driver=parked`.
- Test: `dev_world_parked_driver_stays_parked` — pass + `driver=parked`
  + `peak<5` + `moved<5` where `Hold` drives off.

## Verification

Retail install `fnv1a64:e91e6cd4b2ae30d9` (read-only), Apple M1,
`--frames 12000` headless, commit `a7d2797`, 2026-09-24 — published in
`docs/race-coverage.md`:

- **Parked control (Amateur), 7/7 `status=pass`** on the events where
  C.1 saw opponent finishes — local `cp=0` on 6 of 7, `pos=` last
  everywhere, `dup=0`: london-0 `opp=3/4 F`, sf-0 `opp=4/6 F`,
  sf-2 `1/6 F`, sf-3 `2/5 F`, sf-4 `1/5 F`; london-2/sf-5 progressed
  (omax 4/5) without finishing. **11 opponent finishes with ledger
  results while the local participant contributed nothing** — the AC04
  control leg. sf-0's `cp=1/6`/`moved=8m` is the parked car shoved
  through a trigger by opponent contact — a pushed crossing is a real
  crossing, disclosed.
- **Professional scripted legs (3):** sf-0 `finished place=6/7` behind
  5 opponent finishes (pro `vpcaddie`/`vpbug` field, `env=lt01`; Pro
  wires `6opp 6rt` clean — the `6opp/7tbl` anomaly is amateur-only),
  sf-3 `finished place=1` (`vpvwcup`/`vpbullet`/`vpauditt`, `lt06`),
  london-0 `rs=2` still racing at cap (`vpcoop2k` ×6) — Pro measurably
  selects different authored rosters and conditions vs Amateur.
- **Gates:** `cargo fmt --all -- --check` clean; `cargo clippy
  --locked --workspace --all-targets --all-features -- -D warnings`
  clean; `cargo test --locked --workspace` green at `a7d2797`.

## Not done / open

- Professional coverage partial: 3 scripted Pro legs; the other 21
  events' Pro runtime + all Pro hold/parked legs open.
- The parked control is stationary, not physics-frozen — the field
  shoves it (`moved` up to 8 m, `peak` 4.7 m/s impact-only).
- sf-8's ~8-12 updates/s CPU-bound leg undiagnosed; deep-course budgets
  and any retail-fidelity comparison remain open (engine self-metrics
  only).
