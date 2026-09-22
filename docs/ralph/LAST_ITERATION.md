# Last implementation iteration

- Task ID and title: F17-A.4 — mouse navigation for the menu (F17 spec
  req 5's mouse leg; the last concrete ready item in the F17-A
  remainder besides weather controls gated on F18 and the AC05 audit).
- Starting commit: `801ffd2fca9e719e8ad0a1d7aeb5c0fd97755dc0` on
  `ralph/night`; tree was clean, previous external review verdict pass
  (F10-B.8), so this is feature work, not a repair.
- Why this slice: the F10-B remainder is research-gated (junction
  priority, collision fidelity vs AC03, original timing — all UNK-12
  territory) or acceptance-evidence work; F17-A's mouse leg is a small
  coherent production change with a testable model seam.

## What changed

- `mm2_app::menu` — the mouse path produces the same `MenuCommand`s the
  keyboard/gamepad path produces; nothing touches session state
  directly:
  - `MenuCommand::FocusAt(usize)` — absolute focus for hover; a no-op
    (same row, off-list index) does not dirty the shell.
  - `MenuShell::pending` — a command queue device systems push into;
    `menu_input` drains it first, so hover/click commands execute
    through the one `apply`/`MenuEffect` loop. `reopen` and the
    inactive gate clear it — a stale click can't fire into a running
    session or a reopened menu.
  - `MenuRow { index }` — each row entity `menu_present` spawns carries
    its `shell.rows` index; title/status/footer lines carry none. Rows
    are now full-width `Node`s so the hit box covers the whole line,
    not just the glyphs.
  - `menu_mouse` (runs before `menu_input` in the menu chain): reads
    the primary window's cursor, maps logical → physical through the
    camera's target scaling factor (`window.scale_factor()` fallback —
    `computed.target_info` is render-side only and absent headless),
    clips to the camera viewport when resolved, and hit-tests
    `ComputedNode`/`UiGlobalTransform` rects — the same space
    `bevy_ui`'s picking backend uses (layout runs in PostUpdate, so the
    test is at most one frame stale).
  - Semantics: hover → `FocusAt(index)`, edge-triggered — only a
    *moved* cursor asserts focus, so a resting cursor never fights
    keyboard/gamepad navigation. Left-click → `FocusAt` + `Activate`
    (a disabled row surfaces its reason, never navigates). Right-click
    → `Back` from anywhere — the mouse's Esc.
- `mm2_app::main` — `menu_mouse` wired into the menu chain between
  `menu_watch` and `menu_input`, frozen under `capturing` like every
  other input path so `--menu --frames` captures stay reproducible.
- `mm2_app::pause` / `mm2_app::results` — their `MenuCommand` matches
  fold `FocusAt` into the no-op arm (overlay rows are not
  `MenuRow`-tagged; no mouse producer runs against them).

## Evidence

- `cargo test -p mm2_app --test menu` — 16 pass (+2):
  - `the_mouse_focuses_rows_and_clicks_drive_the_same_commands` —
    `MenuRow` entities map 1:1 onto `shell.rows`; empty space focuses
    nothing; a moved cursor over row 2 focuses it; ArrowUp then moves
    focus and a resting cursor does not re-assert; a click on Cruise
    drives `FocusAt(0)` + `Activate` → `Push(CruiseCity)`; right-click
    → `Back` to Root.
  - `a_click_on_a_disabled_row_shows_its_reason` — clicking the
    disabled Options row focuses it and sets the `not implemented yet
    (F23)` status through the normal `Activate` path; no navigation.
  - The harness simulates layout by stamping `ComputedNode`/
    `UiGlobalTransform` on the real `MenuRow` entities (headless has
    no UiPlugin) — `lay_out_rows` re-applies after any redraw because
    `menu_present` respawns the row entities on `dirty`.
- `cargo fmt --all -- --check` — PASS.
- `cargo clippy --locked --workspace --all-targets
  --all-features -D warnings` — PASS.
- `cargo test --locked --workspace` — PASS, 680 tests, 0 failures.
- Rendered leg unchanged by design: input is frozen during `--frames`
  captures, so the existing `--menu --frames 90 --screenshot` evidence
  from F17-A.3 still stands; the footer now notes `click works`.
  Interactive pointer capture (hover/click on screen) is manual
  verification, not headless evidence.

## Still open

- Hover/click is only proven through headless hit-test math and model
  effects; a rendered screenshot can't show a cursor. No claim of
  original parity — the original's exact mouse behaviour (hover
  highlight style, right-click semantics) is unverified; the
  edge-triggered hover is a designed coexistence policy.
- Pause and results overlays have no mouse path (out of scope for this
  slice — `FocusAt` is inert there); original overlay clickability is
  likewise unaudited.
- F17-A remains active: per-event weather/time/density controls need
  F18's session-legal writers (RACE-3 `customizable`); the original
  menu audit vs F17-AC05's capability denominator and F17-AC03's full
  keyboard/gamepad + visible-focus evidence remain open.
