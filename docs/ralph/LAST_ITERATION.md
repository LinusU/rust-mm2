# Last implementation iteration

- Task ID and title: F10-B.3 — bounded stuck recovery for ambient
  traffic (spec req 3's "obstruction response and stuck recovery";
  F10-AC05's "stuck-car outcomes" reporting leg). Selected per the
  selection policy's F10-B remainder: a car penned forever — behind a
  parked participant, a queue that never drains, an `AlwaysStop` end,
  a permanently occupied junction exit — previously waited
  indefinitely; the only recovery was the 400 m distance recycler.
- Starting commit: `ff80f1dcc29bafb3f135ac81fb916c7b6dfc3ce7` on
  `ralph/night`; tree was clean, previous external review verdict pass
  (F10-B.2), so this is feature work, not a repair.

## What changed

- `mm2_game::traffic` — new `StuckPolicy` (`window_ticks` 4800 = 40 s
  at 120 Hz, `min_displacement` 4 m; designed values, UNK-12 like
  every ambient constant) and `StuckWindow` (anchor + still-ticks).
  The window is sized past the worst *legitimate* wait this
  controller can impose — a junction's longest red is
  `(members − 1) × (green + clear)`, ~2520 ticks at four members —
  so a real signal hold never trips it. Progress ≥ `min_displacement`
  re-anchors (the test is "cannot get anywhere", not "moved slowly");
  a non-finite pose resets rather than counting as a stall.
- `mm2_app::traffic` — `AmbientCar` carries a `StuckWindow` seeded at
  spawn; `drive_ambient` expires it after the post-move position
  write: despawn + `junctions.depart` (queue shedding like the
  dead-end/recycle paths) + `traffic.stuck += 1`. The recovery is
  removal into the pool `maintain_ambient` refills — bounded, and
  never a teleport through whatever pens the car (the spec's explicit
  bar). `AmbientTraffic.stuck_policy` is `pub` so tests/evidence runs
  can bind a shorter window; `stuck=` joins the `traf=` smoke record.
- Recovery trade-off recorded in code: a multi-cycle queue tail at a
  heavily loaded signal *can* outwait the window — that car is
  sacrificed to the recycler so the queue drains instead of freezing
  the population.

## Evidence

- `cargo test -p mm2_game --test traffic` — 21 pass (+1:
  `stuck_window_resets_on_progress_and_expires_stationary` —
  re-anchor on progress, just-under progress keeps counting, expiry
  on the bound tick stays expired, NaN resets).
- `cargo test -p mm2_app --test traffic` — 15 pass (+2:
  `a_penned_car_is_recycled_after_the_stuck_window` — a parked
  participant pens a follower; with `window_ticks=240` it despawns
  after the bound having never closed inside 4 m, and a second
  penned follower recovers identically, `stuck` counting each;
  `a_signal_wait_shorter_than_the_window_never_recovers` — a
  1200-tick window over a ≤360-tick red: the car stands, crosses on
  green, `stuck=0`).
- `cargo fmt --all -- --check` — PASS.
- `cargo clippy --locked --workspace --all-targets --all-features
  -D warnings` — PASS.
- `cargo test --locked --workspace` — PASS, 49 suites, 0 failures.
- Retail headless smoke (install `fnv1a64:e91e6cd4b2ae30d9`):
  - `--city sf --frames 600` → `status=pass … traf=16/16 sp=23
    rec=7 dead=0 uns=0 q=0 jq=5 stuck=0` — prior counters unchanged.
  - `--city london --frames 600` → `status=pass … traf=16/16 sp=21
    rec=5 dead=0 uns=0 q=0 jq=3 stuck=0`.
  - `--city sf --frames 3000` (6000 ticks > the window) →
    `traf=16/16 sp=37 rec=21 dead=0 uns=0 q=0 jq=5 stuck=0` — a
    moving player never pens a car, so no organic stuck outcome was
    observed on retail; the recovery path is synthetic-verified.
  - `--city london --spawn=0.4,5.5,-720,0 --frames 3000` →
    `status=pass … moved=44m … stuck=0` (opportunistic pen attempt
    on the pedestrianised street; the player kept bumping around,
    never penned a car for the window).

## Still open

- Stuck-window constants are designed — original stuck/despawn
  behaviour unverified (UNK-12). Despawn near the player is a
  visible pop (no occlusion check); queue-tail sacrifice at heavily
  loaded signals is a designed trade, not an original rule.
- F10-AC02 remainder: no yielding to crossing traffic inside the
  box (a transfer still cannot see same-tick transfers); F10-AC04:
  spawn-vs-spawn/spawn-vs-participant overlap unchecked;
  F10-AC03: kinematic followers stop short, no collision fidelity
  or lane-change passing. Multiplayer union-of-interest bubbles,
  signal-prop rendering, rendered/GPU junction checks all open.
- A car mid-approach when its phase flips red still freezes wherever
  it stands (bounded, safe, original clear-the-box behaviour
  unverified).
