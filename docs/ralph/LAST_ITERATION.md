# Last implementation iteration

- Task ID and title: F17-A.1 — menu shell: profile/mode/content
  selection over the real catalogs, launch through `Session::begin`,
  quit back to the menu, and the F16-AC06 deliberate-delete
  confirmation leg. Selected from the plan's candidate list; F17-A's
  dependencies are all implemented, F16-C's remainder is blocked on a
  UI surface (now provided), F15-B is research-gated and F11-C is
  evidence-only — the menu unblocks the most downstream work.
- Starting commit: `812aea245b6f8c7cac702b27a5131af023e8be43` on
  `ralph/night`; tree was clean.

## What changed

- `crates/mm2_app/src/menu.rs` (new) — the whole menu slice, ~1000
  lines plus docs:
  - `MenuShell` — the model resource: a `Screen` stack (Root /
    CruiseCity / EventCity / EventTable / EventList / Garage /
    Paints / Profiles / ConfirmDelete), focus index, a `Vec<MenuRow>`
    rebuilt on `dirty`, status line, launch selections
    (vehicle/paint/difficulty), and `active`.
  - `MenuData` — shared state: optional `ProfileStore` + bound
    profile, lazily-scanned cities/`VehicleCatalog`/`GarageTable`/
    `EventCatalog`/`AvailabilityTable` caches (scanned once; the VFS
    is static per run), mod flag. Scans never fail silently — errors
    and empty results become disabled rows / status text.
  - `MenuCommand` / `Action` / `MenuEffect` — pure-ish decision
    surface: `apply(command, data) -> Vec<MenuEffect>`; `menu_input`
    executes effects (bind/unbind write or remove `ActiveProfile`,
    launch resolves `VehicleCatalog::load_by_id` then calls
    `Session::begin`, quit writes `AppExit`). A rejected config lands
    in `status` — no panic path.
  - `menu_watch` reopens the shell when the session reports `Menu`
    again; `menu_input` maps keyboard (arrows/WASD, Enter/Space,
    Esc/Backspace, X/Delete) and gamepad (dpad + left-stick edge nav,
    South/East/West); `menu_present` rebuilds a `bevy_ui` text tree
    (focused `›` marker, disabled rows dimmed with their reason).
  - Screens show real catalog data: Cruise picks from `city/*.psdl`
    stems (a catalogued city without its psdl says so); Events walks
    city → table → rows of real `EventRef`s where incomplete events
    name their missing files, CHK-3/CC gates name the unbeaten
    prerequisites, and Crash Course / empty tables are disabled with
    reasons; Garage lists `listed` roster entries only and refuses
    locked/incomplete cars with the reason; Paints shows authored
    `Colors` names with gated indices disabled; Profiles lists the
    store, binds on activate, creates a driver profile on demand, and
    `X`/`Delete` opens `ConfirmDelete` — activating the profile row
    itself never deletes.
  - Deleting the bound profile unbinds (`ActiveProfile` removed);
    DRV-7's last-profile refusal is preserved — the status is set
    *after* `pop()` so the reason survives the stack pop.
- `crates/mm2_game/src/progression.rs` —
  `AvailabilityTable::of_unbound` / `GarageTable::of_unbound`: the
  fresh-driver view for profile-less menus — still restricted,
  nothing beaten, no grants held (profile-less play cannot persist).
  Both share the bound-profile evaluators through a `beaten`/`holds`
  closure.
- `crates/mm2_app/src/main.rs` — `menu_mode` when no session-shaping
  or smoke/evidence flag is present (`--city`/`--event`/`--dev-world`/
  `--spawn`/`--cam`/`--vehicle-config`/`--banger-pool`/`--traction`/
  `--nav`/`--nav-route`/`--bot` all stay direct launches; smoke flags
  unchanged). Menu mode skips the boot `Session::begin`, inserts
  `MenuShell` + `MenuData` (seeded from the same `choose_launch`
  resolution a direct launch would use — CLI > remembered > default),
  and chains `menu_watch → menu_input → menu_present` into `Update`.
- `crates/mm2_app/src/session.rs` — `drive_session` takes an
  `Option<Res<MenuShell>>`: at `Menu`, quit intent only writes
  `AppExit` when no menu owns the process; `Unloading → Menu` still
  clears every session-scoped resource, so a menu-launched session
  quits back to the menu and a direct launch still exits. A targeted
  `#[allow(clippy::too_many_arguments)]` carries a comment (the menu
  parameter pushed the system over the lint; bundling its unrelated
  borrows would not improve it). `mm2_app::lib` exposes `menu`;
  `profile::ActiveProfile` derives `Debug` for the test assertions.

## Tests

- `cargo test -p mm2_app --test menu` — PASS (6):
  `the_app_boots_into_the_menu` (parked at `Menu`, rows drawn,
  real roster/city names present);
  `an_empty_install_reports_instead_of_faking` (every picker reports
  its reason; Enter never produces a `Loading`);
  `event_rows_carry_real_availability` (incomplete row names the
  missing file, gated row names the prerequisite, open row launches
  the real `EventRef`);
  `garage_picks_carry_through_launch` (gated paint refused; open
  paint lands on `SelectedCar`);
  `cruise_launches_then_quit_returns_to_the_menu` (menu hides →
  Playing → Esc → menu reopens → relaunch; single menu root and a
  single player entity throughout, zero `MenuUi` entities in-game);
  `profiles_bind_create_and_delete` (bind on activate, create,
  `X` opens the confirmation screen, delete removes the file, bound
  delete unbinds, last profile refuses with a visible reason).
- `cargo fmt --all -- --check` — PASS.
- `cargo clippy --workspace --all-targets --all-features --
  -D warnings` — PASS.
- `cargo test --workspace` — PASS, all suites, 0 failures.

## Still open

- F17-A remainder: Quick Race (`last_event` launch), per-event
  weather/time/density controls (need F18's session-legal writers;
  RACE-3 `customizable` is already surfaced), mouse navigation, text
  entry for profile names (new profiles get `Driver N` names), and
  the original-menu audit against F17-AC05's capability denominator.
- No GPU/manual playtest of the menu this slice — verification is the
  headless integration suite; the `bevy_ui` tree structure is asserted
  (row text, focus marker, entity counts) but no screenshot evidence
  exists yet.
- F16-C remainder: AC01's process-level leg (two real interactive
  launches completing an event) still needs a playable session;
  `--bot` finishes are deliberately ineligible.
