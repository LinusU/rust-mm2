# Last implementation iteration

- Task ID and title: F17-A.3 — driver-name text entry: the Profiles
  screen's `New driver` row now opens a real typed-name field instead
  of auto-naming `Driver N`. Chosen from the plan's named F17-A
  remainder: it was the one piece with no external dependency —
  weather/time controls are F18-blocked, mouse nav is a larger input
  rework, and the AC05 audit is a survey, not a slice.
- Starting commit: `4f963925a609f100e7231317a571ed2caad0f2b2` on
  `ralph/night`; tree was clean (F17-B.3 externally checked pass).

## What changed

- `crates/mm2_app/src/menu.rs`
  - `Screen::NewProfile { name }` holds the live buffer;
    `MenuCommand::Type(char)` / `MenuCommand::Erase` added. The
    `New driver` row pushes the entry screen instead of creating a
    profile inline.
  - `MenuShell::apply` handles the entry screen before the row-nav
    match: `Type` appends after an ASCII/control-char check (the
    bundled font can't draw more — a stored name that renders as tofu
    is worse than a refused char; the refusal is a status line) and
    the store's `MAX_NAME_CHARS` bound; `Erase` pops a char; `Back`
    cancels; `Activate` trims and refuses empty/whitespace with `type
    a name first`, else calls `create_profile`.
  - `create_profile` runs the same store `resolve(ProfileRequest::
    Create)` path the CLI uses (rank = the menu difficulty, Standard
    kind): success binds (`MenuEffect::Bind`), refreshes the list and
    pops back to Profiles with `created <name>`; failure keeps the
    entry screen open with the buffer intact and the reason visible.
  - `menu_input` drains `MessageReader<KeyboardInput>` every frame —
    on the entry screen `KeyboardInput.text` (OS-resolved chars:
    layout, Shift, dead keys, held-key repeat) emits `Type` per
    non-control char and `key_code == Backspace` emits `Erase`; on
    every other screen the stream is still drained and discarded so a
    nav key's text (WASD carries text) can't leak into a freshly
    opened field. Nav bindings are off on the entry screen: Space is
    a character, arrows move no focus, Enter→Activate, Esc→Back, and
    pad South/East map to Activate/Back.
  - `menu_present` draws the field line `  Name: <buffer>_` plus a
    per-screen footer; `screen_title` names it `New driver`.
- `crates/mm2_app/src/pause.rs`, `crates/mm2_app/src/results.rs` —
  exhaustive `MenuCommand` matches ignore `Type`/`Erase` (no text
  fields there).
- `crates/mm2_app/tests/menu.rs` — harness registers `KeyboardInput`
  messages; helpers `key_text`/`type_text`/`erase`/`name_buffer`/
  `menu_texts`; the existing `profiles_bind_create_and_delete` create
  leg now types `Dave`; +3 tests (14 total):
  `new_driver_entry_types_edits_and_binds` (types `Ada Lovelacex`,
  Backspace → `Ada Lovelace`, asserts the drawn `  Name: Ada
  Lovelace_` field line, Enter binds and persists the typed name in
  the store), `new_driver_entry_refuses_empty_and_cancels` (empty and
  whitespace-only Enter refused with reason; Esc cancels; reopening
  starts empty — cancelled text doesn't linger),
  `new_driver_entry_bounds_and_filters_input` (non-ASCII refusal
  names the reason, arrows inert, Space types, buffer stops at
  `MAX_NAME_CHARS` = 32).
- `README.md` — menu-controls paragraph documents the name field.
- `docs/ralph/PLAN.md` — F17-A row + F17-A.3 row; open-items list.

## Evidence

- `cargo test -p mm2_app --test menu` — 14 passed, 0 failed.
- `cargo fmt --all -- --check` — PASS.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — PASS.
- `cargo test --locked --workspace` — PASS, all 46 suites, 0
  failures.
- Retail-rendered: `--mm2-path /Users/linus/coding/rust-mm2/retail
  --menu --frames 90 --screenshot /tmp/mm2-menu-f17a3.png` →
  `smoke=visual world=menu status=pass bytes=172070` on Metal/Apple
  M1 — PNG inspected, root menu intact. The entry screen itself is
  interactive-only: input is frozen during `--frames` captures, so
  the drawn field is verified by the UI-tree assertion in
  `new_driver_entry_types_edits_and_binds` rather than a screenshot.
- Retail physics sanity: `--city london --event blitz:0 --headless`
  → `status=pass` (600 updates, wheels 4/4).

## Still open

- F17-A remainder: per-event weather/time/density controls (need
  F18's session-legal writers), mouse navigation, and the
  original-menu capability audit vs F17-AC05.
- The name field is keyboard-only — a gamepad-only user can cancel
  but can't type (no on-screen keyboard yet; tracked as part of the
  F17 input-work remainder, not a hidden gap).
- Non-ASCII names are refused (font limitation, implementation
  choice). If a broader font ever ships, widen the filter to what it
  can draw rather than silently keeping ASCII.
- Retail `sf --event checkpoint:0 --headless` (hold driver) →
  `status=fail … fell through the world` at ~135 m. Unrelated to
  this change — `menu_input` doesn't run in a direct `--event`
  launch (`MenuShell` isn't inserted) — and it's the same class as
  the eight `sf` fall-throughs already recorded under `--bot` in the
  F15 smoke matrix (incl. `checkpoint:0`); the physics/content cause
  is still open there.
- F16-C remainder: AC01's process-level leg (two real interactive
  launches completing an event) still needs a playable session.
