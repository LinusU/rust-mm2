# Last implementation iteration

- Task ID and title: F17-A.2 — Quick Race (DRV-8): the root menu's
  `last_event` launch. Selected per the reconciled plan's ready-slice
  list; it is the highest-value F17-A remainder item with no F18
  dependency.
- Starting commit: `59bb69b8c3fb4d47513925d615db703de0efb6dc` on
  `ralph/night`; tree was clean.

## What changed

- `crates/mm2_app/src/menu.rs`
  - New `quick_race_row` builds a root row between Cruise and Events.
    With a bound profile carrying `selections.last_event`, the
    stem-keyed `EventKey` is resolved back through the live
    `EventCatalog` (`table` + `stem` match → `EventRef`) — never the
    saved row index, so a mod inserting/removing rows cannot retarget
    the save — and activates through the same `Action::LaunchEvent` →
    `Session::begin` path the event list uses, with the current
    vehicle/paint/difficulty selections.
  - Every unreachable leg is a disabled row naming its reason: no
    bound profile, no event played yet, a Crash Course key (events not
    loadable yet, F21), a missing `city/*.psdl`, a stem no longer in
    the catalog, an `Incomplete` record set, and the same availability
    gate the event list enforces (`beat <stems> first`).
  - Layout note: the documented original (help:Quick Race) inserts a
    vehicle-select screen between the pick and the launch. This shell
    keeps vehicle/difficulty as persistent root selections, so the row
    launches directly — recorded in the code comment as an
    enhanced-layout choice, not an original-rules claim.
- `crates/mm2_app/tests/menu.rs` — +2 tests (9 total):
  `quick_race_replays_the_last_event` (no-profile/fresh-profile
  disabled legs → bind → Events→race0 launch persists the stem key to
  the profile file → quit-to-menu → `Quick Race: Checkpoint #0
  (race0)` enabled → activation lands `SessionMode::Event` at the same
  `EventRef`), and `quick_race_reports_an_unresolvable_last_event`
  (gated race3 / incomplete race2 / unknown race99 each disable with
  their reason and never launch).

## Evidence

- Rendered: `./target/debug/mm2 --mm2-path
  /Users/linus/coding/rust-mm2/retail --profile-dir /tmp/mm2-qr
  --new-profile QR --city sf --event checkpoint:0 --headless --frames
  300` → `status=pass ... phase=playing ... profile=driver-0`, and the
  profile file on disk records `last_event = sf/checkpoint/race0`.
  Then `--profile-dir /tmp/mm2-qr --profile driver-0 --menu --frames
  90 --screenshot /tmp/mm2-menu-qr.png` → `smoke=visual world=menu
  status=pass ... bytes=172980` on Apple M1/Metal; the PNG was
  inspected — `Quick Race: Checkpoint #0 (race0)` draws enabled under
  Cruise with `Driver: QR (driver-0)` bound and `Vehicle: VW New
  Beetle` restored. Capture kept out of git (local evidence only).
- `cargo test --locked -p mm2_app --test menu` — PASS (9).
- `cargo fmt --all -- --check` — PASS.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — PASS.
- `cargo test --locked --workspace` — PASS, all 45 suites, 0 failures.

## Still open

- F17-A remainder: per-event weather/time/density controls (need
  F18's session-legal writers; RACE-3 `customizable` is already
  surfaced), mouse navigation, text entry for profile names, and the
  original-menu audit against F17-AC05's capability denominator.
- Remaining review verification gaps, unchanged: gamepad path is
  source-only; `menu_mode` flag selection is exercised only through
  the `--menu` capture; Left/Right difficulty toggle and
  DriveProfileless lack coverage; a `Failed` session returns to a
  plain root menu without carrying the failure reason (F17-B
  territory).
- F17-AC01/02/04/05 remain unverified; rendered evidence covers the
  root screen only — sub-screens, the play→reward→return leg and
  high-DPI layouts (spec req 6) are still missing.
- F16-C remainder: AC01's process-level leg (two real interactive
  launches completing an event) still needs a playable session;
  `--bot` finishes are deliberately ineligible.
