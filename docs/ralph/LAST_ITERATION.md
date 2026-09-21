# Last implementation iteration

- Task ID and title: F17-A.1 review repair — the menu had no render
  target, so its `bevy_ui` tree never drew. Selected per the recovery
  prompt: repair the current external-review blocker before any new
  feature work.
- Starting commit: `79ae93a9cf1a3833e7c45351bcc0061c3010897e` on
  `ralph/night`; tree was clean.

## Root cause

`bevy_ui` renders per camera view. The only cameras in the app are the
two `Camera3d`s `load_session_world` spawns stamped `SessionEntity` —
despawned on every session teardown and absent while the session parks
at `Menu`. `menu_present` built a full `Node`/`Text` tree but no
camera existed to draw it, at boot and after every quit-to-menu. The
headless suite could not catch it: `MinimalPlugins` has no renderer.

## What changed

- `crates/mm2_app/src/menu.rs`
  - `MenuCamera` marker: `menu_present` keeps exactly one `Camera2d`
    alive while `shell.active` — spawned when absent, stable across
    `dirty` redraws (the `MenuUi` text tree still rebuilds per redraw),
    despawned with the rest of the menu when a session takes the
    screen.
  - The UI root is pinned to it via `UiTargetCamera`, so the tree can
    never silently fall back onto a session camera (or onto nothing).
  - Second defect found through the new rendered evidence: the bundled
    font lacks `›`, `•`, `—`, `·`, `↑`, `↓` — the focus marker and
    disabled-row separators rendered as tofu. Every user-facing menu
    string is now ASCII (`> ` focus, ` *` selected, ` - ` separators,
    `Up/Down ...` footer).
- `crates/mm2_app/src/main.rs`
  - `--menu` flag: pairs with `--frames`/`--screenshot` so the existing
    capture harness can render the shell itself — the menu was
    previously unreachable by any evidence command, which is exactly
    why the defect slipped through. `world=menu` in the smoke record.
    Session-shaping flags still win (`--menu --city …` launches and
    warns); `--headless` conflicts. `menu_input` is frozen during
    captures like every other input system.

## Evidence

- Rendered: `./target/debug/mm2 --mm2-path
  /Users/linus/coding/rust-mm2/retail --menu --no-profile --frames 60
  --screenshot /tmp/mm2-menu.png` → `smoke=visual world=menu
  status=pass ... bytes=169503` on Apple M1/Metal; the PNG was
  inspected — title, focused `> Cruise`, `Vehicle: VW New Beetle`,
  dimmed `Driver`/`Options`/`Multiplayer` rows with their reasons, and
  the key-hint footer all draw. Capture kept out of git (local
  evidence only).
- `cargo test --locked -p mm2_app --test menu` — PASS (7):
  new `the_menu_draws_into_its_own_camera` (camera exists and is
  active at boot, UI roots target it, the entity survives a redraw,
  it is gone in-game while session cameras exist, and it returns on
  quit-to-menu); `the_app_boots_into_the_menu` and
  `cruise_launches_then_quit_returns_to_the_menu` gained camera-count
  assertions (1 at menu, 0 in-game — AC06's no-duplicate-cameras leg).
- `cargo fmt --all -- --check` — PASS.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — PASS.
- `cargo test --locked --workspace` — PASS, all 45 suites, 0 failures.

## Still open

- F17-A remainder: Quick Race (`last_event` launch), per-event
  weather/time/density controls (need F18's session-legal writers;
  RACE-3 `customizable` is already surfaced), mouse navigation, text
  entry for profile names, and the original-menu audit against
  F17-AC05's capability denominator.
- Remaining review verification gaps, unchanged: gamepad path is
  source-only; `menu_mode` flag selection is exercised only through
  the new `--menu` capture, not a dedicated test; Left/Right
  difficulty toggle and DriveProfileless lack coverage; a `Failed`
  session returns to a plain root menu without carrying the failure
  reason (F17-B territory).
- F17-AC01/02/04/05 remain unverified; the rendered evidence covers
  the root screen only — a play→reward→return capture leg and
  high-DPI layouts (spec req 6) are still missing.
- F16-C remainder: AC01's process-level leg (two real interactive
  launches completing an event) still needs a playable session;
  `--bot` finishes are deliberately ineligible.
