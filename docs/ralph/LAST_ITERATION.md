# Last implementation iteration

- Task ID and title: F17-B.3 — countdown presentation: a
  session-owned centered banner that shows `3`/`2`/`1` off the
  authoritative `RaceState` countdown and a bounded `GO!` after
  release. Chosen from the plan's named candidates: it is the only
  F17-B remainder that doesn't depend on F17-C's unimplemented mode
  (C&R/scoring variants) and, unlike the F17-A remainder, isn't
  F18-blocked. Previously the countdown was only the `GET READY n`
  HUD line — functional but invisible as a race cue.
- Starting commit: `0b012356d40973f89199ebd86660041d91519357` on
  `ralph/night`; tree was clean (F17-B.2 externally checked pass).

## What changed

- `crates/mm2_app/src/race.rs` — new presentation-only banner beside
  the nav arrow and low-time warning:
  - `CountdownBanner` (root UI node marker) / `CountdownBannerText`
    (text child), `COUNTDOWN_GO_TICKS = RACE_TICK_HZ` (one race-clock
    second of `GO!`).
  - `spawn_countdown_banner` builds a centered, `SessionEntity`-
    stamped UI node; `update_countdown_banner` reads the current
    `RaceState` (generation-filtered, so a stale race can't drive it)
    plus `Session::phase` and writes `Visibility`/`Text`:
    `Countdown{remaining>0}` → `ceil(remaining / RACE_TICK_HZ)` as
    `3`/`2`/`1`; `Running` within `COUNTDOWN_GO_TICKS` while the
    session is `Playing` → `GO!`; `Paused`/`Results`/stale/no-race →
    hidden. The zero-length-countdown case shows only `GO!`.
  - No authority touched: `advance_race`, `input_locked`,
    `RaceStarted` emission, and phase transitions are unchanged; the
    banner never advances its own timer.
- `crates/mm2_app/src/session.rs` — `load_session_world` spawns the
  banner for event sessions (next to checkpoint markers/nav arrow/
  low-time warning); module doc repaired — `session_control_input`
  no longer claims the `Results` Esc (owned by `results_input`; the
  review's non-blocking nit).
- `crates/mm2_app/src/main.rs` — `update_countdown_banner` added to
  the Update tuple beside the other race overlays; `CountdownBanner`
  added to `HudNodes` so `retarget_hud` keeps it on the active
  camera.
- `crates/mm2_app/tests/race.rs` — harness spawns the banner +
  registers the update system; +3 tests (36 total):
  `countdown_banner_counts_digits_then_flashes_go` (`3`→`2`→`1`,
  `GO!` at release, hidden once the clock passes
  `COUNTDOWN_GO_TICKS`); `countdown_banner_go_belongs_to_a_live_playing_session`
  (zero-length countdown → `GO!`, pause hides, resume restores while
  the frozen clock stays in-window, `Results` hides);
  `countdown_banner_ignores_stale_race_and_despawns` (stale
  generation hidden, valid race restores, restart teardown removes
  the session-owned banner).
- `docs/original-rules.md` — DSN-19: digits/`GO!`/one-second window
  recorded as a designed presentation policy, deterministic off the
  race tick clock; no verified original timing claimed (the 3 s
  countdown default remains provisional).

## Evidence

- Rendered on Apple M1/Metal (`/Users/linus/coding/rust-mm2/retail`,
  `sf --event checkpoint:0`):
  - `--frames 60 --screenshot /tmp/mm2-countdown-3.png` →
    `smoke=visual world=city/sf.psdl status=pass frames=done
    bytes=4157777` — PNG inspected: digit `2` centered over the
    starting grid, HUD `GET READY 2` in agreement.
  - `--frames 200 --screenshot /tmp/mm2-countdown-go2.png` →
    `status=pass bytes=4543805` — PNG inspected: green `GO!`
    centered at 0.9 s race clock, cars released.
  - Captures kept out of git (local evidence only).
- `cargo test -p mm2_app --test race` — 36 passed, 0 failed.
- `cargo fmt --all -- --check` — PASS.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — PASS.
- `cargo test --locked --workspace` — PASS, all 46 suites, 0
  failures.

## Still open

- F17-B remainder: C&R/points result variants are F17-C scope (no
  such mode exists yet).
- The banner's digits/`GO!` look-and-feel and one-second window are
  designed (DSN-19) — no original-timing claim; the retail game's
  actual countdown presentation remains unmeasured (unknown).
- Overlay legibility at non-default camera modes/high-DPI unverified
  — same standing gap as pause/results overlays; verified render is
  the default chase camera at default scale.
- F17-AC02 (restart persistence) and F17-AC05 (original-menu
  capability denominator) remain unverified; AC06's in-session leg
  is exercised (banner is `SessionEntity`-stamped, asserted despawned
  on restart).
- F16-C remainder: AC01's process-level leg (two real interactive
  launches completing an event) still needs a playable session.
