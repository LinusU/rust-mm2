# Last iteration — F30-A.2: perf report settings follow the app's own constants (new-run iteration 38 of this runner)

Selection: previous review passed, no blockers. Its verification gaps named one concrete
defect I can fix locally: `settings.window` / `settings.fixed_hz` were hard-coded strings in
`main.rs` mirroring literals used for the window and `Time::<Fixed>`, so they would go stale
silently. The remaining gaps (release baseline, extra columns, windowed proof, Windows/Linux
CPU probes) need hardware/time not available in one small slice and stay open.

Change: `WINDOW_SIZE` and `FIXED_HZ` constants in `main.rs` feed the `WindowPlugin`, the
`Time::<Fixed>` resource and the report's settings rows (`window_label`). 1 unit test
(`report_label_tests`) pins the row format.

Gates (foreground, exit statuses checked): fmt --check pass; clippy --locked -D warnings
exit 0; test --locked --workspace exit 0 (2424 passed, 0 failed). No test processes left running.

Not verified / open: the test pins the formatting, not that the window really opens at that
size (needs a window); menu-mode report does not name the later-launched city/event; release
baseline, memory/draw-call/voice/bandwidth columns, AC02 soak. AC01 still advanced, not
complete. Status: implemented candidate, not independently checked.

---

# Last iteration — F30-A.1: the benchmark report (new-run iteration 37 of this runner)

Selection: previous review passed, no blockers, no failing gate. The F30-B write-guard
items left are packaging/real-OS-layout (not provable here); F30-AC01 was entirely open —
`--perf-log` had percentiles but nothing that names the hardware, build, content or
settings, so a number could not be reproduced or distrusted. Smallest ready slice.

Change: `perf::RunContext` (scene/settings rows + world/install/mods from `main.rs`) and a
`<csv>.report.json` written beside the CSV on exit: engine commit, build profile
(debug assertions), OS/arch/CPU brand/logical cores, GPU adapter (read from Bevy's
`RenderAdapterInfo` once it exists), catalog + gameplay content fingerprints (computed
before frame 1), percentile/stage timings. Unobserved → `null`. `summary()` and the report
share one `Stats`. README "Frame-time profiling" documents it.

Tests: 5 unit in `perf::tests`. Windowed run on this machine (Apple M1, Metal, retail sf,
vpbug `--bot`, 400 frames, `--no-vsync`, dev profile) wrote a populated report
(gameplay fingerprint 5593 files / 28.8 MB; median 26.7 ms, p99 29.4 ms — a dev-profile
number, not a baseline).

Gates (foreground, exit statuses checked): fmt --check pass; clippy --locked -D warnings clean;
test --locked --workspace exit 0 (2423 passed, 0 failed). No test processes left running.

Not verified / open: no release-profile baseline recorded; no process-level test (needs a
window; hosted CI has none) — the windowed run is manual evidence; memory/draw-call/voice/
bandwidth columns; AC02 soak. AC01 advanced, not complete. Status: implemented candidate,
not independently checked.

---

# Last iteration — F30-B.4: the default profile root against the protected dirs (new-run iteration 36 of this runner)

Selection: previous review passed, no blockers. Its open items for F30-AC05 included
"default profile root vs. protected dirs"; that is the smallest ready one (`--mods` is a
read-only mount, so it is not a write and is not guarded).

Change: `profile::guarded_store_root` (+ private `guard_root`) checks the default OS
user-data root with `write_guard::check`. `main.rs` resolves `profile_root` once, after the
explicit-destination guards, and the three former `store_root` call sites (profile bind,
menu store, `settings.json`) use it. A refused default root logs the reason and means no
store; an explicit profile request then exits 2 through the existing path. Docs:
`architecture.md` "Where the app writes", PLAN F30-B.4.

Tests: 3 unit (`profile::guard_tests`), 2 process in `tests/write_guard.rs` (all OS
data-dir env vars pointed into the install → exit 2, nothing created; pointed outside →
profile binds). Red check: with the `check` call removed the install-refusal process test
fails; restored.

Gates (foreground, exit statuses checked): fmt --check exit 0; clippy --locked -D warnings clean; test --locked --workspace exit 0 (2418 passed, 0 failed). No test processes left running.

Not verified / open: no real macOS/Windows/Linux install layouts; no packaging step;
case-insensitive filesystems; dangling symlink components. AC05 advanced, not complete.
Status: implemented candidate, not independently checked.

---

# Last iteration — F30-B.3: the screenshot hotkey and `link/..` gaps of B.2 (new-run iteration 35 of this runner)

Selection: previous review passed with no blockers; its first two verification gaps
(interactive Cmd/Ctrl+P screenshot dir unguarded; symlink + `..` bypass) and a
contradictory PLAN line were the smallest ready F30-AC05 items.

Change: `write_guard::resolve` canonicalizes per component (so `link/..` climbs out of
the link's target); new `write_guard::first_allowed`; `main.rs` holds a `ScreenshotDir`
resource — `screenshots/` unless protected, then `<user-data>/rust-mm2/screenshots`, else
the hotkey refuses. PLAN F30-B row no longer says `--profile-dir` is unrefused;
`architecture.md` updated. Tests: 2 new unit tests (`dot_dot_after_a_symlink…`,
`the_first_unprotected_candidate_wins`).

Gates (foreground, exit statuses checked): fmt --check exit 0; clippy --locked -D warnings clean; test --locked --workspace exit 0 (2413 passed, 0 failed). Red check for the new symlink test against the old textual fold not re-run this iteration. No test processes left running.

Not verified / open: the `main.rs` hotkey wiring (needs a window; no process test);
default profile root and `--mods` unchecked; no real macOS/Windows/Linux install paths.
AC05 advanced, not complete. Status: implemented candidate, not independently checked.

---

# Last iteration — F30-B.2: app writes stay out of the install (new-run iteration 34 of this runner)

Selection: previous review passed, no blockers. It named F30-AC05 as explicitly
open (an explicit `--profile-dir` inside the install was not refused); smallest
ready item of F30.

Change: new `mm2_app::write_guard` (`protected_dirs`, `check`, `resolve`).
`main.rs` guards `--profile-dir`, `--perf-log`, `--screenshot` against the
`--mm2-path` install, the located app assets and the exe directory; a hit logs
the reason and exits 2 before anything is created. Paths are compared resolved
(symlinks followed, `..` folded, missing tail judged by deepest existing
ancestor). Tests: 10 unit + 3 process (`tests/write_guard.rs`); red with the
profile-dir check removed, restored. Docs: `architecture.md` ("Where the app
writes"), PLAN F30-B.2.

Gates (foreground, exit statuses checked): fmt exit 0; clippy --locked -D warnings exit 0; test --locked --workspace exit 0 (2411 passed, 0 failed). No test processes left running.

Not verified / open: default (OS user-data) profile root not re-checked against
the protected dirs; logs/caches the app does not write yet; no packaged bundle;
macOS/Windows/Linux real paths untested (Unix symlink tests only). AC05 is
therefore advanced, not complete. Status: implemented candidate, not
independently checked.

---

# Last iteration — F30-B.1: app assets independent of the working directory (new-run iteration 33 of this runner)

Selection: previous review passed, no blockers. F26-B has been served by a run of
test-only slices; F30 (AC04: "packaged app locates its own synthetic assets
regardless of current working directory") was untouched and `main.rs` mounted
`PathBuf::from("assets")` — a real cwd dependency (running `target/debug/mm2`
from any other directory lost the dev ground texture silently).

Change: new `mm2_app::app_assets` (`locate`/`locate_for_process`) searches from the
executable (`<exe>/assets`, `../Resources/assets`, `../share/rust-mm2/assets`, up
to four ancestors for a cargo tree) and only then the cwd; a directory counts
only if it holds `texture/dev_road.png`; no compile-time paths. `main.rs` logs
`mounted app assets` or warns when none is found. Tests: 10 unit + 2 process-level
(`tests/app_assets.rs`, spawn `mm2 --headless --dev-world` from a foreign cwd; a
cwd with a decoy `assets/` does not shadow). Red with the old cwd-only lookup (both
process tests fail), restored. Docs: `architecture.md` tier table, PLAN F30-B.

Gates (foreground, exit statuses checked): `cargo fmt --all -- --check` exit 0; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` exit 0; `cargo test --locked --workspace` exit 0 (2398 passed, 0 failed). No test processes left running.

Not verified / open: no packaging step exists, so the `.app`/prefix layouts are
unit-proven only; non-ASCII install path untested; AC05 (an explicit
`--profile-dir` inside the install is not refused) not done. Status: implemented
candidate, not independently checked.

---

# Last iteration — F26-B.6: a client that cannot run the changed ad (new-run iteration 32 of this runner)

Selection: previous review passed, no blockers. Its verification gap "a client
that cannot run the new advertisement was not exercised" is the smallest ready
item under F26-AC05 (windowed host menu and retail-city legs are broader).
`drive_lobby` already gated every `Message::Session` through `gate`/`refuse`,
but only the *first* ad had a test.

Change: test-only. `net_app::a_client_that_cannot_run_the_changed_session_leaves_cleanly`
— bridge accepts a dev cruise ad, the raw host re-advertises a city that does
not resolve on the client's mount; the client gets a "cannot run here" notice,
leaves with `LeaveCause::Quit`, exits `AppExit::Error(1)`, and `lobby.advertised`
stays the accepted ad. Verified red by making the `Session` arm accept on gate
failure (app never exits), then restored. No product defect found.

Gates (foreground, exit statuses checked): `cargo fmt --all -- --check` exit 0; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` exit 0; `cargo test --locked --workspace` exit 0 (2386 passed, 0 failed). No test processes left running. Not verified / open: two-process leg of the
refusal, windowed host menu, retail-city change, LAN/Internet. Loopback,
in-process, dev world. Status: implemented candidate, not independently checked.

---

# Last iteration — F26-B.5: operator surface for the changed rematch (new-run iteration 31 of this runner)

Selection: previous review passed with no blockers; its first verification gap
was that nothing a user can touch builds `HostCommand::Session`, so F26-AC05's
changed rematch was code-only. Took that gap (breakdown two-process leg is still
too broad, see PLAN).

Change: stdin `session key=value …` on the in-app host (`city`, `event`, `cnr`
+`gold`/`limit`, `difficulty`, `weather`, `tod`, `seed`; the `--host` flags'
parsers). It parses to `SessionChange` → `HostCommand::Change`, a *patch*
applied by `drive_host` to the link's current config (fresh seed unless
`seed=`), then through the existing `set_session` gates, so a mid-round or
unrunnable edit is refused and the old ad stays. Malformed words never queue
(stderr). Event rows are read against the resulting city; moving an event
session to another city without a new `event=` is refused. With host event
logging the verdict prints `event=session_changed` / `event=session_refused`.
`net::fresh_seed()` now backs both `--host`'s default seed and an unnamed edit
seed. Tests: 4 `SessionChange` units; `net_app::a_session_edit_from_the_operator_patches_the_advertised_session`
(chained edits reach the peer, bad row refused); `net_drive::a_session_edit_typed_on_the_host_reaches_the_next_round`
(two processes: mid-round refusal, `cancel`, `session weather=3`, same client
plays gen 2 with `traction=0.8`; verified red with `weather=0`, 3/3 green).
Docs: `research/net.md`, DSN-79, PLAN.

Gates (foreground, exit statuses checked): `cargo fmt --all -- --check` exit 0; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` exit 0 (after one `clone_on_copy` fix); `cargo test --locked --workspace` exit 0 (2385 passed, 0 failed). No test processes left running.

Not verified / open: no windowed host menu picks a mode; retail-city/two-process
city change; a client that cannot run the new ad; ad summary does not show
conditions (proved via client traction); LAN/Internet. Loopback, one machine,
dev world. Status: implemented candidate, not independently checked.

---

# Last iteration — F26-B.4: rematch with a changed session (new-run iteration 30 of this runner)

Selection: previous review passed with no blockers. The breakdown two-process
leg is still too broad (needs a retail networked race plus a wrecking knob).
Took F26-AC05's other named open item, "rematch with a changed city/mode".
Reading the code showed it was not just untested: `HostLink` fixed the config
it was opened with, so an in-app host could only ever re-run the same session.

Change: `HostCommand::Session(Box<SessionConfig>)` (HostCommand loses `Copy`/`Eq`)
→ `HostLink::set_session(vfs, config, running)`, drained by `drive_host` (now also
takes `Res<Mm2Vfs>`; both real schedules and the test app already insert it).
Gates: refused while `lobby.generation` is live (a peer that cannot run a changed
ad leaves the lobby — must not happen mid-race), then `validate` + `check_session`
(the `--host` flag-time gates); a refusal is a lobby notice and the old ad stays.
Host pick and `mods_active` stay the link's (Start announces the opened pick);
seed is the caller's; `late_join` follows the new mode. Test
`net_app::a_rematch_can_change_the_session_without_dropping_the_peer` (fixture
city, in-process loopback): cruise → Checkpoint event/Professional/new
conditions+seed; mid-round change refused; unresolvable city refused; peer gets
the new ad and a `Start` carrying it at generation 2; host seat begins it; a
newcomer is then refused `SessionStarted`. Verified red with the late-join update
removed. Docs: `research/net.md`, DSN-79, PLAN.

Gates (foreground, exit statuses checked): `cargo fmt --all -- --check` exit 0;
`cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`
exit 0; `cargo test --locked --workspace` exit 0 (2379 passed, 0 failed). No test
processes left running.

Not verified / open: no operator surface sends the command (stdin words and
lobby keys cannot build a config; no windowed host menu picks a mode), retail
city change, two-process leg, client that cannot run the new ad (leaves via
`refuse`), results→lobby windowed UI, spectator/race late-join, breakdown
process leg, LAN/Internet. Status: implemented candidate, not independently
checked.

---

# Last iteration — F26-B.3: late join after a leave (new-run iteration 29 of this runner)

Selection: previous review passed, no blockers. Report 6 follow-up 1's
two-process leg (breakdown for a remote human's wreck) needs a networked
Blitz/Checkpoint race on a retail install plus an evidence knob that wrecks
a car headlessly — too broad for one iteration and unprovable on the
synthetic dev world, so left open (recorded in PLAN). Took the next named
F26-B open item instead: reconnect. Without player identity on the wire a
"reconnect" is a new connection, so the leg pins what must hold: the
leaver's seat is gone, the newcomer is handed the live generation under a
fresh wire id and sees only the host.

Change (test + docs only): `net_drive::a_seat_freed_by_a_leaver_is_not_resurrected_for_the_next_joiner`.
Host starts gen 1 alone, `first` joins/drives/quits (`event=left cause=quit`),
`second` joins: host logs a second `remote participant spawned` with a
different wire id; second's record is `mp=gen1 phase=playing`, inputs sent,
snaps applied, `rem1` exactly. The first run failed on `remote_spin>0`
(the host's own car has driven to the dev-world end by then and sits still),
so this leg asserts the other `assert_client_drove` fields inline and says
why; the spinning copy is covered by the first joiner. Waits are output-driven
(`until`/`until_within`), no sleeps or fixed frame counts. Docs: `research/net.md`,
DSN-79, PLAN.

Gates (foreground, exit statuses checked): `cargo fmt --all -- --check` exit 0;
`cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`
exit 0; `cargo test --locked --workspace` exit 0 (2378 passed, 0 failed). No test
processes left running.

Not verified / open: spectator/race participation for late joiners, race ending
during a late join, same-identity reconnect (designed non-feature), breakdown
process leg, LAN/Internet. Loopback, one machine, synthetic dev world. Status:
implemented candidate, not independently checked.

---

# Last iteration — F26-B.2: late-join process leg (new-run iteration 28 of this runner)

Selection: previous review passed with no blockers (its gaps are
loopback/synthetic scope, already disclosed). Operator report 6's
follow-ups 1 (in-process legs landed), 2 (reconciled, DSN-11) are done;
the remaining open F26 item the prior handoff named was late-join policy.
F26 req 4 / AC02 had only in-process legs, so I wrote the separate-process
leg first and let it find what was broken. Nothing was: it passed first
time and 3/3 on repeat.

Change (test + docs only): `net_drive::a_client_that_joins_a_running_session_is_handed_the_live_one`
— in-app host `start`s generation 1 with an empty roster, then a
`--join --ready` process connects; it is handed the running `Start`,
loads generation 1, spawns the host seat as a remote copy, streams inputs
and applies snapshots (`mp=gen1 phase=playing`, via `assert_client_drove`);
the host spawns the late seat and applies its inputs; clean leave/quit.
Waits spin on process output (`until*`), no fixed frame counts or sleeps.
Docs: `research/net.md` (late join, process level), DSN-79 note, PLAN.

Gates (foreground, exit statuses checked): `cargo fmt --all -- --check` exit 0;
`cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`
exit 0; `cargo test --locked --workspace` exit 0 (2377 passed, 0 failed). No test
processes left running.

Not verified / open: late-joiner participation/spectator policy in a race
(races refuse late joiners by MP-5), reconnect with the same identity,
race ending during a late join, late join of a session with broken props
at process level (retail install only), LAN/Internet. Loopback, one
machine, synthetic dev world. Status: implemented candidate, not
independently checked.

---

# Last iteration — F26-B.1: lobby rematch (new-run iteration 27 of this runner)

Selection: previous review passed with no blockers (its gaps are
no-audio-device / unrendered rows, environment limits). Operator report 6
says keep working on networking, and recent iterations had drifted to
F20/F21/F23. F26 req 6 / AC05 (lobby → session → result → rematch → lobby)
had no process-level evidence at all and `grep rematch` found nothing in
code, so I wrote the two-round leg first and let it find what was broken.

Found and fixed (both shipped by the leg, neither visible to the
in-process tests):
1. `--ready` readied once at join. The host's `Cancel` clears every ready
   flag, so an unattended client could never pass round 2's start gate.
   `LobbyLink::keep_ready` (set from `--ready` in `main.rs`) and `mm2-join`
   now re-send `ready` on each `Cancel`; the host resets before it sends
   `Cancel`, so the answer lands after the reset.
2. A stale `SessionControl::quit`. Teardown leaves `quit` queued past
   `Unloading → Menu` (the Menu arm consumes it next frame by design); a
   `Start` in that window begins the next session in the same frame, and the
   old flag quit the new one as it went live; the host's Menu auto-cancel
   then ended round 2 for everybody (`event=cancelled generation=2`
   straight after `started generation=2`). New `net::begin_wired` owns the
   four wire-driven begins (client direct/parked, host direct/parked) and
   clears quit/restart/pause on success only. Diagnosed by logging phase and
   intents per frame in `drive_host`; that debug code is removed.

Tests (+5): `net::tests` `begin_wired` x2; `net_app`
`a_keep_ready_client_readies_again_for_the_next_round` and its negative;
`net_drive::two_mm2_processes_play_a_rematch_without_reconnecting`
(host + `--ready` client, round 1 gated on the host spawning the seat,
`cancel`, unprompted re-ready, `start` → gen 2, client cap record reads
`mp=gen2 phase=playing`, clean quit). Confirmed red with the quit-clear
commented out (fails 3/3), green with it (3 full network-suite runs, 102/102).
A first draft of the process leg failed under the parallel workspace run
(client frame budget 3000 too tight on a loaded machine); the budget is
4500 and the record wait is `until_within(120 s)`.

Gates (foreground, exit statuses checked): `cargo fmt --all -- --check` exit 0;
`cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`
exit 0; `cargo test --locked --workspace` exit 0 (2376 passed, 0 failed). No test
processes left running.

Not verified / open: rematch with a *changed* city/mode (the host advertises
one session per lobby today), windowed results→lobby UI, late join/reconnect
policy, LAN/Internet. Loopback, one machine, synthetic dev world only.
Status: implemented candidate, not independently checked.

---

# Last iteration — F23-B.3: audio volume levels (new-run iteration 26 of this runner)

Selection: previous review passed with no blockers; its gaps (no real
device, unrendered rows) are environment limits, not defects. Operator
report 6 points at networking, but F23's req 2 audio half had no
implementation at all (AC05 "audio and graphics controls have actual
observable effects"), `bevy_audio` is wired, and a volume option is a
small self-contained slice. Networking items need multi-iteration
two-process work and were left for a dedicated iteration.

Finding that shaped the design: bevy's `GlobalVolume` only scales a sink
at creation, and this tree's mixers rewrite `set_volume` every frame, so a
global master would silently do nothing for loops. Gain is therefore applied
in two places.

Change: `settings::AudioLevels` (master/effects/commentary/city, `u8`
percent so `GraphicsSettings` stays `Eq`; `gain(bus)` = master × bus) is a
field of `GraphicsSettings`, so persistence, the live resource, the menu's
`set_settings` and the pause page's `adopt` all carry it unchanged. `audio.rs`:
`VoiceKind::bus`, `voice_gain`, `level_new_voices` (PostUpdate, before
`TransformSystems::Propagate` — bevy's own queued-audio systems run after it,
so a sink is never created at the authored volume; registered in `main.rs`),
and `push_mix` takes the bus gain (`engine_drive`, `ambient_engine_drive`,
weather beds, `surface_voices`); `object_sound` multiplies its per-frame
falloff. The `mix` stored on voice components stays the authored value.
Menu Options screen and pause graphics page gained four rows (pause
`CONTROLS_ROW` is now derived, 7). Ledger DSN-78, `docs/research/menu.md`.

Tests (+13, 2371 total): settings unit (defaults, gain maths, step/wrap,
round-trip + pre-audio file, >100 clamp and malformed reset, row text);
`tests/audio.rs` (default = authored 0.9, master×bus scales once and does not
compound, zeroed bus silences only its voices, bystander `PlaybackSettings`
untouched, no settings = authored, bus mapping); `tests/menu.rs`
(volume rows step/wrap/persist/reset; existing option tests re-indexed);
`tests/session.rs` (pause volume rows; existing pause tests re-indexed).

Gates (foreground, exit statuses checked): `cargo fmt --all -- --check` pass;
`cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`
exit 0; `cargo test --locked --workspace` exit 0 (2371 passed, 0 failed).

Not verified / open: no output device — the sink-side multiply in the mixers
(`push_mix`, object-sound `set_volume`) is glue that needs a live sink and is
only covered by the pure gain tests and the spawn-time pass; audibility is
unheard. An in-flight one-shot keeps its spawn-time level. Rows not rendered.
Still open for F23: audio on/off toggles, music bus (no music player),
device/stereo/quality/balance, window/resolution/scaling and display recovery
(AC04), accessibility options, pad shift buttons, auto-reverse row, pad
rebinding, mouse driving. Status: implemented candidate, not independently
checked.

---

# Last iteration — F23-A.2: selectable transmission policy (new-run iteration 25 of this runner)

Selection: previous review passed with no blockers (its gaps: fmt/clippy not
visible in the log it read, live pad log path not run — neither is a defect to
repair). Took the F23 req-1 item still absent: "selectable transmission
policy" (CTL-1 documents `T` auto/manual and `A`/`Z` shifts). The sim already
had an explicit `VehicleInput::forced_gear` command, so this is a device-side
slice only; physics untouched.

Change: `ControlSettings.transmission` (`TransmissionPolicy` Automatic default /
Manual), persisted as `transmission` in `controls.json` (unknown value →
automatic + issue line). `DriveAction` gained `ShiftUp`/`ShiftDown` (default
G/B — the documented A/Z collide with steer-left and the nav-arrow key; they
rebind like the driving keys and appear on both Controls pages). New
`mm2_app::manual_gear::ManualGear` holds the chosen gear per car: seeded from
the car's own gear (switching never lurches), one key *press* = one gear,
clamped to the config's gear count, dropped on every non-driving frame and
re-seeded for a new car. `vehicle_input` pins `forced_gear` from it. A
Transmission tuning row (menu Controls screen + pause controls page) flips the
policy; Reset restores automatic. Policy is inert when
`authority_role()` is not authority: `forced_gear` never rides the wire, so a
pinned predicted copy would diverge from the host's automatic car (recorded in
DSN-77, original-rules.md). Devices record row "manual transmission" is now
SyntheticOnly (doc + table kept in step).

Tests (+12, 2358 total): `manual_gear` unit (seed/step/clamp/cancel/reseed/
shorter gearbox); controls unit (policy persists, bad value repaired, old file
loads with shipped shifts, clash with new default resets as a set, shift keys
are edges, row toggle + reset); `tests/input.rs` (manual pins/steps/gates
through the production `vehicle_input`; inert on a `Remote` session); menu
test for the row + shift-key rebind. Existing menu/session tests re-indexed for
the extra rows; two tests that used KeyB as a free key now use KeyJ.

Gates (foreground, exit statuses checked): `cargo fmt --all -- --check` pass;
`cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`
pass; `cargo test --locked --workspace` pass (2358 passed, 0 failed).

Not verified / open: no real keyboard/pad session; the manual box's feel on a
real car was not driven (the sim's forced-gear path has its own
`mm2_vehicle` test, `drive.rs::forced_gear_pins_the_gearbox`); original `T`/`A`/`Z`
behaviour and rev-limit/auto-downshift rules unverified; no pad shift buttons;
a hand-edited `controls.json` that already used G or B for another action
resets its key set (reported). Still open for F23: auto-reverse row, pad
rebinding, mouse driving, non-driving keys, audio/accessibility options, text
entry vs live field, AC04 display recovery, AC05 audio effect, unrendered
pause Controls page. Status: implemented candidate, not independently checked.

---

# Last iteration — F23-C.3: per-capability input-device record (new-run iteration 24 of this runner)

Selection: previous review passed with no blockers. Its open F23 list is
mostly large (pad rebinding, mouse driving, audio/accessibility options)
or needs hardware/rendering this run lacks. F23-AC06 ("device/platform
verification recorded per capability, untested wheel/feedback support
explicitly labeled") had no artifact at all, and is cheap to satisfy
honestly, so I took it. Pause-page rendering, pad rebinding, transmission
policy and audio options untouched.

Change: new `mm2_app::devices` — `Capability` (11: keyboard driving, key
rebinding, gamepad driving/menus/hot-plug, focus-loss release, pad
rebinding, mouse driving, manual transmission, steering wheel, force
feedback), `Status` (`NotImplemented` / `SyntheticOnly` — no hardware
level exists because none was recorded) and `RECORDS` with a note per
row. `log_input_capabilities` (Startup) writes the record to the log;
`log_pad_connections` (Update) names each pad the OS reports with its USB
ids and, on removal, the remembered name (`describe_event`). Both are
registered in `main.rs`. `docs/research/input-devices.md` carries the same
table; a unit test fails if a record line is missing from it. Wheel and
force feedback are `NotImplemented` (no wheel mapping, no rumble/FFB
requests anywhere in the tree — grep for `Rumble` is empty).

Tests (+5, 2346 total): records cover every capability once in order;
wheel/FFB never claimed; doc table lists every record line;
`connected_line` ids; a disconnect is named after its connect and the name
is forgotten afterwards.

Gates (foreground): `cargo fmt --all -- --check` pass; `cargo clippy
--locked --workspace --all-targets --all-features -- -D warnings` clean;
`cargo test --locked --workspace` exit 0 (2346 passed, 0 failed; was
2341). No processes left running.

Not verified / open: the log systems were not run against a real pad (only
`describe_event` is unit-tested; the system wiring is three lines); every
`SyntheticOnly` row still lacks a physical-device session. AC06 is advanced
(the record exists, wheel/FFB labeled), not closed: a hardware session
needs a pad/wheel this run cannot reach. Still open for F23: pad
rebinding, mouse driving, transmission policy, non-driving keys,
audio/accessibility options, "text entry never drives" with a live text
field, AC04 display recovery, AC05 audio effect, unrendered pause Controls
page. Status: implemented candidate, not independently checked.

---

# Last iteration — F23-B.2: pause-menu route to the Controls page (new-run iteration 23 of this runner)

Selection: previous review passed with no blockers; its open F23 list led
with the pause-menu route to Controls (the Controls screen was reachable
only from the main menu, so a player mid-race could not remap). Took that
one leg; audio/accessibility options, pad rebinding and AC04 untouched.

Change: the pause overlay's graphics page gained a "Driving controls" row
opening `PausePage::Controls` (`PauseMenu.options: bool` became
`page: PausePage`, plus `capture`): one row per key slot (primary and
"(alt)"), Enter listens for the next key (Esc / pad East / Start cancel,
nav keys bind instead of moving), X / Delete / pad West clears a slot,
Left/Right/Enter step the tuning rows, reset disables itself at the
defaults; Esc backs out one page at a time. Every change replaces the
live `ControlSettings` and saves `controls.json` through a new
`ControlsSave` resource (main.rs inserts it with the same evidence-run
rule as the menu). To avoid a second copy of the screen, the shared pieces
moved into `controls.rs`: `with_key` / `without_key` (validated rebind +
status line or refusal), `tuning_rows` / `adjusted` (`ControlItem`,
`ControlRow`); the main menu's Controls screen now uses them too. Because
the pause page edits the live resource, `menu_watch` re-syncs the menu's
copy on reopen (same as graphics) — otherwise the next menu edit would
have overwritten the pause edit with a stale map. `PauseGraphics`
SystemParam gained the controls resources; `LiveSettings` / `PauseView`
SystemParams keep `menu_watch` / `pause_present` under the argument lint.

Tests (+5, 2341 total): `session`: the pause page rebinds / refuses
reserved + conflicting keys while still listening / Esc cancels / nav keys
bind / clears a slot, never the last / steps tuning / resets / backs out a
page at a time, each change checked on the resource and re-loaded from the
file; the row is disabled without `ControlSettings` and a pending capture
dies with the pause (restart). `menu`: a pause rebind (pad East cancels a
listen first) survives into the main menu's Controls screen and a later
menu edit keeps it. `controls.rs`: +2 unit (`with_key`/`without_key`,
`tuning_rows`/`adjusted`). Existing pause-options tests updated for the
`page` field and the extra row (Back moved from 4th to 5th).
Mutation checks: dropping the `menu_watch` controls re-sync fails the menu
test; dropping the capture reset on leaving `Paused` fails the pending-
capture test.

Gates (foreground): `cargo fmt --all -- --check` pass; `cargo clippy
--locked --workspace --all-targets --all-features -- -D warnings` exit 0;
`cargo test --locked --workspace` exit 0 (2341 passed, 0 failed; was
2336). No processes left running.

Not verified / open: synthetic key/pad events only; the new page was not
rendered (16 rows at 22 px should fit 720 px but no capture was taken —
there is no `--pause` page flag); no real keyboard session. Still open for
F23: pad rebinding, mouse/wheel rows, auto-reverse/transmission policy,
non-driving keys, audio buses/accessibility options, "text entry never
drives" with a live text field, AC04 display recovery, AC05 audio effect,
AC06 device records. Status: implemented candidate, not independently
checked.

---

# Last iteration — F23-C.2: any connected pad answers (new-run iteration 22 of this runner)

Selection: previous review passed with no blockers. Of its open items the
pause-menu route to Controls would mean a second hand-rolled copy of the
Controls screen in `pause.rs`, so I took the smaller AC03 leg first
("gamepad focus remains usable after hot-plug", controller ownership).
Audit found every menu-like screen and driving used `pads.iter().next()`:
an idle first pad (spare controller, a wheel that registers as a gamepad)
silently shadowed the pad the player held, and the stick edge latch kept a
stale value across an unplug.

Change: `input::pad_nav(pads, &mut latch) -> PadNav` merges the D-pad /
South / East / West / Start edges over every connected pad and takes the
strongest left-stick deflection for the edge-triggered stick nav; with no
pad the latch resets to neutral. `menu_input`, `pause_input` and
`results_input` now use it (their three copies of the pad block are gone).
`ControlSettings::drive_input` takes the pads and the first pad actually in
use (stick past deadzone, trigger past deadzone, or South) owns the frame —
never a mix of two pads' axes. `control_just_pressed` uses `any` pad.
Behaviour with one pad is unchanged.

Tests (+6): `menu`: any pad navigates the main menu through hot plug (idle
+ held pad, stick edges, unplug the idle one, unplug all → keys, plug a
late pad, South/East on another pad); a stick held through an unplug does
not poison the latch; the pause menu answers the second pad (Down, Start
resumes). `results`: rows answer a second pad. `device_transitions`: an
idle first pad does not shadow the pad in use / no axis mixing; W and Left
held while `Paused` or at `Results` read neutral and resume cleanly.
Mutation check: reverting to first-pad-only (`take(1)` in `pad_nav`, break
after the first pad in `drive_input`) fails 4 of the 5 pad-ownership tests
(the latch test guards the reset, not the ownership).

Gates (foreground): `cargo fmt --all -- --check` pass; `cargo clippy
--locked --workspace --all-targets --all-features -- -D warnings` clean;
`cargo test --locked --workspace` exit 0 (2336 passed, 0 failed; was 2330).
No processes left running.

Not verified / open: synthetic `Gamepad`/`RawGamepadEvent` state only — no
physical pad or real hot-plug; with two pads deflected at once the first in
entity order owns driving (a choice, not an original rule). AC03's "text
entry never drives" has no live text field in-session (only the profile
name screen, already isolated by `menu_input`), pause-menu route to
Controls, pad rebinding, mouse/wheel rows, audio/accessibility options and
AC04 display recovery remain open. Status: implemented candidate, not
independently checked.

---

# Last iteration — F23-B.1: Controls screen (new-run iteration 21 of this runner)

Selection: previous review passed with no blockers. F23-B (the rebinding
UI) was the next queued F23 slice and the only place AC01's "remapped
binding" could be reached by a player — `ControlSettings` existed with no
screen. Slice: key rebinding + stick tuning UI only; audio, accessibility
and menu-text isolation (AC03) untouched.

Change: `Screen::Controls`, opened from a new "Driving controls" row on the
Options screen (`--menu-screen controls` for captures). Each action's
primary key is the row, its alternate the side entry; Enter listens
(`MenuShell::capture`), the next key goes through `ControlSettings::rebind`
— conflict (names the owner), reserved and unbindable keys are refused and
the screen keeps listening; Esc/Back/pad East cancel; hover and clicks are
inert while listening and nav keys bind instead of moving focus (`menu_input`
sends `MenuCommand::Capture`). X/Delete clears a slot (last key refused).
Stick deadzone, trigger deadzone, sensitivity and inversion cycle in place;
reset row disabled at the shipped map. Every change saves `controls.json`
(save failure keeps the change and says so) and emits `MenuEffect::Controls`
to replace the live resource `vehicle_input` reads. `controls.rs` gained
`cycled_*`/`slot_label` helpers.

Tests: `tests/menu.rs` +6 (rows; capture rebinds + resource + file + reset;
alternate via side entry; refusals keep listening/Esc cancels/nothing saved;
clear + last-key; tuning persists/Back drops capture), `controls.rs` +1
(step wrapping, off-grid values, ranges); the Options row-list test updated
for the new row. Rendered: `--menu --menu-screen controls --frames 20
--screenshot` → `status=pass bytes=2760756` on Metal/Apple M1, PNG inspected
(local only).

Gates (foreground): `cargo fmt --all -- --check` pass; `cargo clippy --locked
--workspace --all-targets --all-features -- -D warnings` exit 0; `cargo test
--locked --workspace` exit 0 (2330 passed, 0 failed; was 2323). No processes
left running.

Not verified / open: no real keyboard session driving the capture (synthetic
`ButtonInput` presses; the persisted-remap-drives-the-car path is the
earlier `tests/input.rs` test); the in-session pause Options row is still
disabled so controls are reachable only from the main menu; no pad
rebinding, mouse/wheel rows, audio/accessibility options, AC03 menu/text
isolation after hot-plug, AC04 display recovery. Status: implemented
candidate, not independently checked.

---

# Last iteration — F23-C.1: focus-loss and pad-disconnect evidence (new-run iteration 20 of this runner)

Selection: previous review passed with no blockers and flagged F23-AC02 as
unverified. F23-B (rebinding UI/options) is a larger slice; AC02's logic
(focus gate in `vehicle_input`, bevy's `release_all` on `KeyboardFocusLost`,
`Gamepad` removal on disconnect) already exists but had no test, so I took
the small test-only slice.

Change: `crates/mm2_app/tests/device_transitions.rs` (registered in
`tests/app.rs`), four tests driving bevy's real `InputPlugin` with raw
events through the production `input::vehicle_input`: focus loss + refocus
with held keys; unfocused window + held pad (answers again on refocus, the
device is physically still held); pad disconnect with stick/trigger held
(neutral, keyboard still drives, reconnect starts neutral); first of two
pads unplugged → the second drives. No production code changed.

Mutation check: forcing `focused = true` in `vehicle_input` fails the pad
focus test. The key focus test still passes under that mutation because
bevy itself releases the keys (the test documents the engine behaviour, it
does not prove ours). Disconnect behaviour is the engine's; not mutable here.

Gates (foreground): `cargo fmt --all -- --check` pass; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` exit 0; `cargo test --locked --workspace` exit 0 (2323 passed, 0 failed; was 2319). No processes left running.

Not verified / open: synthetic events only — no real window focus change,
physical pad or hot-plug; F23-AC03 (menu/text input isolation, pad focus
after hot-plug in menus) untouched; F23-B UI, audio/accessibility options,
wheel/FFB audit remain. Status: implemented candidate, not independently
checked.

---

# Last iteration — F23-A.1: rebindable driving controls (new-run iteration 19 of this runner)

Selection: previous review passed with no blockers. The F21-B remainder
(instruction flow, pathset restore, UNK-35 evaluators) is research-blocked
with no new evidence, so I took the first untouched ready feature: F23-A had
no rebinding or controls persistence (only graphics settings existed).
Slice: the persistent input schema + normalization, no UI (F23-B).

Change: new `mm2_app::controls` (DSN-76). `ControlSettings` holds two keys
per driving action, pad deadzones, steering gain, stick inversion;
`drive_input(keys, pad)` is the single keys+pad → `VehicleInput` mapping and
`input::vehicle_input` now calls it (absent resource = shipped defaults, for
harness apps). `rebind`/`unbind` refuse unbindable, reserved (in-session
function keys), conflicting (names the owner) and last-key cases; `load`
repairs invalid pieces individually, rejects a doubly-claimed key as a whole
set, and never fails. `controls.json` sits beside `settings.json`
(evidence runs skip it, like graphics); `settings::write_json_atomically`
is now shared by both files.

Tests: 11 unit (`controls.rs`), `tests/input.rs` +3 (persisted remap →
fresh app → drives, old key dead; remapped key obeys Free/pause gates;
deadzone/gain/inversion). The existing pad/key tests pass unchanged.

Gates (foreground): `cargo fmt --all -- --check` pass; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` exit 0 (one targeted `too_many_arguments` allow on the Bevy system `vehicle_input`); `cargo test --locked --workspace` exit 0 (2319 passed, 0 failed; was 2305). No processes left running.

Not verified / open: no rebinding screen; default-map-equals-before is
asserted only by the existing tests; no windowed run, no real pad hot-plug;
mouse/wheel controllers, auto-reverse and transmission policy rows,
audio/accessibility options remain. Status: implemented candidate, not
independently checked.

---

# Last iteration — F21-B.10: required lesson vehicle (new-run iteration 18 of this runner)

Selection: previous review passed with no blockers. Of the F21-B
remainder (instruction flow, pathset/vehicle restore, UNK-35 evaluators)
only the vehicle leg has documented evidence (CC-4, help text) and fits a
small slice; evaluators and instruction content stay research-blocked.
Nothing made the Crash Course use its school's car — a lesson drove
whatever the menu had selected.

Change: `mm2_content::required_vehicle` (sf→`vpbullet` Ford Mustang
Fastback, london→`vpcab` London Cab, others none; ids confirmed through
the retail `--list-cars`); `menu.rs::launch_vehicle` substitutes it for an
unpassed lesson row (passed lesson = replay in the pending car, CC-4's
second sentence; car absent from the install = keep the pick + warn);
the shell's pending selection is untouched; `profile::note_session_start`
takes `remember_vehicle` (false for a lesson session) so the forced car
never becomes the remembered vehicle. Ledger CC-4 and PLAN updated.

Tests: `tests/menu.rs` +2, `mm2_content/tests/crashcourse.rs` +1. Mutation
checks: disabling the substitution fails both menu tests; passing
`remember_vehicle=true` fails the second.

Gates (foreground): `cargo fmt --all -- --check` pass; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` clean; `cargo test --locked --workspace` exit 0 (2305 passed, 0 failed; was 2302). No processes left running.

Not verified / open: no retail menu launch or windowed lesson; per-lesson
vs per-school car unknown; CLI `--event crash:N` keeps `--car`;
instruction flow, pathset restore, UNK-35 evaluators. Status: implemented
candidate, not independently checked.

---

# Last iteration — F21-B.9 follow-up: end-to-end pass-credit test (new-run iteration 17 of this runner)

Selection: previous review passed with no blockers; its first verification
gap was that no test ran the real `drive_lesson`/`advance_race` path
through to the profile credit (the progression tests hand-fed
`LessonDriver::observe` and injected the ledger entry). Closed that with a
test; no production code changed.

Change: `crates/mm2_app/tests/lesson_drive.rs` — `lesson_app` now wraps
`lesson_app_with(setup, config)`; new test
`clearing_every_leg_credits_the_profile_once_through_the_production_systems`
runs a two-leg lesson on a bound Standard profile with
`record_session_results` scheduled after `drive_lesson`: the first leg's
ledger finish leaves the profile empty, the last clear records exactly
one beaten record on the lesson key (finishes 1, best ticks = the driver's
summed pass ticks) and grants the `crash` reward once. Mutation check:
disabling the lesson branch in `record_session_results` makes it fail.

Gates (foreground): `cargo fmt --all -- --check` pass; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` exit 0; `cargo test --locked --workspace` exit 0 (2302 passed, 0 failed; was 2301). No processes left running.

Not verified / open: unchanged — no retail/windowed lesson pass, no
lesson results-screen text, per-difficulty credit unverified,
instruction flow, pathset/vehicle restore, UNK-35 evaluators. Status:
test-only candidate, not independently checked.

---

# Last iteration — F21-B.9: lesson-pass credit (new-run iteration 16 of this runner)

Selection: previous review passed with no blockers. Its gap list named
lesson-pass credit as the next F21-B item (midterms/finals were
unreachable from the menu without it), so I took it.

Design: no new persistence. A lesson's pass is recorded on the lesson's
own `EventKey` through the existing `apply_result` (`Finished{sum of leg
ticks}`, place 1) — DSN-17 already reads CC-3/CC-4 from that record's
`beaten` flag and `apply_result` already grants indexed `crash,N`
rewards. `lesson_launch` now carries the city's real reward/availability
tables. Leg ledger entries still record nothing (F21-B.5). Credit is
gated like any result (standard profile, eligibility, no scripted
driver) and happens once per session generation, from
`LessonDriver::pass` only.

Change: `race.rs` (`LessonSetup` +rewards/availability), `progression.rs`
(`record_session_results` lesson branch, `note_outcome`), docs (DSN-75,
PLAN, lesson.rs module doc).

Tests: `tests/progression.rs` +2, `tests/menu.rs` +1 (synthetic).

Gates (foreground): `cargo fmt --all -- --check` pass; `cargo clippy
--locked --workspace --all-targets --all-features -- -D warnings` exit 0;
`cargo test --locked --workspace` exit 0 (2301 passed, 0 failed; was
2298). No processes left running.

Not verified / open: no retail or windowed run of a pass; no
results/pass/fail screen text for a lesson; whether the original credits
per difficulty rank is unverified; instruction flow, pathset/vehicle
restore, UNK-35 evaluators. Status: implemented candidate, not
independently checked.

---

# Last iteration — F21-B.8: Crash Course menu entry (new-run iteration 15 of this runner)

Selection: previous review passed (no blockers). The first remaining F21-B
item was the menu entry — the only way a player could reach a lesson was
`--event crash:N`. Lesson-pass credit is next but needs a design on how it
differs from an event record (F21-B.5 refuses them), so I took the smaller,
independent slice.

Change: `menu.rs` — Crash Course table row enabled by catalog rows (shows
`N lessons`), list rows named by the authored tag (`Lesson 1`, `Midterm 1`,
`Final`), Options entry closed ("crash course lessons run as authored"),
the three "not loadable yet (F21)" refusals removed (table row, Quick
Race, Records). Midterms/finals keep the authored CC-2 gate and therefore
stay locked until lesson-pass credit lands — stated in DSN-75 and PLAN.

Tests: `tests/menu.rs::crash_course_rows_launch_as_lessons` (synthetic);
the unresolvable-records test now expects the generic catalog reason for a
stale crash record.

Gates (foreground): `cargo fmt --all -- --check` pass; `cargo clippy
--locked --workspace --all-targets --all-features -- -D warnings` exit 0;
`cargo test --locked --workspace` exit 0 (2298 passed, 0 failed; was 2297).
No processes left running.

Not verified / open: no retail/windowed menu check, no lesson-pass credit
(midterm/final unreachable from the menu), instruction/pass/fail screens,
pathset/vehicle restore, UNK-35 evaluators. Status: implemented candidate,
not independently checked.

---

# Last iteration — F21-B.7: retail lesson launch validation (new-run iteration 47)

Selection: the previous review passed; its first verification gap was
that `--event crash:<row>` had never run on original data. Retail is
available locally (`MM2_RETAIL`), so I closed that gap instead of adding
more unverified code.

Change: no production code. Ran `mm2 --headless --frames 300 --event
crash:N [--pro]` for all 13 london + 13 sf rows × both difficulties on
retail (`fnv1a64:e91e6cd4b2ae30d9`): 52/52 loaded leg 0 and reached
`race=Running`, 0 errors (e.g. london crash:0 4 gates 24 s, crash:1 23
gates untimed, sf crash:5 23 gates 178 s). The london crash0 parked-car
`_crash0` pathset is already spawned by the generic path. Added the
repeatable opt-in test `tests/lesson_launch.rs::
every_retail_lesson_launches_at_both_difficulties` (denominator = catalog
CrashCourse rows; prints and returns when `MM2_RETAIL` is unset — the
external gate has no retail, so it is a no-op there).

Gates (iteration 47, foreground): `cargo fmt --all -- --check` pass; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` clean; `cargo test --locked --workspace` exit 0 (2297 passed, 0 failed; was 2296). No processes left running.

Not verified / open: launch only — no leg driven on retail, no windowed
capture, menu entry, instruction/pass/fail screens, lesson vehicle
restore, per-leg HUD rebinding, lesson-pass credit, UNK-35 evaluators.
Status: implemented candidate, not independently checked.

---

# Last iteration — F21-B.6: lesson session launch (new-run iteration 46)

Selection: the previous review passed; its gap list said the future
launcher must reinstall the driver for a restarted lesson. The launcher
is the next F21-B piece and the last thing between the driver and a
playable (gate-baseline) lesson.

Change: `race::lesson_launch` (crash row → leg 0 `EventSetup` + fresh
`LessonDriver`); `load_session_world` routes `SessionMode::Event` crash
rows through it and inserts the driver beside `RaceState`. Restart
reloads, so the driver is rebuilt on leg 0 (the review's gap). Race picks
are skipped for lessons. Unbuildable lesson fails the session. Reachable
only via `--event crash:<row>`; menu rows, `event_race_setup`, net and
`mm2-host` unchanged (still refuse/disabled). DSN-75 and PLAN updated.

Tests: `tests/lesson_launch.rs` +4 (via `tests/app.rs`).

Gates (iteration 46, foreground): `cargo fmt --all -- --check` pass; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` clean; `cargo test --locked --workspace` exit 0 (2296 passed, 0 failed; was 2292). No processes left running.

Not verified / open: menu entry, instruction/pass/fail screens, lesson
pathset overlay and vehicle restore, per-leg HUD/racestat/nav/countdown
rebinding, lesson-pass credit, UNK-35 evaluators. No original-data run;
tests synthetic. Status: implemented candidate, not independently
checked.

---

# Last iteration — F21-B.5: lesson isolation + driver teardown (new-run iteration 45)

Selection: the previous review passed with no blocking findings, but named
two must-fix items before any lesson can launch: per-leg `Finished`
results still reach `ResultLedger` → `record_session_results` as event
finishes, and nothing removed `LessonDriver` at teardown. Both are small
and independent of the (larger) launcher, so I took them.

Change: `record_session_results` takes `Option<Res<LessonDriver>>` and
blocks while one is present (no `EventRecord`, no authored reward;
`SessionReport.note` = "crash course lesson - leg results are not event
records"). `drive_session`'s teardown removes `LessonDriver` next to
`RaceState`. DSN-75 and the PLAN rows updated.

Tests: `tests/progression.rs::a_lesson_leg_finish_records_nothing`
(verified to fail with the guard disabled), `tests/race.rs::
restart_removes_the_lesson_driver`.

Gates (iteration 45, foreground): `cargo fmt --all -- --check` pass;
`cargo clippy --locked --workspace --all-targets --all-features -- -D
warnings` exit 0; `cargo test --locked --workspace` exit 0 (2292 passed, 0
failed; was 2290). No processes left running.

Not verified / open: no launcher (no `SessionMode` installs the driver;
`event_race_setup` still refuses crash rows); no lesson-pass credit
(separate consumer, design open); per-leg countdown banner/car hold and
HUD/racestat/nav rebinding to the swapped race are not inspected; no
UNK-35 evaluators. No original-data run; tests synthetic. Status:
implemented candidate, not independently checked.

---

# Last iteration — F21-B.4: in-session lesson leg driver (new-run iteration 44)

Selection: the previous review passed with no blocking findings. Its gap
list named the missing consumer: nothing wired `LegReport::from_gate_run`
to a live `RaceState`. A full lesson launch (new session mode, crash-row
world setup, reward/record isolation) is too broad for one change, so I
took the runtime half: the part that runs legs back to back inside a
session.

Change: `mm2_app::lesson::LessonDriver` (resource over `LessonSetup`) and
`drive_lesson` (`FixedLast`, chained after `advance_race`, registered in
`main.rs`). A non-final clear swaps `RaceState` to the next leg,
rebuilds the participant's `RaceProgress`, respawns the gate markers and
reseats the car through `ResetVehicle` (the `Teleported` re-anchor means
the jump is never a crossing). `advance_race` gained an optional
`LessonDriver` read so a non-final finish does not move the session to
`Results`; the last clear stores the once-only pass, a timeout ends the
session as a failed event. Retry stays the session's own restart (a
wreck already queues it — the driver never reports `Disabled`). DSN-75
(designed) in `docs/original-rules.md`.

Tests: `crates/mm2_app/tests/lesson_drive.rs` +4 (registered in
`tests/app.rs`; real `advance_race`/`drive_lesson`/`vehicle_reset`),
`lesson.rs` +5 unit.

Gates (iteration 44, foreground): `cargo fmt --all -- --check` pass;
`cargo clippy --locked --workspace --all-targets --all-features -- -D
warnings` clean; `cargo test --locked --workspace` exit 0 (2290 passed, 0
failed; was 2281). No processes left running.

Not verified / open: no launcher — no `SessionMode` installs the driver
and `event_race_setup` still refuses crash rows; each leg's finish still
lands in `ResultLedger` and would be recorded as an event finish if a
session were launched today (isolate before launch); no reward credit,
instruction flow, family evaluators (UNK-35); `racestat`/HUD are built
from leg 0 only. No original-data run; tests are synthetic. Status:
implemented candidate, not independently checked.

---

# Last iteration — F21-B.3: lesson setup + gate-run verdict adapter (new-run iteration 43)

Selection: the previous review passed with no blocking findings. F21-B.2's
sequencer had no consumer and no way to turn a real race outcome or a
`crash:N` row into its inputs. The full session driver (swap `RaceState`
per leg, reseat on retry, reward credit) is too broad for one change, so I
took the two pure joints it needs: how a leg's gate run becomes a report,
and how a Crash row becomes legs + sequencer.

Change: `LegReport::from_gate_run` (finish → `Cleared` on the race clock,
expiry → `Failed(TimedOut)`, disabled while racing → `Failed(Disabled)`,
undecided → `None`, finish beats a late disable) and
`mm2_app::race::lesson_race_setup` → `LessonSetup{key, legs, run}` via the
production catalog (DSN-74, designed). `event_race_setup` is untouched and
still refuses Crash rows.

Tests: `mm2_game` `tests/lesson.rs` +4, `mm2_app` `tests/lesson_setup.rs` +4
(registered in `tests/app.rs`).

Gates (iteration 43, foreground): `cargo fmt --all -- --check` pass;
`cargo clippy --locked --workspace --all-targets --all-features -- -D
warnings` exit 0; `cargo test --locked --workspace` exit 0 (2281 passed, 0
failed; was 2273). No processes left running.

Not verified / open: nothing launches a lesson — no `RaceState` per leg, no
world restore on retry, no reward credit, no instruction flow; family
evaluators (UNK-35) unrecovered, so the verdict is the gate-run baseline
only. No original-data (retail) run this iteration; tests are synthetic.
Status: implemented candidate, not independently checked.

---

# Last iteration — F21-B.2: lesson leg sequencer (new-run iteration 42)

Selection: the previous review passed with no blocking findings; its one
non-blocking item (london `exam1_1` is 15 rows, not 19, in
`docs/research/crashcourse.md` and the `lesson_def.rs` docs) is corrected.
F21-B's remaining pieces are the session loader, family evaluators
(UNK-35, unrecoverable from data), world restore and rewards. The loader
needs a defined sequencing/retry/credit contract first, so I took that as
the next small, testable slice rather than launching `crash:N` as a bare
gate run (which would risk counting a gate run as a lesson pass).

Change: `mm2_game::lesson::LessonRun` (DSN-73, designed) — pure state
machine over a lesson's legs: strict authored order; reports for another
leg / a superseded attempt / a non-running lesson are `Stale` (counted,
never applied); a failure ends the attempt; `retry` restarts the whole
lesson from leg 0 with cleared counters (the DMG-2 `RestartEvent`
reading; resume-at-failed-leg is unrecovered); `abandon`; `take_pass`
yields the pass exactly once (failed/quit/duplicate never). No new
dependency (manual `Display`/`Error`).

Tests: `crates/mm2_game/tests/lesson.rs` +12 (order, duplicate/out-of-
order, failure-then-late-clear, retry clears counters and stales the old
attempt, mid-lesson restart, pass-once, passed is terminal, quit yields
nothing, stale counting across attempts, zero-leg refusal).

Gates (iteration 42, foreground): `cargo fmt --all -- --check` pass;
`cargo clippy --locked --workspace --all-targets --all-features -- -D
warnings` exit 0; `cargo test --locked --workspace` exit 0 (2273 passed,
0 failed; was 2261). No processes left running.

Not verified / open: **no consumer** — nothing drives `LessonRun` from a
session or the race runtime, so no in-game behavior changed; nothing
launches `crash:N`. Retry-whole-lesson and order-as-gate-sequence are
designed readings, not recovered rules. Still open for F21-B: session
loader, world restore on retry, instruction flow, family evaluators
(UNK-35), lesson-only reward credit. Status: implemented candidate, not
independently checked.

---

# Last iteration — F21-B.1: Crash Course lesson legs (new-run iteration 41)

Selection: the previous review passed with no blocking findings. F20's
remaining items need semantics decisions (UNK-9, B.3c) or a bridge
level-change fixture, while F21-B (queued, deps F21-A.1 implemented)
had nothing at all: every `crash:N` event is refused by
`race_definition`. Broad F21-B (loader + sequencing + retry + family
evaluators) does not fit one iteration, and the family rules are
unrecovered (UNK-35), so I took the data-side first slice.

Change: `mm2_content::lesson_def::lesson_legs(catalog, lesson,
difficulty)` turns each sub-event of the difficulty's table into a
`RaceDefinition` (start pose row 0, ordered gates rows 1.., one pass,
`TimeLimit` → ticks with 0 = untimed, `AmbDensity` → traffic). A lesson
is a sequence of legs (exams chain 2–3). A leg that cannot run fails the
whole lesson with a named `LessonBuildError`. `LessonLeg::objective`
keeps the inferred `Event` family as the dispatch key. Ledger DSN-72
(designed, labelled gate-run baseline — explicitly *not* the lesson's
pass criterion), `docs/research/crashcourse.md` gains the measured leg
sizes. `race_def.rs` helpers (`time_limit_ticks`, `event_params`,
`checkpoint`, `start_slots`) became `pub(crate)`; no behaviour change.
`mm2-inspect crash-course` prints each leg; `--strict` fails per
lesson/difficulty that does not build.

Tests: `crates/mm2_content/tests/lesson_def.rs` +8, inspect +1.
Retail (`fnv1a64:e91e6cd4b2ae30d9`): `mm2-inspect crash-course <retail>
--strict` exit 0, 52/52 lesson×difficulty sets build.

Gates (iteration 41, foreground): `cargo fmt --all -- --check` pass; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` exit 0 (after fixing one clone-on-copy); `cargo test --locked --workspace` exit 0 (2261 passed, 0 failed; was 2252). No processes left running.

Not verified / open: no loader, no leg sequencing, no retry/world
restore, no instruction flow, no family evaluators (UNK-35), no reward
credit — nothing launches a lesson yet and a gate run is not counted as
passing one. Row-ordering-as-gates and the `TimeLimit` unit are
inferences. Status: implemented candidate, not independently checked.

---

# Last iteration — F20-C.3: respawn while chased (new-run iteration 40)

Selection: the previous review passed with no blocking findings. F20's
remaining open items are AC03 outcome semantics (UNK-9) and the density
option (B.3c), both needing a semantics decision; of the spec's listed
edge cases, "respawn while chased" had no test (a race finish is covered
by the stand-down test; "multiple offenses one tick" has no offense
model). Test-only slice; no production change, no ledger change.

Test (`tests/police.rs::respawning_while_chased_ends_the_chase_and_a_return_starts_a_new_one`):
a cop pursuing a racing player; the driver's reset is sent through the
production `session::spawn_resets` → `ResetVehicle` path to a point 1 km
away. Asserts the player was teleported, the cop is `Lost` within
`lose_after + 2 s` (not carried to the respawn, `z > -300`), report
`committed/gave_up/pursuing == 1/1/0`, lights off. A second reset back
into sight starts nothing during the cooldown, then after
`cooldown + reaction` a second separate chase (`committed == 2`).

Gates (iteration 40, foreground): `cargo fmt --all -- --check` pass;
`cargo clippy --locked --workspace --all-targets --all-features -- -D
warnings` exit 0; `cargo test --locked --workspace` exit 0 (2252 passed,
0 failed; was 2251). No processes left running.

Not verified: synthetic flat ground, no road graph, not a retail chase;
no mutation check that the test fails without the reset. Open for F20:
outcome/bust semantics (UNK-9), density option (B.3c), bridge level
change. Status: implemented candidate, not independently checked.

---

# Last iteration — F20-C.2: long-chase bound test (new-run iteration 39)

Selection: the previous review passed with no blocking findings. Of F20's
open items, AC03 outcome semantics are unknown (UNK-9) and the density
option needs a new `Densities` field plus a semantics decision, so I took
AC04 ("active pursuer count and stuck recovery remain bounded during a
long chase"), which had only short-chase tests. Test-only slice; no
production change, no ledger change (it asserts the existing COP-9/COP-11
enhanced-policy numbers).

Test (`tests/police.rs`): four cops ringed around the start, cap 2, a
target circling at ~12 m/s for 3600 frames. Per frame: pursuer count ≤
cap and equal to both the cops' `Pursuing` phases and
`PursuitReport.pursuing`; `EmergencyLights` exactly on pursuing cops; four
cops always present; positions finite and within 600 m. At the end: peak
== cap (it was reached), commits past one per cop each follow a give-up,
and per-cop `escapes + turnarounds` ≤ frames / 207 + 1. Observed: peak 2,
committed 4, gave_up 2, per-cop recoveries 0–7.

Gates (iteration 39, foreground): `cargo fmt --all -- --check` pass;
`cargo clippy --locked --workspace --all-targets --all-features -- -D
warnings` exit 0; `cargo test --locked --workspace` exit 0 (2251 passed,
0 failed; was 2250). No processes left running.

Not verified: synthetic flat ground, teleported target, no road graph —
not a retail chase; I did not run a mutation check that the test fails if
the cap were not enforced (the cap lives in the pure `Pursuit::step`
gate, covered by its own unit tests). Open for F20: outcome/bust
semantics (UNK-9), density option (B.3c). Status: implemented candidate,
not independently checked.

---

# Last iteration — F20-C.1: cops recover like opponents (new-run iteration 38)

Selection: the previous review passed with no blocking findings; its
gaps (retail numbers unverified by the reviewer, density/activation
unknown) stay disclosed. B.3c needs a semantics decision, so I took the
F20 edge case "overturned pursuer" / AC04 (bounded stuck recovery) and
searched for what actually happens to a wrecked cop. Finding: the three
recovery resolvers (`damage::resolve_disabled`, `stuck::resolve_stuck`,
`recovery::resolve_recovery`) skip any identified vehicle with no `Player`
("unidentified object"), and cops are deliberately not `Player`s (COP-8).
Retail `vpcop` ships `vehcardamage`/`vehstuck`, so `equip_authored_vehicle`
armed detectors nothing answered: a wrecked or wedged cop stayed down
for the session.

Changes:
- `mm2_app::police::recovery_driver(player, is_cop)`: a cop resolves as
  `PlayerControl::Ai`; the three resolvers use it (query gains
  `Has<PoliceCar>`). Cops stay non-participants.
- `resolve_recovery`: a cop never falls back to the session `SpawnPoint`
  (the player's start — it would teleport the cop onto its target); it
  keeps its own landing/anchor.
- Ledger COP-14 (implementation choice, mirrors the opponent policy,
  UNK-13).
- Tests (3, in `tests/{damage,stuck,recovery}.rs`): a wrecked cop resets
  in place and repairs without restarting the session; a rolled, stuck cop
  is righted in place; a cop recovers to its own landing and not the
  spawn. Checked causal: all three FAIL with `recovery_driver` returning
  `None` for cops.

Retail, local: sf `--spawn=59,1.5,95,0 --headless --frames 1800`
`status=pass pol=19/19 pur=1/1/1,r1,nr2`.

Gates (iteration 38, foreground): `cargo fmt --all -- --check` pass;
`cargo clippy --locked --workspace --all-targets --all-features -- -D
warnings` clean; `cargo test --locked --workspace` exit 0 (2250 passed, 0
failed; was 2247). No processes left running.

Not verified: no retail scenario that wrecks a cop; nothing seen or
heard. Open for F20: outcome/bust semantics (UNK-9), long-chase bound
test, density option (B.3c). Status: implemented candidate, not
independently checked.

---

# Last iteration — F20-B.3b: Cruise roam cops (new-run iteration 37)

Selection: the previous review passed with no blocking findings; its
gaps (overlay unseen, retail evidence is the implementer's claim) stay
disclosed. The F20 spec's remaining slice is Cruise cops and density.
`mm2_content::cruise_police_roster` already built the `roam` lineup and
was tested, but nothing consumed it — Cruise fielded no cops. I took the
fielding alone; the density option (B.3c) needs a new `Densities` field,
menu row and a semantics decision, so it stays queued.

Changes:
- `mm2_app::police`: `cruise_roster` (logs and returns an empty roster
  when a city ships no readable roam record) and `insert_fleet` (fleet,
  policy, report, and the road graph only when cops spawned — the event
  path now shares it).
- `session.rs`: the free-roam arm fields the roam lineup for
  `SessionMode::Cruise` + city world + local authority only.
- Ledger COP-13 (implementation choice: the whole lineup is fielded
  because the retail activation rule is unknown, COP-7).
- Tests (`tests/police.rs`, real `load_session_world`, synthetic city):
  6 new — lineup/graph/ownership, no roam record, Host/Remote none, dev
  world none, in-sight chase vs far control, restart refield.

Retail, local (`--headless --frames 1800`): sf `pol=19/19`, london
`pol=20/20`, `status=pass` (spawn at its default: `pur=0/0/0`);
`--city sf --spawn=59,1.5,95,0` `status=pass pur=1/1/1,nr2`.
A first spawn guess left the car under the world (`status=fail`,
`wheels=0/4`) — an off-road spawn point, not a cop effect; not kept.

Gates (iteration 37, foreground): `cargo fmt --all -- --check` pass;
`cargo clippy --locked --workspace --all-targets --all-features -- -D
warnings` clean (exit 0); `cargo test --locked --workspace` exit 0 (2247
passed, 0 failed; was 2241). No processes left running.

Not verified: no capture/listen of a Cruise chase; cost of ~20 extra cars
beyond the smoke; roam activation and cop density unknown/open.
Status: implemented candidate, not independently checked.

---

# Last iteration — F20-B.3a: police debug overlay and loader coverage (new-run iteration 36)

Selection: the previous review passed with no blocking findings. Its
gaps named two things I could close: no test asserts that
`load_session_world` inserts `PoliceNav`, and the F20 spec's debug
state/route visibility (req 6) was open. I took both (B.3 split: Cruise
cops and cop density stay B.3b).

Changes:
- `mm2_game`: `ChaseRoute::remaining()`; `DevOverrides.police_debug`.
- `mm2_app::police_debug` (new, a real consumer of the chase state): pure
  `debug_lines` (phase pole; goal cross at `last_seen` and the route or
  an unrouted straight line, only while pursuing) and `draw_police_debug`
  through gizmos, gated by `enabled` (session config). `--police-debug`
  CLI flag, scheduled in the windowed app only (headless draws nothing).
  Ledger COP-12 (implementation choice).
- `tests/police.rs`: three tests drive the real loader over a synthetic
  city (`traffic::synthetic_psdl`/`bai_bytes` made `pub(crate)`): cops get
  the graph; a copless event does not; a missing BAI leaves it absent.
  One test runs the overlay system through a live chase.

Retail, local: `--headless --frames 2400 --event checkpoint:8
--police-debug` `status=pass`, `pur=1/1/1,r1` (unchanged). A windowed
`--frames 700 --screenshot` ran but no cop was in frame, so the overlay
has not been seen drawn.

Gates (iteration 36, foreground): `cargo fmt --all -- --check` pass;
`cargo clippy --locked --workspace --all-targets --all-features -- -D
warnings` clean; `cargo test --locked --workspace` exit 0 (2241 passed, 0
failed; was 2231). No processes left running.

Not verified: how the overlay looks; Cruise cops, cop density, bust
outcome still open. Status: implemented candidate, not independently
checked.

---

# Last iteration — F20-B.2: a chasing cop follows the road (new-run iteration 35)

Selection: the previous review passed with no blocking findings; its one
non-blocking note (a doc comment claiming a siren failure is counted
"once per activation") was corrected in `audio.rs` — it retries every
update. The handoff queued road-aware chase as F20-B.2, so I took that
alone (Cruise cops/density/debug overlay stay B.3). Retail pursuit routing
is unknown (UNK-9), so everything is designed and disclosed (ledger COP-11).

Changes:
- `mm2_game::police`: `ChaseRoute` (polyline + forward-only cursor,
  `follow`/`lateral`), `ChaseNav` component (`aim`: direct when the target
  is in view within 30 m, else a `NavGraph::drive_line` road line to the
  chase goal — the last-seen point, never the target's true position —
  re-planned on 25 m goal drift / 30 m off-line, rate-limited 1.5 s, 4 s
  after a failed query; no graph/line → straight aim reported `Unrouted`).
- `mm2_app::police`: `PoliceNav` resource (the same `build_for_routing`
  graph the opponents use, inserted by `load_session_world` only when cops
  spawned, removed at teardown); `chase_input(aim, goal)` steers for the
  aim and arrives/stops against the goal, caps pace for a bend, and
  reverses round (`scripted::begin_turnaround`, shared escape machinery,
  not counted as a stuck escape) when the aim is >~100° off a slow car;
  `PursuitReport.planned/unrouted`, smoke `pur=c/g/p[,r<N>][,nr<N>]`.
- A retail probe (temporary stderr trace, removed) showed the first cut
  wedged a cop that is released facing away from the road line: its
  authored heading is not along the road, and full lock into the kerb went
  nowhere until the stuck detector fired 1.5 s later. The turn-around law
  fixed that; the cop now reverses round and runs the line at ~15 m/s.
- Tests: 7 `mm2_game` unit, 3 graph tests in `mm2_game/tests/nav.rs`
  (route aim vs straight, rate-limit/drift, unreachable-goal bound,
  off-route re-plan), 4 `mm2_app` unit (aim vs goal, bend cap, turn-around,
  report format), 3 `tests/police.rs` (cop clears a wall across the
  straight line along a road bend — and is held by the wall without the
  graph —, failed-query bound, teardown removes the graph).

Retail, local (`--headless --frames 2400`): sf checkpoint 8 `pol=4/4
pur=1/1/1,r1` (was `1/0/1`: the cop now turns round, follows the road loop
and gives up at the 8 s no-contact bound — a behaviour change, not a
claim of better play), `--pro` `pol=8/8 pur=2/1/2,r9,nr2`, london
checkpoint 2 `pur=0/0/0`, sf checkpoint 0 no `pol`/`pur`; all
`status=pass`.

Gates (iteration 35, foreground): `cargo fmt --all -- --check` pass;
`cargo clippy --locked --workspace --all-targets --all-features -- -D
warnings` clean (exit 0); `cargo test --locked --workspace` exit 0 (2231
passed, 0 failed; was 2215). No processes left running.

Not verified: no capture or listen of a real chase, so how it reads on
real streets is unseen; level/one-way correctness is only the router's;
the 8 s lose bound is not scaled to route length; debug route overlay
(F20 req 6), Cruise cops, bust/arrest outcome all still open. Status:
implemented candidate, not independently checked.

---

# Last iteration — F20-B.1: a chasing cop runs its siren and light bar (new-run iteration 34)

Selection: the previous review passed with no blocking findings, and
the handoff named siren + lights as the next F20 leg. I took that slice
alone (road-aware chase and Cruise cops stay queued as B.2/B.3). Pursuit
trigger and cadence are unknown in retail (UNK-9/UNK-25), so both are
designed and disclosed (ledger COP-10).

Findings from the retail model (`mm2-inspect car/pkg vpcop`): `SRN0..3`
are flat 4-vertex quads at mtx origins along the bar (so far drawn
permanently lit as ordinary parts); `SIREN0/1` are 24-vertex box pieces
with no mtx (the housing). Retail audio already authors the siren
(`SIREN_FLAG` on `vpcop`, shared opponent program) and F07-B.7's `Siren`
machine already served non-player cars — it just had no activator.

Changes:
- `mm2_game::police`: `EmergencyLights` component (flash clock,
  `lit_side`, `advance`) + `LIGHT_BAR_HALF_PERIOD` (0.25 s).
- `mm2_app::police::police_pursuit`: inserts/advances/removes it exactly
  while the cop is `Pursuing` (reacting, lost, idle, stood-down: dark).
- `mm2_app::audio::siren_follow_lights`: lights → opponent siren program
  on a flagged non-player car, through the existing `Siren`/`siren_drive`/
  `MAX_SIRENS`; no program → counted failure, no substitute. Chained
  ahead of `siren_toggle`/`siren_drive` in the windowed and headless apps.
- `mm2_app::car_visual`: `SRNn` quads become `GlowKind::Siren(n % 2)`
  flares; `update_glows` lights one half at a time while the car has
  `EmergencyLights`; the `SIRENn` boxes stay solid.
- Tests: 2 `mm2_game`, 3 new `tests/light_bar.rs` (synthetic model through
  `spawn_vehicle_model`), 5 `audio`, 1 new + 4 extended `police`.

Retail, local (`--headless --frames 2400`): sf checkpoint 8 `pol=4/4
pur=1/0/1 aud=…/12w/1y`; `--pro` `pol=8/8 pur=2/1/2 …/13w/1y` (the cop
that gave up has no siren); london checkpoint 2 `pur=0/0/0` and sf
checkpoint 0 carry no `w/y`; all `status=pass`.

Gates (iteration 34, foreground): `cargo fmt --all -- --check` pass;
`cargo clippy --locked --workspace --all-targets --all-features -- -D
warnings` clean (exit 0); `cargo test --locked --workspace` exit 0 (2215
passed, 0 failed; was 2204). No processes left running.

Not verified: that the flares look right (no capture taken; flare colour
comes from the authored SRN shader, flash rate/trigger designed); that the
siren is audible (headless, no output device); road-aware chase; Cruise
cops; the `Explosion sample` consumer. Status: implemented candidate, not
independently checked.

---

# Last iteration — F20-A.3: the cops detect, chase and give up (new-run iteration 33)

Selection: the previous review passed with no blocking findings. The
fielded cops (A.2) stood handbrake-held with no behavior, so I took the
next queued leg, A.3: the detect → engage → pursue → lost machine. The
retail rules are unknown (COP-4/UNK-9); the machine follows the only
documented shape (COP-1 chase on sight, COP-2 escape by leaving sight)
with disclosed, designed numbers (ledger COP-9) — no stars, bust or
arrest.

Changes:
- `mm2_game::police`: pure `Pursuit` (component) / `PursuitPhase` /
  `PursuitPolicy` (resource; detect 90 m, contact 140 m, reaction 0.75 s,
  lose 8 s, stand-down 6 s, 4 pursuers) / `Sighting` / `PursuitEvent`;
  `last_seen` is the only place a chase learns the target's position.
- `mm2_app::police`: `police_pursuit` (nearest racing `PlayerControl::Local`
  `PlayerVehicle`; sight ray cast against `GameLayer::World` only; pursuer
  cap filled in authored order), `chase_input` (reuses the opponents'
  `steer_toward`/`CarLimits`/`bearing_throttle`/`recovery_input`/
  `watch_stuck` — no second steering law), `PoliceDrive` component,
  `PursuitReport`. Scheduled in both the windowed and headless apps;
  `load_session_world` inserts/`drive_session` removes the policy and
  report with the fleet. Smoke `pur=<chases>/<given up>/<peak>` beside
  `pol=` only when cops are fielded.
- Tests: 8 pure `mm2_game`, 5 pure `mm2_app`, 8 integration in
  `tests/police.rs`. A finding worth knowing: the shared synthetic test
  car barely yaws under full lock, so integration cops are authored
  facing the player; turning is covered by the pure sign test and retail.

Gates (iteration 33, foreground): `cargo fmt --all -- --check` pass;
`cargo clippy --locked --workspace --all-targets --all-features -- -D
warnings` clean; `cargo test --locked --workspace` exit 0 (2204 passed, 0
failed; was 2182). Retail, local (`/Users/linus/coding/rust-mm2/retail`,
`--headless --frames 2400`): sf checkpoint 8 `pol=4/4 pur=1/0/1`; `--pro`
`pol=8/8 pur=2/1/2`; london checkpoint 2 `pol=1/1 pur=0/0/0`; sf
checkpoint 0 neither field; all `status=pass`. No processes left running.

Not verified: that a chase looks or plays right (headless; straight at the
goal, not road-aware — a cop can be walled off by geometry the ray does not
model beyond `World` colliders); every number is designed, not original;
siren/lights/audio (the `Siren` program on the opponent side is the hook);
Cruise `roam` cops; a bust/arrest outcome (unknown, none invented); no debug
route overlay yet (F20 req 6 — state is visible via `pur=` only). Next in
F20: A.4/B — siren + lights on engage, road-aware chase through the nav
graph, Cruise cops. Status: implemented candidate, not independently
checked.

---

# Last iteration — F20-A.2: the event's police stand on the road (new-run iteration 32)

Selection: the previous review passed with no blocking findings (its
non-blocking gap — `PoliceReport::scan` skipping an unparsable extra
aimap — does not touch the roster producers and is left open). F20's
roster existed with no consumer, so I took the next queued leg, A.2:
field the authored cops. Chase behavior is unverified (COP-4/UNK-9), so
no pursuit was invented; this slice is the spawn only.

Changes:
- `mm2_app::police` (new): `spawn_police`, `PoliceCar`, `PoliceFleet`
  (authored / spawned / unplaceable / load_failed, `smoke_detail`),
  `staging_yaw`. `EventSetup.police` is built from the same
  `event_aimap` read as the opponent roster; `load_session_world` fields
  it beside the opponents and inserts/removes `PoliceFleet` with the
  session. Cars use `load_opponent` + the shared
  `opponents::equip_authored_vehicle` (pure extraction from
  `spawn_opponents` — its tests are unchanged and pass), carry no
  `Player`/`RaceProgress`, hold the handbrake, and are not fielded when
  `authority != Local` (MP-4, same as opponents). Bad rows are skipped
  and counted, never padded.
- Smoke `pol=<spawned>/<authored>[,uns<N>][,fail<N>]`, absent on cop-less
  sessions (existing records bit-identical).
- Measured, not kept: Cruise `roam` headings vs nearest vehicle lane
  tangent are inconclusive (16/39 rows >12 m from any lane; of 23 near
  one, 10 within 30° of the lane axis, 13 across) — the yaw convention
  stays inferred. Ledger COP-8 records this.
- Tests: `crates/mm2_app/tests/police.rs` (5; reuses the opponents test
  harness, whose helpers became `pub(crate)`).

Gates (iteration 32, foreground): `cargo fmt --all -- --check` pass;
`cargo clippy --locked --workspace --all-targets --all-features -- -D
warnings` clean; `cargo test --locked --workspace` exit 0 (2182 passed, 0
failed; was 2177). Retail, local (`MM2_RETAIL=/Users/linus/coding/rust-mm2/retail`):
headless `--event checkpoint:8` on sf: `pol=4/4`, `--pro` `pol=8/8`;
london `checkpoint:2` `pol=1/1`; sf `checkpoint:0` no `pol=`; all
`status=pass`. No processes left running.

Not verified: that a cop is *visible* or correctly oriented (headless; no
render check, heading unit/axis inferred); that standing cops are right
at all before pursuit exists (they are an authored placement, not
behavior); Cruise cops (not fielded: how many of the 19–20 `roam` rows
are active is unknown, COP-7); cop-density option (no field to read).
Next in F20: A.3 detect → engage → pursue → lost state machine (enhanced
policy where the original is unknown, labelled), then siren
(`Siren::activate` on the opponent program) and lights, Cruise cops.
Status: implemented candidate, not independently checked.

---

# Last iteration — F20-A.1: the police roster (new-run iteration 31)

Selection: the previous review passed with no blocking findings, and
F27's remaining items are process-level evidence or a menu-hosted lobby
(a startup-time `HostLink` the app cannot yet open from the menu — not a
small change). F20 (single-player police) was the one original feature
with no code at all ("no police logic"), so I took its content-selection
leg: F20-A's own first requirement ("import/discover police content,
session density and event restrictions"), reusing the opponent-roster
producer's shape. Pursuit rules are unverified (COP-4 / UNK-9), so no
behavior was invented.

Changes:
- `mm2_game::police`: `PoliceSpec` (vehicle, position, heading reduced to
  (−180, 180], raw tail, source line), `PoliceRoster`, `PoliceIssue`
  (`MissingVariant`, `CountMismatch`, `Unplaceable`), `normalize_heading`.
- `mm2_content::police`: `police_roster` (event, shares `event_aimap` with
  the opponent producer), `cruise_police_roster` (the `roam` record, same
  Amateur/Professional split + fallback), `PoliceReport::scan` (events at
  both difficulties + Cruise + extra stems + wired ids vs the vehicle
  catalog; crash courses counted `unsupported`, never dropped).
- `mm2-inspect police [--city] [--strict]`.
- Ledger COP-5/6/7 + `research/aimap.md`. Measured facts: heading column
  spans −270…535 (my first draft assumed ±180 and the audit's one
  `Unplaceable` row — `535` on london `roam` — disproved it, so the contract
  now normalises instead of rejecting); wired count == table `Cops` on all
  checkpoint builds; Cruise rosters 19/20 (sf) and 20/20 (london); london
  `crash10/11` `[Police]` rows are non-cop cars (F21's concern).

Gates (iteration 31, foreground): `cargo fmt --all -- --check` pass;
`cargo clippy --locked --workspace --all-targets --all-features -- -D
warnings` clean; `cargo test --locked --workspace` exit 0 (2177 passed, 0
failed; was 2163). Retail, local with
`MM2_RETAIL=/Users/linus/coding/rust-mm2/retail`: `cargo test -p
mm2_content --test police` 11 passed incl. the retail one (not a skip);
`mm2-inspect police --strict` exit 0 on both cities. No processes left
running.

Not verified: anything at runtime — nothing spawns a cop; the heading
unit is inferred (degrees, matches the aimap's own column comment; no
in-world check); what the other tail columns mean (StartLink/Dist/Mode/
Lane/Patrol per the file's comment do not fit the column counts); how many
`roam` rows are active at once and what the Cruise cop-density option
scales; the `_cop` tuning variants (likely unread, like `_opp` — not
checked). Next in F20: A.2 spawn the cops (session-owned `vpcop` cars,
count bounded, siren state), then A.3 detect/pursue/lose. Status:
implemented candidate, not independently checked.

---

# Last iteration — F27-B.4c commentary: the announcer calls the gold (new-run iteration 30)

Selection: the previous review passed with no blocking findings; the
remaining closable F27 item named in the handoffs was commentary (the
cue vocabulary was audited in iteration 9, nothing voiced it). Reading the
retail `cnrsf.csv`/`cnrlondon.csv` rows against the wave inventory showed
the suffix draw was wrong for them, so that was repaired first.

Changes (three commits):
- `mm2_game::audio::draw_cue_suffix` now draws `add + 1 ..= end`
  (`mm2_formats::spchdata` validate flags `add >= end`). Evidence: the 14
  C&R families of a table partition one wave pool per speaker
  (`as1cops01`–`11`, `as1robrob01`–`10`; london `al1cops01`–`12`) through
  `<end>,<add>` — e.g. `ROBDROPLOOT 6,5` is `as1cops06` — while the old
  `add + 1 + rng % end` named waves up to `as1robrob19`. Weather/time rows
  have `add` 0, so the pre-race draw and its seeded stream are unchanged.
  AUD-12 / `research/audio.md` corrected.
- `GoldEvent::call` / `GoldView::call_since` (`mm2_game::gold`, pure) and
  `mm2_content::cnr::commentary_cue` (side × get/drop/stash/recover → family;
  every authored family is reachable, test-enforced).
- `mm2_app::cnrvoice::cnr_commentary_voices` + `CnrCommentary` (bound in
  `cnr::start_match`, removed at teardown, registered in the app and the
  smoke schedules). Authority voices `CnrEvent`s, a client the replica
  change; lines queue ≤ 3, stale (> 6 s) dropped counted, a missing
  wave/row counts `failed` and says nothing. Smoke `cnr=` gains `,say<N>`.
  Ledger CNR-13, `research/cnr.md`.

Gates (iteration 30, foreground): `cargo fmt --all -- --check` pass;
`cargo clippy --locked --workspace --all-targets --all-features -- -D
warnings` clean; `cargo test --locked --workspace` exit 0 (2163 passed, 0
failed; was 2145). Retail, local with `MM2_RETAIL=/Users/linus/coding/rust-mm2/retail`:
`cnrvoice::retail_every_family_window_names_only_shipped_waves` passes (not
a skip); `network two_retail_processes_decide_a_cops_and_robbers_match`
passes (73 s) and now asserts the client's `say>=2` — measured `say2`. No
processes left running.

Not verified: that anything is *audible* (headless, no output device —
voice entities and stems are the evidence); the host's process-level count
(its record is read after `quit`, match gone — covered only by the 10
`mm2_app` unit-level tests); when the original actually fires each family
(CNR-13: designed), including the FFA → `ROB*` choice; client lines for
frames lost on the wire; contested pickups / multi-client cycle /
impairment at process level / rendering. Open: menu-hosted lobby offer,
F27-AC01..06. Status: implemented candidate, not independently checked.

---

# Last iteration — F27-B.4c review gaps: the re-plan wait is tested, and the first frame keeps it (new-run iteration 29)

Selection: the previous review passed with no blocking findings; its
verification gaps named two small, closable items in the `--bot` Cops &
Robbers driver — the wait/drop-stale-guide path had no test (only the
`guide_stale` predicate did), and on a car's very first frame the
`plan_wait` armed on the throwaway `ScriptedBot` value was lost (one extra
router query). Process-level gaps (client carry, contested pickups,
impairment, rendering) are not closable in one unattended change.

Change (`mm2_app::scripted`): the wait logic moved out of the system into
`nav_replan(wait, plan)` → `NavReplan::{Waiting, Planned, Failed}`; the
planner closure runs only when the wait has run out, and a failure re-arms
`NAV_REPLAN_FRAMES`. When the car has no `ScriptedBot` yet, a failed plan
inserts one carrying the armed wait. 1 unit test
(`a_failed_plan_waits_a_window_before_the_router_is_asked_again`: failure
arms 120, the next 120 frames never call the planner, the following frame
does, exactly 2 planner calls). Behaviour is otherwise unchanged.

Gates (iteration 29, foreground): `cargo fmt --all -- --check` pass;
`cargo clippy --locked --workspace --all-targets --all-features -- -D
warnings` clean (a first `drop_non_drop` in the new test was fixed);
`cargo test --locked --workspace` exit 0 (240 lib, 767 app, 99 network, 0
failed). Retail two-process decided-match test not re-run (no behaviour
change on the success path). No processes left running.

Not verified: the system-level failure path with a real unreachable
objective, the new-bot-insert branch (no test builds the system), a client
that carries/delivers, contested pickups, impairment/late-join at process
level, rendered output. Open: commentary, menu-hosted lobby offer,
F27-AC01..06. Status: implemented candidate, not independently checked.

---

# Last iteration — F27-B.4c review nit: the evidence driver stops re-planning every frame (new-run iteration 28)

Selection: the previous review passed with no blocking findings. Its one
code finding (non-blocking) was in the `--bot` Cops & Robbers driver: when
`plan_nav_route` fails (no graph, or endpoints unreachable) while a guide is
unset or stale, the router was re-queried every frame and a stale guide for
the previous objective kept being chased. Its other gaps (multi-client
cycle, contested pickups, impairment, rendered output) are not closable in
one small unattended change, so I repaired the finding.

Change (`mm2_app::scripted`): `ScriptedBot.plan_wait` + `NAV_REPLAN_FRAMES`
(120). A failed plan arms the wait (no router query until it runs out) and
drops a stale guide, so the car aims straight at the objective as the cruise
bot does instead of chasing the old goal. `guide_stale` factors the
"missing or objective moved > 2 m" test out of the system; the goal-carrying
guide still never re-anchors. 1 unit test (`a_guide_is_stale_only_when_its_
objective_moved`: missing, within 2 m, beyond 2 m, authored guide).

Gates (iteration 28, foreground): `cargo fmt --all -- --check` pass;
`cargo clippy --locked --workspace --all-targets --all-features -- -D
warnings` clean; `cargo test --locked --workspace` all binaries ok (239 lib,
767 app, 99 network, 0 failed). Retail evidence, run here with
`MM2_RETAIL=/Users/linus/coding/rust-mm2/retail`: `network
two_retail_processes_decide_a_cops_and_robbers_match` is not a skip — 72.9 s,
host `Delivered` tick 12370 → `Ended(PointLimit, Player(0))`, client
`driver=parked phase=results`. (The external gate has no retail install, so
that test skips there; this local run is the only process-level evidence.)
No processes left running.

Not verified: the failure path of the planner itself with a real unreachable
objective (only the staleness predicate is unit-tested), a client that
carries/delivers, contested pickups, impairment/late-join at process level,
rendered output. Open: commentary, menu-hosted lobby offer, F27-AC01..06.
Status: implemented candidate, not independently checked.

---

# Last iteration — F27-B.4c (decided two-process match): the `--bot` driver plays Cops & Robbers (new-run iteration 27)

Selection: the previous review passed (no blocking findings); its gaps
were process-level evidence. The largest explicit open item on F27 was
"a two-process match that is *decided* (pickup, delivery)": nothing had
ever driven a car to the gold in a real process pair, so the decided
frame, the client's Results state and the winner agreement were only
unit/socket evidence.

Changes (one coherent slice, committed as separate commits):
- `NavGraph::drive_line` (mm2_game): the routed lane arcs sampled and
  bracketed by the query points; the router's error, never a guessed
  straight line. One synthetic test through a junction + the
  unreachable reverse trip.
- `scripted::cnr_target` + a nav-planned `ScriptedRoute` (`goal` set):
  with no race, `--bot` chases the gold, then its side's delivery marker
  from `CnrHost`/`CnrReplica`. **No re-anchor for these guides**: the
  first attempt "delivered" at tick 947 via the bounded re-anchor
  teleporting onto the route end — a faked delivery, removed before any
  evidence was recorded. 3 unit tests (`cnr_target`).
- `cnr_host_step` logs each `CnrEvent` (`cops and robbers event …`).
- `mm2-inspect cnr` site-reach audit + `GoldMatch::opening_sites`
  (1 test): **sf 12/44, london 14/46** sites lie on a routable vehicle
  lane (15 m horizontal, 5 m vertical); sf has 78 seeds in 0..4096 whose
  opening draw is all on lanes (london 99). Most retail sites are off the
  road, so a road-following driver finishes only some rounds — recorded
  in `docs/research/cnr.md` + ledger CNR-12 as measured data, no rule claim.
- Smoke `cnr=` gains `win=p<id>|s<side>|tie` after `dec1`.
- `MM2_RETAIL`-gated `net_drive::two_retail_processes_decide_a_cops_and_robbers_match`
  (+ `Proc::until_within`): host `--bot` seat + parked joined client on
  seed 1291, `100pts` FFA. 3/3 runs (~73 s each): host `Picked` (tick
  ~760) → `Delivered` (tick 9.5k–12.4k) → `Ended(PointLimit, Player(0))`;
  client `seats2,solo2,dec1,win=p0,phase=results`.

Gates (iteration 27, foreground): `cargo fmt --all -- --check` pass;
`cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`
clean; `cargo test --locked --workspace` exit 0 (2145 passed, 0 failed;
was 2139). A first full run caught `--seed` without `--host` being a
deliberate usage error (`mm2_host_flag_gates_are_named_exits`); I had
relaxed it for local exploration and reverted that, keeping the gate.
No processes left running.

Not verified: a client that picks up/carries/delivers (AC01 multi-client
cycle), contested pickups (AC02), impairment/late-join at process level
(AC05), rendered output, the client's carrier mass at process level. The
winner agreement reads the host's *log line* against the client's record
(the host's own record after `quit` has no match cells). Open: commentary,
menu-hosted lobby offer, F27-AC01..06. Status: implemented candidate, not
independently checked.

---

# Last iteration — F27-B.4c (client-load leg): a client's predicted car carries the gold's mass (new-run iteration 26)

Selection: the previous review passed with no blocking findings; its
gaps are process-level/rendered evidence that a single unattended code
change cannot add. The smallest explicit open item on F27 was "the
client car's carrier mass is not applied to its local prediction": a
joined carrier drove at base mass while the host simulated it heavier
(F27-AC04 holds on the host only).

Change: `mm2_app::cnr::reconcile_gold_load` (already registered on every
role) now also takes `Option<Res<CnrReplica>>` and `Res<Session>`. With
no `CnrHost` it reads the carrier from the replica
(`GoldView::carrier`) and the load from the session's own
`SessionMode::CopsAndRobbers(settings).rules(RACE_TICK_HZ).load` —
nothing new on the wire. The authority path is unchanged (`load_for`);
the recorded-base write keeps apply-once/exact-restore, and removing the
replica at teardown strips the load. Handling scalar stays unapplied.

Tests (`mm2_app::cnr`, synthetic): replica names the carrier → +500 kg
from the HalfTon setting, applied once, only the carrier; transfer moves
the load and restores base mass/inertia exactly, replica removal strips
it; weightless gold loads nothing; a replica outside a C&R session loads
nobody. Docs: `net.md`, `original-rules.md` CNR-12, PLAN.

Gates (iteration 26, foreground): `cargo fmt --all -- --check` pass; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` clean (one `field_reassign_with_default` in a new test fixed first); `cargo test --locked --workspace` exit 0 (2139 passed, 0 failed; was 2135 + the 4 new tests). No processes left running.

Not verified: process-level check of the client's mass, a decided
two-process match, rendered output. Open: commentary, menu-hosted lobby
offer, F27-AC01..06. Status: implemented candidate, not independently
checked.

---

# Last iteration — F27-B.4c (decided-repeat leg): a lost decided frame no longer strands a client (new-run iteration 25)

Selection: the previous review passed with no blocking findings; its
gaps (retail-gated test unverified by review, no pickup/delivery in two
processes, no rendered evidence) are not closable by a single unattended
code change. Reading F27-AC05 ("agree under packet loss/reordering")
against `publish_cnr` found a real defect instead.

Finding: `publish_cnr` repeats on *match* time (`PUBLISH_EVERY_TICKS`),
but a decided match stops its clock (`GoldMatch::tick` returns early; the
host step is idle outside `Playing`). So the decided frame went out
exactly once, on change. Any lost/dropped copy left a joined client on
the live HUD with no Results screen, permanently.

Change: `cnrnet::publish_cnr` gains a `Local` run counter; while the
view has an outcome the final frame repeats every `DECIDED_REPEAT_RUNS`
= 120 runs (rendered frames, ~2 s), reset on every send. Undecided
behaviour is unchanged. The client stage already drops equal-freshness
repeats as `stale`, so the repeat is harmless once received (the
`stale=` smoke cell will now count them after a decision). Docs:
`docs/research/net.md`, PLAN.

Test: `network::net_app::a_decided_match_survives_its_first_frame_being_
lost` — a wire peer behind `ImpairProxy`; Down `loss: 1.0` while the
decided frame is published (asserts the proxy dropped it and `sent==1`),
link healed, host repeats within the cadence (not every frame), peer
decodes a view equal to the host's decided view.

Gates (iteration 25, foreground): `cargo fmt --all -- --check` pass; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` clean; `cargo test --locked --workspace` exit 0 (2135 passed, 0 failed; was 2134 + the new test). No processes left running.

Not verified: random-loss/reorder matrix over undecided C&R frames,
late join (C&R is closed to it), process-level impairment of a C&R
match, any rendered output. Open: commentary, menu-hosted lobby offer
(note: no menu lobby exists at all — hosting is CLI `--host`),
client carrier-mass prediction, a decided two-process match;
F27-AC01..06. Status: implemented candidate, not independently checked.

---

# Last iteration — F27-B.4c (two-process leg): a started match across two real processes (new-run iteration 24)

Selection: the previous review passed with no blocking findings; its
first gap — no two-process host+client run of a Cops & Robbers match —
is the one a single machine can close without a GPU.

Finding (a real defect in the evidence harness, not the game): the
headless smoke app (`smoke.rs`, what `--headless` runs) registered
`publish_cnr`/`apply_cnr` but not the systems that seat, step and end the
match, so a hosted headless match never advanced past its opening frame
(first run: host `cnr=sent1`, client `landed1,seats0`).

Change:
- `smoke.rs`: `enroll_cnr_participants → cnr_host_step →
  end_decided_match → reconcile_gold_load` as their own `FixedLast` chain
  after `resolve_recovery` (the existing chain is at bevy's 20-tuple
  limit); `end_replicated_match` after `apply_cnr`; `CnrEvent` message.
- Record gains ` cnr=sent,landed,stale,ref` plus, while a match is
  visible, `seats,solo,rob,cop,red,blue,dec`. Absent with no match/frame,
  so other records are unchanged.
- `network::net_drive::two_retail_processes_play_a_started_cops_and_
  robbers_match` (skips without `MM2_RETAIL`).

Evidence: `MM2_RETAIL=/Users/linus/coding/rust-mm2/retail cargo test
--locked -p mm2_app --test network two_retail_processes_play` passes:
host `cnr=sent18`, client `cnr=sent0,landed16,stale0,ref0,seats2,solo0,
rob1,cop1,red0,blue0,dec0`. Real processes, loopback, headless, retail
sf. The host's own seats are not printed (its record is taken after
`quit`, parked in the lobby), so host-vs-client seating agreement is not
asserted, only that the client sees both sides seated.

Gates (iteration 24, foreground): `cargo fmt --all -- --check` pass;
`cargo clippy --locked --workspace --all-targets --all-features -- -D
warnings` clean; `cargo test --locked --workspace` exit 0 (2134 passed, 0
failed; was 2125 + the new test skipping without `MM2_RETAIL`). No
processes left running.

Not verified: a gold pickup, delivery or decided match in two processes
(nobody drives to the gold), the client's Results screen, any rendered
output, impairment. Open: commentary, menu-hosted lobby offer, client
carrier-mass prediction; F27-AC01..06. Status: implemented candidate,
not independently checked.

---

# Last iteration — F27-B.4c (rematch-net leg): the match-over screen for hosted and joined matches (new-run iteration 23)

Selection: the previous review passed with no blocking findings; its
gap "hosted and joined matches still freeze on the HUD readout after the
match is decided" is the one a single process can close honestly. The
visual/GPU gaps and the two-process run stay out of reach here.

Change:
- `cnr::end_decided_match` now acts for any authority (was `Local`
  only), so a hosted decided match opens `Results` on the host.
  `publish_cnr` already sends from `Results`, so the decided frame still
  reaches the peers.
- `cnr::end_replicated_match` (new, `main.rs` after `apply_cnr`): a
  client whose `CnrReplica` reports an outcome moves `Playing → Results`.
- `results.rs`: `ResultsKind` (Race / CnrLocal / CnrNetworked) picks the
  rows; a networked match has one row, `Back to lobby` (Continue → the
  existing quit-to-menu, which lands in the host/join lobby). There is no
  `Play again` there: the session lifecycle consumes a restart intent
  without beginning for `Host`/`Remote` authority, so the row would lie.
  The body reads the host's match or the replica through the new
  `cnrhud::match_view` (also used by the HUD readout).
- Docs: README, ledger CNR-12, PLAN.

Tests: `cnr` — decided hosted match opens Results; a `Remote` session
with a (stray) `CnrHost` is untouched; client: decided replica opens
Results, undecided keeps playing, authority ignores a replica; `results`
— rows per kind, kind per authority, a joined client's screen shows the
verdict and `Back to lobby` only; `network` — a decided hosted match
moves the host to Results and the peer still receives a decided `Cnr`
frame off the real socket.

Gates (iteration 23, foreground): `cargo fmt --all -- --check` pass;
`cargo clippy --locked --workspace --all-targets --all-features -- -D
warnings` exit 0; `cargo test --locked --workspace` exit 0, no failing
result line. No processes left running.

Not verified: two real processes (host + joined client) playing a match
to its end; any screenshot of the screens; the retained `Playing` input
of a client between the host's decision and the frame arriving (a few
frames of local driving). Open: commentary, menu-hosted lobby offer,
client carrier-mass prediction, two-process started match; F27-AC01..06.
Status: implemented candidate, not independently checked.

---

# Last iteration — F27-B.4c (rematch leg): match-over screen with Play again (new-run iteration 22)

Selection: the previous review passed with no blocking findings; its gaps
(no screenshot of the new menu screens, retail-city menu gating, fmt/clippy
markers absent from `verify.log`, no two-process match) are visual /
log / two-process matters I cannot repair here. Of the queued B.4c legs
(commentary — audio, menu lobby offer, rematch) the rematch is the one a
single process can honestly finish and test: a decided match previously
just froze its HUD with no way to start another short of `F4`.

Change:
- `cnr::end_decided_match` (fixed step, after `cnr_host_step`): a decided
  match on a `Local`-authority `Playing` session transitions it to
  `Results`. Host/remote sessions are untouched — the lobby's
  `Cancel`/`Start` owns their restarts and each match is a new wire
  generation, so the replica stage needs no epoch (the earlier note about
  a rematch inside one generation does not arise).
- `cnrhud::result_lines` (pure; verdict/reason/time helpers shared with
  `scoreboard_lines`, whose output is unchanged): verdict, reason + played
  time, team totals, ranked rows with leavers marked. `None` while
  undecided.
- `results.rs`: with a `CnrHost` the overlay shows "Cops & Robbers" + those
  lines (no rewards block — C&R records nothing to the profile) and the
  restart row reads `Play again`; the row is the existing restart intent.
- Docs: README, ledger CNR-12 (wording/layout are designed, original
  results presentation unrecovered), PLAN.

Tests: `cnrhud` +2 (body needs a result, ranks, leaver mark, other side's
verdict; FFA has no team line); `cnr` +4 (local decided → Results once;
undecided keeps playing; Host/Remote untouched; no match → nothing);
app-level `a_decided_cops_and_robbers_match_offers_play_again` (menu →
launch → clock run out → `Results` with the C&R body and `Play again` →
activate → new generation, undecided match, clock 0).

Evidence: `cargo fmt --all -- --check` PASS; `cargo clippy --locked
--workspace --all-targets --all-features -- -D warnings` clean (no
diagnostics); `cargo test --locked --workspace` exit 0 (2125 passed, 0
failed). `MM2_RETAIL=/Users/linus/coding/rust-mm2/retail cargo test
--locked -p mm2_app --test app cops_and_robbers` 3 passed incl. the retail
session test. No screenshot of the new screen (it reuses the results
overlay; not captured), no audio, no two-process run. No processes left.

Open: commentary, menu-hosted lobby offer, host-side rematch, client
carrier-mass prediction, two-process started match; F27-AC01..06 open.
Status: implemented candidate, not independently checked.

---

# Last iteration — F27-B.4c (menu leg): offer Cops & Robbers from the main menu (new-run iteration 20)

Selection: the previous review passed with no blocking findings; its gaps
(fmt/clippy absent from `verify.log`, retail-gated test not confirmed to
run, replica path unit-only, no two-process run) are log/two-process
matters I cannot repair here. The match is reachable by CLI and visible
(HUD), so the next smallest honest leg of B.4c is the in-game menu offer
that the HUD slice deliberately waited for.

Change:
- `menu.rs`: root row `Cops & Robbers` → `Screen::CnrCity` (every city,
  disabled with the `net::check_session` reason when its
  `multicopwaypoints.csv` pool is short — the same gate `--cnr --host` and
  joining clients apply; verdict cached in `MenuData`) →
  `Screen::CnrOptions { city, settings }` (Game / Gold weight / Limit
  cycle rows, Left/Right/Enter; `Start match` launches
  `SessionMode::CopsAndRobbers(settings)` single-seat through the same
  `launch` as Cruise). `menu_graphics.rs` titles/side panel updated.
- `mm2_game::cnr_options`: `label()` for variant/gold/limit and wrapping
  `CnrSettings::cycled_{variant,gold,limit}`. Gold/limit names are the
  `mmlang.dll` strings; variant and `No limit` wording are ours.
- Docs: README, `docs/research/menu.md` entry table, ledger CNR-12, PLAN.

Tests: `cnr_options` unit test (wrap, labels); app-level
`the_menu_offers_cops_and_robbers_where_the_city_can_seed_a_round`
(disabled with reason without a pool, enabled with 3 sites, defaults,
cycles incl. Left wrap, launch reaches `Playing` with the picked mode and
a `CnrHost`).

Evidence (recovery iteration 21): iteration 20 left this tree uncommitted
because `cargo test --workspace` failed on its own new unit test — root
cause: the test indexed `MatchLimit::choices()` wrongly (`labels[6]` is
`250 pts`; the list is `None`, 4 time limits, then 100/250/500/1000
points, so 500 is index 7). Test expectation fixed (also asserts index 5 =
`100 pts`); production code unchanged. Results after the fix:
`cargo fmt --all -- --check` pass; `cargo clippy --workspace --all-targets
--all-features -- -D warnings` pass; `cargo test --workspace` exit 0,
2118 passed / 0 failed. Note `cargo test` stops at the first failing
crate, so the earlier run had not exercised later crates.

Not verified / open: menu-hosted lobby offer (menu Multiplayer row is still
F24), commentary, rematch, client carrier-mass prediction, two-process
started match, any screenshot of the new screens (not captured).
F27-AC01..06 open. Status: implemented candidate, not independently checked.

---

# Last iteration — F27-B.4c (HUD leg): the Cops & Robbers match readout (new-run iteration 19)

Selection: the previous review passed with no blocking findings; its
gaps (fmt/clippy absent from `verify.log`, MM2_RETAIL tests skipped
there) are log matters I cannot repair. The match is reachable from the
command line but a player could not see the clock, their points or the
gold's state, so the HUD is the next smallest honest leg of B.4c. The
in-game menu offer stays queued behind it (a menu entry on a match with
no readout would have been a thin button).

Change:
- `mm2_app::cnrhud` (new): `scoreboard_lines(view, me, hz)` (pure) and
  `update_cnr_scoreboard` (reads `CnrHost` on the authority, else
  `CnrReplica`; the local participant is the `PlayerControl::Local` car
  through `participant_id`). Lines: clock/limit, team totals or rank +
  points, the gold's state (carried by you → which marker to take it to,
  by someone, loose, up for grabs) or the result once decided.
  `spawn_cnr_scoreboard` is called from `cnr::start_match`;
  `camera::retarget_hud` pins it (`HudNodes`); the `H` gate hides it.
- Layout, wording and dev-font text are designed (original instrument
  art unrecovered); the data shown matches the documented mode.
- Docs: README, ledger CNR-12, PLAN row.

Tests: 8 `cnrhud` unit tests (clock rounding, timed FFA countdown/rank,
point-limit target and lone driver, team totals + hideout/bank wording,
rival naming, frozen clock + result variants, ordinals, and the system
end to end: host vs replica precedence, `H` gate, no match → hidden);
the `MM2_RETAIL`-gated session test now also asserts one readout with
"COPS & ROBBERS" and "YOU: ROBBERS" in retail sf and none after
teardown.

Evidence: `cargo fmt --all -- --check` PASS; `cargo clippy --locked
--workspace --all-targets --all-features -- -D warnings` exit 0;
`cargo test --locked --workspace` exit 0 (2116 passed, 0 failed; was
2108). `MM2_RETAIL=/Users/linus/coding/rust-mm2/retail cargo test
--locked -p mm2_app --test app cops_and_robbers` ran (0.56 s) and passed.
Graphical: `mm2 --mm2-path <retail> --city sf --cnr cops --cnr-limit
250pts --frames 90 --screenshot` rendered; the panel sits top-right
("COPS & ROBBERS first to 250 / ROBBERS 0 COPS 0 / YOU: ROBBERS 0 pts /
THE GOLD IS UP FOR GRABS"), screenshot kept local (original content).
No audio, no two-process match; the client path is covered by the
replica unit test only. No test processes left.

Not verified / open: in-game menu entry, commentary, rematch, client
carrier-mass prediction, two-process started match; F27-AC01..06 open.
Status: implemented candidate, not independently checked.

---

# Last iteration — F27-B.4c (CLI leg): offer Cops & Robbers from the command line (new-run iteration 18)

Selection: the previous review passed with no blocking findings. Its
gaps (fmt/clippy absent from `verify.log`; client markers and re-seat
only synthetic) are verification-log / two-process matters I cannot
repair from here; the match now starts and seats, so the next ready leg
is making it *reachable*. B.4c (menu, HUD, commentary, rematch) is
broad; the command-line offer is its smallest honest leg (the in-game
menu stays queued).

Change:
- `mm2_game::cnr_options`: `CnrVariant::parse`, `GoldMass::parse`,
  `MatchLimit::parse` (only the host menu's stock values),
  `CnrSettings::parse(variant, gold, limit)` with errors naming what
  would parse. Spellings are ours (implementation choice).
- `mm2 --cnr ffa|cops|robbers [--cnr-gold ..] [--cnr-limit ..]`: sets
  `SessionMode::CopsAndRobbers`; conflicts with `--event`, `--dev-world`,
  `--join`; the option flags require `--cnr`. With `--host` the config
  is `validate`d and `net::check_session`-gated at flag time (a city
  that cannot seed a round exits 2 instead of advertising).
- `mm2-host` deliberately *not* extended: it runs no simulation, so it
  could advertise a match nobody runs.
- Docs: README, ledger CNR-12, PLAN row.

Tests: `mm2_game` `cnr_options` (3: every name parses and the tables are
fully covered, off-table/malformed rejected, settings defaults + error
text); `net_app::cnr_flag_gates_are_named_exits` (9 usage-error shapes
incl. the site-pool gate on a synthetic city, real `mm2` processes);
`net_app::mm2_host_cnr_advertises_the_mode` (`MM2_RETAIL`-gated: the
`listening=` record names "cops & robbers, "; lobby then killed).
Manual: `mm2 --mm2-path <retail> --city sf --cnr cops --cnr-gold half
--cnr-limit 250pts --headless --frames 120` → "cops & robbers match
built markers=3", smoke status=pass (production `load_session_world`).

Evidence: `cargo fmt --all -- --check` exit 0; `cargo clippy --locked
--workspace --all-targets --all-features -- -D warnings` exit 0 (one
`question_mark` lint fixed on the way); `cargo test --locked --workspace`
exit 0 (2108 passed, 0 failed; was 2103). The two new `net_app` tests
also run with `MM2_RETAIL=/Users/linus/coding/rust-mm2/retail` (the
retail leg ran, not skipped). No GPU, audio or two-process started
match. No test processes left.

Not verified / open: no in-game menu entry or HUD; no two-process
started match; client carrier-mass prediction; rematch; F27-AC01..06
open. Status: implemented candidate, not independently checked.

---

# Last iteration — F27-B.4b (client): markers follow the replica, leavers are re-seated (new-run iteration 17)

Selection: the previous review passed with no blocking findings. Two of
its verification gaps were real defects, so they came first: (1) a
participant who vanished and returned was never re-seated; (2) reading
`load_session_world` showed a *client* also ran `start_match` and kept an
inert `CnrHost`, so its markers (`sync_cnr_markers` read only the host)
would have stayed on the opening draw forever. Both are small and share
the "who owns the match" seam, so one change.

Change:
- `mm2_game::gold::GoldMatch::rejoin` — a left member returns on their
  side with their points (refused: match over, never joined, still
  connected); pushes `Joined` so the revision moves and replicas see it.
  Implementation choice (original reconnect handling unrecovered).
- `cnr::enroll_cnr_participants` re-seats a known-but-disconnected
  participant whose car is present, before seating fresh ones.
- `cnr::start_match` inserts `CnrHost` on the authority only; a client
  builds the same seeded draw just to place markers.
- `cnr::sync_cnr_markers` reads `CnrHost` if present, else `CnrReplica`.
- Docs: ledger CNR-12, `research/net.md`, PLAN row.

Tests (synthetic): `mm2_game` `a_leaver_who_returns_resumes_their_side_and_
points`, `nobody_returns_to_a_finished_match`; `mm2_app::cnr`
`a_participant_whose_car_comes_back_is_reseated_on_their_own_side`,
`a_client_draws_its_markers_from_the_replicated_match` (opening, carried →
hidden, delivery → new sites), `a_client_builds_the_same_draw_for_its_
markers_but_holds_no_match`.

Evidence: `cargo fmt --all -- --check` PASS; `cargo clippy --locked
--workspace --all-targets --all-features -- -D warnings` PASS; `cargo
test --locked --workspace` exit 0 (2103 passed, 0 failed; was 2098).
`MM2_RETAIL=/Users/linus/coding/rust-mm2/retail cargo test --locked -p
mm2_app --test app cops_and_robbers` ran (0.55 s, not skipped) and passed
— that is the only original-data evidence, host side, unchanged. No GPU,
audio or two-process run; the client marker path is synthetic only. No
test processes left.

Not verified / open: client car's carrier mass is not predicted; no HUD,
menu or CLI offers the mode; no two-process run of a started match;
F27-AC01..06 open. Status: implemented candidate, not independently
checked.

---

# Last iteration — F27-B.4b (host): start the match, seat the cars (new-run iteration 16)

Selection: the previous review passed with no blocking findings, so no
repair was owed. B.4b ("sides from the roster, spawn + `CnrHost`,
markers, replica") is still broad; its host half is the ready leg — the
`SessionMode` exists (B.4a) but nothing builds or seats a match from it.
Client markers and the menu/CLI offer stay queued (B.4b-client, B.4c).

Change:
- `cnr::start_match` (called from `load_session_world`): builds the
  `CnrHost` from the city's `CnrContent` pool and `CnrSettings::rules`
  at `RACE_TICK_HZ`, seeded from the session seed, with nobody seated;
  spawns the markers; an `Err` (short pool, non-city path) fails the
  session instead of cruising.
- `cnr::enroll_cnr_participants` (fixed schedule, before the step): seats
  each non-AI car on `GoldMatch::balanced_side()` (fewest connected
  members, ties to the first side; designed — team choice is
  unrecovered), in ascending id order; networked sessions need the car's
  `NetPlayer` stamp.
- `cnr::participant_id`: a car is the match's participant under its
  *wire id* when networked (minted `PlayerId`s differ per process, so the
  replicated match could not name a car otherwise), else its minted id.
  `cnr_host_step` and `reconcile_gold_load` use it.
- Docs: ledger CNR-12, `research/net.md`, PLAN row.

Tests: `mm2_game` `joiners_fill_the_sides_alternately`; `mm2_app::cnr`
(6 new, synthetic: pool→match with same-seed same-round, short/absent
pool and non-city path refuse, alternating seating by wire id with bots
and unstamped cars excluded, local-session seating, no seating off
`Playing`, rules read a car by wire id); `tests/session.rs`
`a_cops_and_robbers_session_builds_seats_and_tears_down_its_match`
(production `load_session_world` path, skips without `MM2_RETAIL`; run
against the retail install at `/Users/linus/coding/rust-mm2/retail`: sf
match built, local car seated as Robbers, hideout/bank/gold markers on
the drawn sites, host and markers gone after quit).

Evidence: `cargo fmt --all -- --check` exit 0; `cargo clippy --locked
--workspace --all-targets --all-features -- -D warnings` exit 0 (one
targeted `too_many_arguments` allow on `start_match`, commented); `cargo
test --locked --workspace` exit 0 (2098 passed, 0 failed; was 2090). The
retail session test also run alone with `MM2_RETAIL` set (ran 0.56 s, not
skipped). No GPU, audio or two-process run. No test processes left.

Not verified / open: no menu/CLI offers the mode and no HUD shows it;
clients do not draw markers from `CnrReplica` and their own car does not
predict the carrier's mass; a car respawned by a pick change is not
re-seated (a leaver is never re-seated); no two-process run of a started
match; F27-AC01..06 open. Status: implemented candidate, not
independently checked.

---

# Last iteration — F27-B.4a: Cops & Robbers as a session mode on the wire (new-run iteration 15)

Selection: the previous review passed with no blocking findings, so no
repair was owed. B.4 (lobby/start, HUD, rematch) is too broad for one
change; its first ready leg is making the match a *configurable,
advertisable session mode* — everything later (sides from the roster,
host wiring, client replica, HUD) needs a `SessionConfig` that names it.

Change:
- `mm2_game::cnr_options` (new): `CnrSettings`, `GoldMass`, `MatchLimit`,
  the option tables and rule constants moved verbatim out of
  `mm2_content::cnr` (which re-exports them) so `mm2_game` can carry the
  lobby's choices without depending on content. Plus
  `MAX_LIMIT_MINUTES`/`MAX_LIMIT_POINTS` bounds.
- `SessionMode::CopsAndRobbers(CnrSettings)`; `SessionConfig::validate`
  requires a city world and a bounded limit (`ConfigError::
  CopsAndRobbersNeedsCity`/`CopsAndRobbersLimit`). Wreck outcome =
  free-roam recovery (designed); no `ResultId` event.
- `mm2_app::net`: the mode rides the advertisement by name (not the
  inferred variant numbering); unnamed choices fail the decode;
  `check_session` refuses a city whose site pool is under 3
  (`SessionContentError::CopsAndRobbers`); `late_join_policy` (shared
  with `mm2-host`) closes the mode to late joins.
- Docs: `docs/research/net.md` section, ledger CNR-12, PLAN row.

Tests (all synthetic): mm2_game config validation + wreck outcome; net
unit — 3x3x9 option grid round trip, city/limit refusals (encode and
hand-written blob), unnamed choices rejected, join policy; `net_check` —
site-pool gate (0, 2, 3, 40 sites).

Evidence: `cargo fmt --all -- --check` PASS; `cargo clippy --locked
--workspace --all-targets --all-features -- -D warnings` PASS; `cargo
test --locked --workspace` PASS (2090 passed, 0 failed; was 2084). No original-data, GPU,
two-process run.

Not verified / open: no menu or CLI offers the mode (deliberately — it
would be a hollow button until B.4b starts a match); nothing creates a
`CnrHost` or renders `CnrReplica`; the fresh-generation-per-match
question for rematch stays B.4b/c; F27-AC01..06 open. Status:
implemented candidate, not independently checked.

---

# Last iteration — F27-B.3: the Cops & Robbers match on the wire (new-run iteration 14)

Selection: the previous review passed with no blocking findings, so no
repair was owed. Of F27-B's queued legs, B.3 (replicate the host's match
to clients) is the next ready one: B.4 (lobby, HUD, rematch) needs a
client-visible match to build on, and F27-AC05 (teams/scores/winner agree
across clients) starts here. The host measures positions itself, so no
client pickup request is needed — the wire is one-way, host → client.

Change:
- `mm2_game::gold`: `GoldMatch::view()` → `GoldView` (everything a
  replica shows) with `freshness()` = `(revision, elapsed)`.
- `mm2_net` protocol v21: `Message::Cnr { generation, frame: SnapCnr }`
  (tag 0x13, bounded to 32 seats; opaque discriminants; strict decode).
  A client sending it is dropped `Malformed` like every host→client
  frame.
- `mm2_app::cnrnet` (new): the discriminant tables, `encode_view` /
  `decode_view` (refuses unnamed discriminants, non-finite positions,
  duplicate/reserved/foreign-side seats, a carrier/dropper/winner that
  is not a participant), `CnrStage` (in `RemoteSnaps`; freshest per
  generation, stale/refused counted, malformed frames cannot become a
  watermark), `publish_cnr` (host; on revision change, else every 120
  match ticks), `apply_cnr` (client → `CnrReplica`, removed with the
  session). Registered in `main.rs`/`smoke.rs` next to the world clock;
  `NetDriveReport` gains `cnr_sent/landed/stale/refused`.
- Docs: `docs/research/net.md` (new section), ledger CNR-12, PLAN row.

Tests (17 new, all synthetic): `mm2_game` view; `mm2_net` codec round
trip (every state), float bits kept, truncation/padding, oversize both
ways, bad bool, and a client cannot assert the match (real sockets);
`cnrnet` unit (every ownership state × variant and every outcome shape
survive the wire, 18 self-contradicting frames refused with the right
reason, stage ordering/generation/bound/reset); `net_app` (host
publishes on change and at the cadence, decoded off a loopback socket
== the host's own view; client lands, drops stale/foreign/malformed,
replica dies with the session). A mutation (`<=`→`<` on the freshness
check) fails three of them.

Gates: `cargo fmt --all -- --check` PASS; `cargo clippy --locked
--workspace --all-targets --all-features -- -D warnings` PASS; `cargo
test --locked --workspace` PASS (2084 passed, 0 failed; was 2067). No
original-data, GPU, two-process or impairment run. No test processes
left running.

Not verified / open: nothing starts a match in the shipped app (B.4) and
nothing renders `CnrReplica` (client HUD/markers — `sync_cnr_markers`
still reads only `CnrHost`); there is no join-time unicast (a late
joiner learns the match from the ≤1 s periodic repeat); a rematch inside
one generation restarts `revision` and would be dropped stale (B.4:
new generation per match or a match epoch); the `net=` record line does
not print the cnr counters yet; F27-AC01..06 all open. Status:
implemented candidate, not independently checked.

---

# Last iteration — F27-B.2b: Cops & Robbers host from content, retail markers (new-run iteration 13)

Selection: the previous review passed (no blocking findings), so no
repair was owed. Of F27-B's queued legs B.2b needs neither protocol nor
lobby design, and it connects the match to real content: the city's
authored site pool and the retail marker models.

Change (`crates/mm2_app/src/cnr.rs`, one `Update` system line in
`main.rs`):
- `CnrHost::from_content(content, settings, tick_hz, generation, gold,
  seed, participants)` — match over `CnrContent::sites` (via `v3`) with
  `CnrSettings::rules`; a pool under 3 is refused (`PoolTooSmall`).
- `spawn_cnr_markers` — `wpobj_gold`/`pt_hideout`/`pt_bank` through
  `MovableModels` (the shared PKG→mesh path), session-owned roots with
  render-part children, no collider; a model that fails to load is
  counted in `CnrMarkerReport`, never substituted.
- `sync_cnr_markers` — hideout/bank on the drawn sites, gold marker on
  the resting/dropped gold, hidden while carried (implementation
  choice, ledger CNR-12 updated). Idle without a `CnrHost`.
- 5 synthetic tests (pool→sites, short pool refused, model-name binding
  with a missing model counted, gold marker follows/hides/returns on a
  drop, hideout/bank positions and no-host idle).

Evidence: gates `cargo fmt --all -- --check` PASS; `cargo clippy --locked
--workspace --all-targets --all-features -- -D warnings` PASS; `cargo
test --locked --workspace` PASS (2067 passed, 0 failed; was 2062). One
local original-data run through a throwaway test (deleted, not
committed): against `/Users/linus/coding/rust-mm2/retail` both cities
build a host from the real pool (sf 44 sites, london 46, 0 content
issues) and all three marker PKGs load and spawn (3/3, no missing
models or textures). No GPU/screenshot: marker scale, ground contact
(model origin vs road) and appearance are unverified.

Still open: nothing calls `from_content`/`spawn_cnr_markers` in the
shipped app (B.4 lobby), no wire (B.3), no HUD/commentary; F27-AC01..06
open. Status: implemented candidate, not independently checked.

---

# Last iteration — repair: a wreck must not farm the gold (new-run iteration 12)

Root cause (review of iteration 11, implementation defect): `cnr_host_step`
turned every connected car in reach into a pickup `Contact`, including a
`Disabled` one. After the drop lockout (120 ticks) the wreck sitting on its
own dropped gold was granted it again (+25 recovery points, Picked event),
then the `wrecked` check dropped it again — a loop of ~1 grant/second with
no opponent. The old test only looked one step after the drop, inside the
lockout.

Fix (`crates/mm2_app/src/cnr.rs`): wrecked players are excluded from the
pickup contacts and from the delivery attempt. Regression test
`a_wreck_never_retakes_the_gold_after_the_lockout_ends` keeps a Disabled car
on the gold for 3x the lockout and asserts no carrier, unchanged score, no
events, base mass. Raised/unchanged review gaps stay open: everything is
synthetic, no real Avian vehicle, teardown ordering and the non-authority
idle path untested, 8 m/s / 25-point recovery are placeholders (ledger
CNR-12), and "rammers trading the gold for 25 points each" is a design
placeholder still to revisit with the cop/robber scoring (F27-A open).

Gates: `cargo fmt --all -- --check` PASS; `cargo clippy --workspace --all-targets -- -D warnings` PASS; `cargo test --workspace` PASS (2062 passed, 0 failed; was 2061). The new test was confirmed to FAIL with the guard disabled and pass with it. No test processes left running.
Status: implemented candidate, not independently checked.

---

# Last iteration — gold rules over the host's cars (new-run iteration 11)

Selection: the previous review passed with no blocking findings, so no
repair was owed. Of F27-B's queued legs, B.2 (Bevy consumers of the new
rule core) is the only one that needs neither a protocol nor a lobby
design, and it is where F27-AC04 (load applied once, no leak into a
later race) becomes a property of real bodies instead of a pure function.

Change: `mm2_app::cnr`. `CnrHost` (resource: the `GoldMatch`, the dislodge
threshold, last-seen positions). `cnr_host_step` (fixed, authority only,
Playing only): ticks the clock; a participant whose car was seen and is
now absent leaves (gold drops at the last position; never-seen or
non-finite-pose participants are not leavers); a `Disabled` carrier, or a
car-on-car `ImpactEvent` of the carrier at ≥ `DEFAULT_DISLODGE_SEVERITY`
(8 m/s, enhanced policy — wall/prop hits never count), drops the gold;
every connected car within the pickup radius becomes a `Contact` from the
*host's* `Position` and `resolve_pickups` decides; the carrier tries
`deliver`; gold below `WorldFloor` goes through `gold_out_of_bounds`;
`GoldEvent`s publish as `CnrEvent` messages. `reconcile_gold_load`
writes the carrier's `Mass` and principal `AngularInertia` from a
recorded `GoldLoadApplied` base (never additive, so repeats cannot
stack) and restores it when `load_for` is none or the `CnrHost`
disappears; `drive_session` teardown removes the `CnrHost`. Both systems
are in the fixed chain next to `recovery`. The handling scalar is
recorded but not applied: its effect is unidentified (UNK-10).

Tests: 15 `mm2_app::cnr` units over a plain App with real
`Session`/`Position`/`Mass`/`AngularInertia` (pickup + load, load once
over 50 steps, load ends exactly with the carrying incl. inertia restore,
soft/world/self impacts ignored, nearest of two wins with one award,
rammed carrier → striker recovers while the dropper is locked out,
`Disabled` carrier drops and cannot retake at once, vanished carrier
leaves and gold drops in place with points kept, unspawned participant is
not a leaver, delivery scores once and sheds the load, below-floor gold
re-placed, pause freezes the clock, time limit ends and publishes once,
removing the host strips the load, NaN pose neither picks up nor ejects).
All synthetic. Ledger CNR-12, PLAN F27-B row updated.

Gates (all exit 0): `cargo fmt --all -- --check` PASS; `cargo clippy
--locked --workspace --all-targets --all-features -- -D warnings` PASS;
`cargo test --locked --workspace` PASS (2061 passed, 0 failed; was 2046).
No original-data, GPU or network run. No test processes left running.

Not verified / open: nothing creates a `CnrHost` in the shipped app (lobby
= B.4), no site markers/meshes from `CnrContent` (B.2b), no wire/client
replica (B.3), no HUD/audio. Whether the mass actually changes handling
feel on a real vehicle in Avian is untested (no physics world in these
tests); the 8 m/s threshold is a guess. F27-AC01..06 all open. Status:
implemented candidate, not independently checked.

---

# Last iteration — authoritative gold state machine (new-run iteration 10)

Selection: the previous review passed with no blocking findings, so no
repair was owed. F27-A's data half is done and its remaining items are
research I cannot resolve without running the original; F27-B (authoritative
gold/teams/scoring/handling) was `queued` behind it and the spec's
requirements 3 and 4 (one valid ownership state, deterministic contested
pickups, load applied/removed exactly once) are pure rules that need no
network or GPU. I split F27-B and took B.1, the rule core, over starting
with wire messages: a protocol written before the rules would encode guesses.

Change (no protocol change, no gameplay wired): `mm2_game::gold::GoldMatch`.
One ownership state `Resting` / `Carried` / `Dropped`; `resolve_pickups`
decides a tick's `Contact`s (player + round from the client, *position from
the host's sim*) — nearest wins, equal distance to the lower `PlayerId`, so
arrival order never matters; losers are `Contested`, repeats `Duplicate`,
old rounds `Stale`, a dropper `Locked` for a short lockout; `deliver` scores
the 100 points once, clears the carrier, draws new seeded distinct sites and
advances the round; `dislodge` (rammed/destroyed), `leave` (disconnect:
gold drops in place, points stay), `join`, `gold_out_of_bounds` (re-placed
at a fresh site, no score), `tick` (time limit); point limit compares the
individual in FFA and the team total otherwise; ties are `Winner::Tie`.
`load_for(player)` *derives* the carrier's mass/handling from the state so
the load cannot be applied twice or outlive the carrying, and a fresh match
has none. `mm2_content::cnr::CnrSettings::rules(tick_hz)` builds `GoldRules`
from the recovered tables (new constants `PICKUP_POINTS` = 25,
`PICKUP_RADIUS_M`, `DROP_LOCKOUT_SECONDS`); `CnrVariant` moved into
`mm2_game::gold` (re-exported from `cnr`, no caller changed).

Classification (docs/research/cnr.md table, ledger CNR-11): delivery 100 /
radius 12 / mass options / limits are original constants; the 25-point
pickup is an original constant applied to every grant (trigger unknown, so
an implementation choice); pickup radius, dropper lockout, disconnect drop,
out-of-bounds re-placement, red→hideout/blue→bank and tie handling are
enhanced policy or implementation choices and say so at their definitions.

Tests: 28 `mm2_game::gold` units (construction validation; three distinct
sites for any seed; nearest-wins; every arrival order of equidistant
requests yields one carrier and one award; duplicates; refusals with
reasons and the inclusive radius; no re-pickup of carried gold; dislodge →
recover with the load moving; lockout expiry; stale/repeated impact reports;
non-finite drop position; delivery scoring once, out-of-range, wrong
marker, stale round, no second score; pickup and delivery in the same tick;
cops→bank/robbers→hideout; red/blue; point limit freezes the match; team
total ends it, not an individual; time limit and ties; leaver keeps points;
late join; out-of-bounds re-placement; load follows only the carrier and a
new match has none; deterministic replay and seed sensitivity; revision and
event drain; balanced sides) and 4 `mm2_content::cnr` units (defaults, every
variant×mass×limit choice maps to the enforced rule, lockout follows the
tick rate, stock settings drive a real match to its end). All synthetic.

Gates (all exit 0): `cargo fmt --all -- --check` PASS; `cargo clippy
--locked --workspace --all-targets --all-features -- -D warnings` PASS (no
new allow); `cargo test --locked --workspace` PASS (2046 passed, 0 failed;
was 2014). No original-data run this iteration (no content path changed).
No test processes left running.

Not verified / open: this is rule-core evidence only — no wire, no
client replica, no HUD/lobby, no vehicle component applying the load, no
impact-threshold that calls `dislodge`, no rematch; F27-AC01..06 all open.
The original's own answers for what knocks gold loose, who may recover and
what recovery scores, disconnect/out-of-bounds outcomes and the handling
scalar's physical effect remain unknown (UNK-10); the policies above are
placeholders that are labelled, not evidence. Status: implemented
candidate, not independently checked.

---

# Last iteration — Cops & Robbers rule matrix, data half (new-run iteration 9)

Selection: the previous review passed with no blocking findings, so no
repair was owed. Eight iterations in a row had gone to F26-A, whose
remaining items are measurement/impairment legs I cannot run here and
polish on already-bounded paths. F27-A (verify the full C&R rule/settings
matrix and placement dependencies) was `queued` with the whole mode
unevidenced — the ledger said "no C&R-specific data discovered" — and is
a research slice that needs no network or GPU, so it is the highest-value
ready work.

What was found (all in `docs/research/cnr.md`, each fact classed
verified / inferred / unknown): the mode's data ships — `race/<city>/
multicopwaypoints.csv` is the gold/hideout/bank site pool (44 sf rows, 46
london), plus five marker models (`wpobj_gold`, `pt_hideout`, `pt_bank`,
`pt_red`, `pt_blue`) with banger records, five map dots, the per-city
`cnr<city>.csv` commentary tables (14 cue families: get/drop/stash/recover
per role, has/stashed/dropped per team) and a loading image. The rest is
code constants read from `Midtown2.exe` (disassembly) and `mmlang.dll`
(string table): the packed host-settings word and its tables (gold mass
0/100/200 engine units, time 5/10/20/30 min, points 100/250/500/1,000);
delivery = 100 points within a 12.0-radius marker with the carrier's mass
removed; pickup arbitration by the host only when no carrier exists; the
local carrier's handling scalar 1.0/0.9/0.81. The executable also tries an
optional `multicopsets.csv` first; no retail install ships one.

Code (no protocol change, no gameplay change): `mm2_content::cnr` —
`CnrContent::load` resolves the 18 per-city dependencies and the 14 cue
families through the VFS and records every miss as an issue (never loses a
denominator entry); typed option tables (`TIME_LIMIT_MINUTES`,
`POINT_LIMITS`, `GoldMass` with engine units kept separate from the
documented kilogram reading, `MatchLimit`, `CnrVariant`, `DELIVERY_POINTS`,
`DELIVERY_RADIUS_M`). `mm2-inspect cnr [--city] [--strict]` audits it
(strict also fails on an empty city list). Ledger: CNR-5 narrowed, CNR-6…
CNR-10 added, MP-9 and UNK-10 updated; the inventory's "no C&R data" note
now points at the audit.

Tests: 14 `mm2_content::cnr` units (complete city, every dependency counted
when absent, missing pool, under-3 pool, non-finite site, header-less pool,
empty install, cue family missing/empty, empty cue table, option tables,
kg ratio, limit choices, variant scoring, paths) and 4 `mm2_inspect::cnr`
units. All synthetic.

Original-data evidence (separate): `cargo run -p mm2_inspect -- cnr
/Users/linus/coding/rust-mm2/retail --strict` — london and sf each 18/18
dependencies, 14/14 cue families, pools of 46 and 44, exit 0. Code facts
were read by disassembling the retail executable (`objdump`); they are
reading evidence, not runtime-observed: no original game was run.

Gates (all exit 0): `cargo fmt --all -- --check` PASS; `cargo clippy
--locked --workspace --all-targets --all-features -- -D warnings` PASS (no
new allow); `cargo test --locked --workspace` PASS (2014 passed, 0 failed;
was 1996). No test processes left running.

Not verified / open: the *inferred* items above (variant 1 numbering, the
radius test direction, the mass unit); what knocks gold loose, pickup
radius, cop/robber scoring beyond delivery, respawn timing, disconnect and
out-of-bounds outcomes; the `multicopsets.csv` path is read from code and
unimplemented. No state machine, HUD or lobby flow exists — F27-B/C stay
queued behind F25-B, and F27-AC01..06 are all open. Status: implemented
candidate, not independently checked.

---

# Last iteration — bound the host's power over a client's re-seeks (new-run iteration 8)

Selection: the previous review passed with no blocking findings, so no
repair was owed. Of its verification gaps the one that is a real
defect-in-waiting rather than missing evidence: "a hostile or buggy host
can force replays of up to 2^21 steps per actor per frame by sending
increasing ticks under the cap; no per-second seek rate limit". A seek
replays every actor from its start, so the cost of one grows with the
target. I took this over per-client traffic relevancy (moot at the
default cap) and over the report-6 DSN-11 reconciliation (a larger
multi-process lifecycle change).

Change (no protocol change, still v20): `worldclock::WorldLimits`, held
by `WorldStage`. A generation's first frame is free (a late joiner must
land wherever the host stands; still bounded by `MAX_SEEK_TICKS`). Each
later frame is judged against the last one *taken*: dropped counted
`throttled` if within 500 ms of it, refused (`ref`) if its tick is more
than `480 + 480 × elapsed s` ahead (4× the 120 Hz fixed rate + 4 s
slack, for a stalled host or a burst after a link blackout). Neither
moves the watermark, so the next honest frame is judged against the same
anchor. `push` stamps wall time; `push_at` takes it explicitly so tests
drive the clock. `RemoteSnaps::set_world_limits` lets a harness relax
the bound; the record's `wclk=` gains `thr<n>`. The numbers are margins
(Implementation choice), not measurements of a real host. Docs:
`docs/research/net.md`, PLAN F26-A slice 8.

Tests: 5 new `WorldStage` units (throttle leaves the watermark on the
taken frame; runaway clock refused with no watermark and the inclusive
edge; a regressing clock stays stale and the seek cap holds unbounded;
a frame-per-millisecond hostile host with a 10× clock gets at most one
frame per interval and shuts out once implausible; an honest 1 Hz host
including a 3 s blackout burst is never limited — only the two frames
bunched behind the first are throttled); the `net_app` world-clock leg
now relaxes the limits for its back-to-back legs and ends with a
throttled frame over a real loopback socket (interval no run can
outlast) and a growth-refused frame followed by an honest one. All use
conditions under bounded spins or injected time, no wall-clock timing
decides an outcome.

Process evidence (separate, retail install `/Users/linus/coding/rust-mm2/
retail`): `MM2_RETAIL=… cargo test --locked -p mm2_app --test network
two_retail_processes_replicate_the_hosts_traffic` passes; host
`wclk=sent14`, client `wclk=sent0,landed5,seek4,ref0,thr9`. The nine
throttles are the **headless fixture**: its host clock runs faster than
wall time (1,680 ticks in a ~8 s test), so frames arrive faster than one
per 500 ms. Nothing was refused (the 4× margin held). It is not an
honest vsync-bound host; that run is unobserved. Not rendered, not
impaired.

Gates (all exit 0): `cargo fmt --all -- --check` PASS; `cargo clippy
--locked --workspace --all-targets --all-features -- -D warnings` PASS;
`cargo test --locked --workspace` PASS (1996 passed, 0 failed; was
1991). No test processes left running.

Still open (F26-A stays active, not AC01..06 completion): no RTT
compensation (client trails by the one-way delay; past ~50 ms it
re-seeks each accepted frame, now at most two a second), proximity
leaves per-peer, no windowed/GPU or impaired-network scenery
measurement, the first frame of a generation can still ask for up to
2^21 steps once, per-client traffic relevancy (moot at the default
cap), signal heads, interpolation beyond the velocity carry, mid-session
weather, late-join of props beyond the resend cycle, sound
replication, report-6 follow-ups 1 (two-process leg) and 2 (DSN-11).
Status: implemented candidate, not independently checked.

---

# Last iteration — world-clock replication for Cruise (new-run iteration 7)

Selection: the previous review passed with no blocking findings, so no
repair was owed. I first considered the review's "per-client relevancy"
gap for traffic and dropped it: the host's population is capped at
`SpawnPolicy::max_active` = 32, already under the 64-row wire bound, so
the id-ordered truncation can never drop a car today (a mod raising the
cap would need it; recorded, not built). The next F26-A gap that is real
today is the one the iteration-5 clock audit named and the iteration-6
race-row re-seek only half closed: **a Cruise has no race row**, so a
Cruise client — and above all a late joiner (AC02) — had nothing to align
its timed scenery (drawbridge leaves, boats, ferries, trains) to.

Change (protocol **v20**): `Message::World { generation, ticks }`
(tag 0x12, 17 B, host→client only; a client-sent one drops the peer
`Malformed`, AC04). `mm2_app::worldclock`: `publish_world_clock` (host;
`Countdown`/`Playing`/`Results`; at once, then every
`PUBLISH_EVERY_TICKS` = 120 world ticks, immediately if the clock went
backwards — so a pause stops the frames), `WorldStage` (inside
`RemoteSnaps`; newest tick per generation, ≤4 generations, older/equal
dropped `stale`, foreign generation refused at apply and never able to
stale-mark the session's own, a tick past `MAX_SEEK_TICKS` = 2^21 refused
*before* it can become a watermark because a seek replays every actor from
its start), `apply_world_clock` (client; held through `Loading`/`Paused`)
→ the existing `WorldClock::sync` (6-tick tolerance). Wired in main.rs,
smoke.rs and the `net_app` harness. Record gains ` wclk=sent,landed,seek,
ref` (absent when the wire carried none; `world=` was already taken by the
city name). Docs: `docs/research/net.md` (new section + budget row), PLAN
F26-A slice 7.

Tests: proto round-trip/truncation/padding; lobby
`a_client_cannot_assert_the_world_clock`; 5 `WorldStage` units (per-
generation newest, foreign generation, implausible tick not a watermark,
bounded generations, reset); `net_app::the_host_publishes_its_world_clock_
at_the_cadence` (real loopback socket: first frame at once, none a tick
short of the cadence — the next frame on the wire is the one at the
cadence — and a restart announced at once) and
`net_app::a_world_clock_frame_re_seeks_a_cruise_clients_scenery` (late
joiner lands on the host's tick, reordered older frame dropped, jitter
inside tolerance lands without a seek, foreign generation refused, absurd
tick refused without poisoning the honest frame after it). All spin on a
condition under a bounded deadline (report 6 rule).

Original-data / process evidence (separate from the synthetic tests):
`MM2_RETAIL=/Users/linus/coding/rust-mm2/retail cargo test --locked -p
mm2_app --test network two_retail_processes_replicate_the_hosts_traffic`
(now also asserts the clock): two real `mm2` processes on loopback,
retail sf Cruise, one Apple Silicon machine: host `wclk=sent15`, client
`wclk=sent0,landed15,seek13,ref0`, both `status=pass`. The 13 seeks are
**not** evidence of alignment quality: the headless harness free-runs
both processes at unrelated update rates, so the clocks diverge between
frames. How rarely a vsync-bound client seeks is unobserved. Not
rendered, not impaired.

Gates (all exit 0): `cargo fmt --all -- --check` PASS; `cargo clippy
--locked --workspace --all-targets --all-features -- -D warnings` PASS (no
new allow); `cargo test --locked --workspace` PASS (1991 passed, 0 failed;
was 1982). The retail two-process leg was re-run on the final code and
passed. No test processes left running.

Still open (F26-A stays active, not AC01..06 completion): no RTT
compensation (a client trails the host by the one-way delay; past ~50 ms
it re-seeks every frame), proximity leaves are still per-peer, no
windowed/GPU or impaired-network scenery measurement, per-client traffic
relevancy (moot at the default cap), signal heads, interpolation beyond
the velocity carry, mid-session weather, late-join of props beyond the
resend cycle, sound replication. Status: implemented candidate, not
independently checked.

---

# Last iteration — ambient-traffic replication (new-run iteration 6)

Selection: the previous review passed with no blocking findings, so no
repair was owed. F26-A's remaining largest gap against AC01 ("two clients
see the same relevant traffic") was that a networked session fielded *no*
ambient traffic at all (the MP-4 gate in `load_ambient_traffic`), so there
was nothing to share. Weather/time-of-day already agree (they ride
`Start`'s session config, which a late joiner also gets); props landed in
iterations 2–5.

Policy decision (recorded, nobody to ask — DSN-71): MP-4 documents "no
ambient traffic, cops or AI opponents in MP *races*" and names no Cruise
exception, while the F26 spec wants Cruise clients to share traffic. So
`traffic::fields_ambient_traffic` = offline always, networked only in
free-roam Cruise (enhanced policy, original unverified); networked races
keep none on both sides. The `Host` simulates; a `Remote` client holds
copies.

Change (protocol **v19**): `Message::Traffic { generation, tick, roster,
rows }` (host→client only; a client-sent one drops the peer, AC04) with
`SnapCar { id, class, state, pos, rot, vel }`, ≤64 rows (`MAX_SNAP_CARS`).
`mm2_app::worldtraffic`: `publish_traffic` (host-minted per-spawn ids via
`TrafficLedger::collect`, every third frame, roster digest on each frame),
`TrafficStage` (per-car latest-wins on `(generation, tick)`, staged ≤256,
applied ledger ≤16,384, held through `Loading`), `apply_traffic` +
`TrafficReplica` (client roster/class cache; kinematic `TrafficCopy`
bodies with the class model, collider and ambient engine table; roster
mismatch refused counted; velocity carry; snap past 3 m; TTL retirement
after 240 ticks; ≤128 copies). Record gains ` cars=sent,omit,live,landed,
mism` (absent while the wire carried no car). Docs: `docs/research/net.md`
(budget 26 B + 47 B/row), ledger DSN-71 + MP-4 note, PLAN F26-A slice 6.

Tests: proto round-trip/bound/truncation (mm2_net); lobby
`a_client_cannot_assert_traffic`; 7 `TrafficStage`/digest unit tests;
`traffic::networked_cruise_traffic_is_the_hosts_and_a_clients_is_a_replica`
(replaces the old "networked spawns none" test: Host cruise fields it,
Remote cruise holds only the replica, networked races none, Local race and
roam unchanged); `traffic::a_client_copies_the_hosts_traffic_and_retires_it_when_frames_stop`
(production row collector → frame codec → `apply_traffic`: one copy per
car, same class/pose, follows the host's motion as the *same entities*,
foreign roster refused, copies survive a short silence and retire after
the TTL). That leg caught a real bug — the retention pass forgot copies
spawned in the same run (queued in `Commands`), so every frame
duplicated the whole population; fixed with a `known` set.

Original-data / process evidence (separate from the synthetic tests):
operator-run `MM2_RETAIL=/Users/linus/coding/rust-mm2/retail cargo test
--locked -p mm2_app --test network two_retail_processes_replicate` — two
real `mm2` processes on loopback, one Apple Silicon machine, one binary,
retail sf Cruise: host `cars=sent4539,omit0`; client
`cars=sent0,omit0,live16,landed4316,mism0`, both `status=pass`. Not
rendered/audible evidence (headless) and not impaired.

Gates (all exit 0): `cargo fmt --all -- --check` PASS; `cargo clippy
--locked --workspace --all-targets --all-features -- -D warnings` PASS (no
new allow); `cargo test --locked --workspace` PASS (1982 passed, 0 failed;
was 1970). The retail two-process leg was re-run on the final code and
passed. No test processes left running.

Still open (F26-A stays active, not AC01..06 completion): per-client
relevancy (broadcast; bounded by the host's interest union), signal heads
not replicated, no measured impairment cell / windowed or real-GPU leg for
`Traffic` or `Props`, interpolation beyond the velocity carry, mid-session
weather (static per session), late-join of props beyond the resend cycle,
drawbridge/mover/sound replication, a client's contact with a copy is
predicted against the copy's last pose. Status: implemented candidate, not
independently checked.

---

# Last iteration — retail two-process site-table evidence + staging repair (new-run iteration 5)

Selection: the previous review passed with no blocking findings. Its
largest verification gap — no two-process retail run showing host and
client report equal `SiteTable`s — is the next evidence step for F26-A,
and its two staging-looseness notes are cheap, in-file repairs.

Repairs (`worldprops::PropStage`): the host's table is now held *per
generation* (≤4, oldest evicted) and replaced only by a frame of that
generation at least as new in tick — so a reordered older frame cannot
carry a stale, partly stamped table in, and a newer-generation frame no
longer decides the older one's rows (the review's "falls through to
agreed" note: a drained row's generation always has its own table, and a
row whose table is gone is refused counted as `mismatched`). A first
attempt that kept one global table broke the existing
`a_client_folds_prop_rows_into_its_stamped_world` leg (an interleaved
foreign-generation frame poisoned current rows) — caught by the full
suite, redesigned. Tests: `a_reordered_older_frame_cannot_replace_the_hosts_table`,
`each_generation_keeps_its_own_table`,
`rows_whose_generation_table_was_evicted_are_refused`.

Evidence plumbing: `NetDriveReport` gains `prop_world` (own table),
`props_landed`, `props_mismatched`, written by `publish_props`
(host) / `apply_props` (client); the headless record gains
` props=sites<count>:<digest>,landed<n>,mism<n>` (absent while nothing is
stamped, so other records stay identical). New operator-run test
`network::net_drive::two_retail_processes_stamp_the_same_prop_world`
(skips without `MM2_RETAIL=<install>`): `mm2 --host --city sf` + `mm2
--join`, both headless, loopback.

Result (retail install at `/Users/linus/coding/rust-mm2/retail`, one
Apple Silicon machine, one binary): host and client both printed
`props=sites5953:ce16a67de227adeb`; in a longer manual 4000-frame run the
client landed 3657 rows with `mism0`; the test run passed (7.5 s).
Evidence level: real processes, real loopback, retail world — site
ordinals agree *on the same platform*. Cross-platform agreement (the
quantised-home rounding-boundary concern) is still unobserved; name-only
digest is the recorded fallback if it ever diverges.

Gates: `cargo fmt --all -- --check` exit 0; `cargo clippy --locked
--workspace --all-targets --all-features -- -D warnings` exit 0 (one more
targeted `too_many_arguments` allow, on `apply_props`); `cargo test --locked
--workspace` exit 0 (1970 passed, 0 failed); the retail test re-run on the
final code passed. No test processes left running.

Still open (unchanged): measured impairment cell or real-GPU leg for
`Props`; interpolation/velocities; traffic/weather/time-of-day;
late-join beyond the resend cycle; drawbridge/mover/sound replication.
Not F26-AC01..06 completion. Status: implemented candidate, not
independently checked.

---

# Last iteration — world-agreement check for replicated props (new-run iteration 4)

Selection: the previous review passed with no blocking findings, so no
repair was owed. Its first verification gap — "site-ordinal agreement
between host and client … any stamp-time difference between peers (model
-load failure, content mismatch) would silently misattribute rows; there
is no checksum or site-count handshake" — is the highest-value ready
small slice of F26-A, and a silent wrong-prop pose is worse than no
pose.

Change (protocol **v18**): `Message::Props` carries `SiteTable { count,
digest }`. `mm2_app::worldprops::SiteRegistry` records each stamped
placement (ordinal, authored name, home position quantised to 0.25 m,
taken from `Added<BangerSite>` before any impact can move it, scoped to
the session generation) and hashes them in ordinal order (FNV-1a). The
host puts its table on every frame (`PropLedger.sites`); a client keeps
its own in `PropStage` and, on drain, compares: a disagreement drops the
whole drain counted as `mismatched` (distinct from `unresolved`), applies
nothing, logs one warning and sets `PropStage::divergence()`; the first
frame whose table agrees resumes replication. All-or-nothing by design —
with a shifted ordinal no row can be trusted. Rotation is excluded from
the digest (trig-derived, platform-sensitive); the 0.25 m quantum is a
design choice (Implementation choice) so cross-platform last-bit float
noise hashes alike. Docs: `docs/research/net.md` (budget header 18→30 B),
ledger DSN-70, PLAN F26-A slice 4.

Tests: proto round-trip/bounds/truncation carry the table;
`SiteRegistry` unit tests (order- and noise-insensitive; shifted,
renamed, moved, missing and gapped worlds all differ; generation reset);
`PropStage` mismatch drain; `net_app::a_client_refuses_rows_from_a_host_
with_a_different_world` (client with two of the host's three placements
poses nothing, then recovers under an agreeing table); the convergence
leg now asserts `mismatched()==0`, no divergence and a 4-placement
table; the publish-window leg asserts every frame's table equals the
22-placement world's. Existing legs adjusted so the host stamps props at
their homes and moves them afterwards (identity is the home pose).

Gates (all exit 0): `cargo fmt --all -- --check`; `cargo clippy --locked
--workspace --all-targets --all-features -- -D warnings` (one targeted
`too_many_arguments` allow on the `publish_props` Bevy system);
`cargo test --locked --workspace` (1966 passed, 0 failed). Retail sf
`--headless --frames 300` smoke (`/Users/linus/coding/rust-mm2/retail`):
`status=pass`, `bng=6110d`. No test processes left running.

Not verified / open: the check is exercised on synthetic stamps and the
in-process loopback harness only — a two-process retail run where host
and client report equal `SiteTable`s is still the next evidence step
(retail agreement not observed, so the ordinals-agree claim on real
worlds stays unverified); no measured impairment cell or real-GPU leg
for `Props`; no interpolation/velocities; traffic/weather/time-of-day,
late-join beyond the resend cycle, drawbridge/mover/sound replication
remain open. Not F26-AC01..06 completion. Status: implemented
candidate, not independently checked.

---

# Last iteration — world-prop replication repair (new-run iteration 3)

**Recovery of review rejection for iteration 2 (F26-A).** Root cause
(implementation): `PropStage::drain_for` recorded every drained
current-generation row in the unbounded `applied` watermark map, resolved
or not, so a host streaming ever-new `(site, fragment)` keys grew client
memory for the whole session while docs claimed a bound. Fix: `applied`
is now written only by `PropStage::remember`, called from `apply_props`
for rows that resolved against the local world, and is hard-capped at
`MAX_APPLIED_PROPS` (16,384; at the cap a new prop is simply
unwatermarked, harmless since phases only move forward). Regression
tests: `unresolvable_rows_never_grow_the_applied_ledger` (drains >4,096
distinct unresolvable keys across frames, ledger stays 0) and
`the_applied_ledger_has_a_hard_cap_even_for_resolved_rows`. Also guarded
the reviewer's `BangerFragment.index` gap: clamp is now 254 so a
pathological set can never alias the 255 placement sentinel. Still open
(unchanged, reviewer-listed): two-process retail site-agreement check,
measured impairment/real-GPU legs for `Props`. Status: implemented
candidate, not independently checked. Results of this repair's gates are
at the end of this file.

---

# Previous iteration — world-prop replication (new-run iteration 2)

Selection: F26-A, the prop half of F26-AC01. The previous review passed
with no blocking findings, so no repair work was owed. Of the operator's
networking follow-ups (report 6) the world-clock slice had landed; the
largest remaining F26-A gap was that **a client's props never move at
all** — `activate_bangers`/`settle_bangers` are authority-only and the
module doc promised "replication (F26) delivers authoritative
`BangerStateChanged`", which nothing did. Proximity leaves / latency
compensation stay open as recorded (no RTT, no retail `prox` path).

Design decision (recorded here, nobody to ask): replicate *state*, not
events, as its own frame. `Message::Props` (protocol v17, host→client
only) instead of a new `Snap` field, which would have touched 54 test
constructors for no gain and ties props to the pose stream's watermark.
Identity is a new `BangerSite` stamp ordinal (`Session::mint_banger_site`),
**not** `ObjectId`, whose slots interleave with vehicles/remote seats/
fragments in per-process order. Fragments are named `(site, piece index)`
via a `BangerFragment{parent,index}` tag. Bounded and loss-tolerant: every
active body + fresh changes + a rolling 8-row resend window of
settled/broken, every 2nd `Update`, ≤96 rows; client inbox ≤4,096,
latest-wins per prop on `(generation, tick)`, held through `Loading`,
phases never regress. Details: `docs/research/net.md`, ledger DSN-70.

Code: `mm2_net::proto` (`SnapProp`, `Message::Props`, `MAX_SNAP_PROPS`,
`OversizeProps`), `mm2_game` (`BangerSite`, `BangerFragment`,
`Session::mint_banger_site`), `mm2_app::banger` (`shatter_placement`,
`spawn_fragment` shared by authority and client; fragments tagged),
`city::spawn_banger_prop` (site stamp), new `mm2_app::worldprops`
(`publish_props`, `apply_props`, `PropStage` inside `RemoteSnaps`), wired
into `main.rs`, `smoke.rs` and the `net_app` harness.

Tests added: proto round-trip/bounds/truncation (mm2_net 3 + lobby 1:
a client-sent `Props` drops the peer, AC04); `mint_banger_site` is
independent of object-slot order; real pathset stamp gives consecutive
sites and fragments carry piece tags (`tests/banger.rs`); 7 `PropStage`
unit tests; 4 `net_app` legs (client rows incl. stale/unknown/NaN/foreign
generation/fragment-past-pieces, load-time hold, publish-window bound and
cycle, two-app host→client convergence over a real loopback socket with
the production systems). New legs spin under bounded deadlines on the
condition, never fixed frame counts (report 6 rule).

Gates (all exit 0): `cargo fmt --all -- --check`; `cargo clippy --locked
--workspace --all-targets --all-features -- -D warnings`; `cargo test
--locked --workspace` (mm2_app lib 143, app suite 763, network suite 85,
mm2_net 86 …, 0 failed). Retail sf `--headless --frames 300` smoke
(`/Users/linus/coding/rust-mm2/retail`): `status=pass`, `bng=6110d`
dormant bangers stamped — no regression from the site stamp. No test
processes left running.

Not verified / open (F26-A stays active, not AC01..06 completion): no
interpolation or velocities (active props move at the publish rate); no
measured impairment cell or two-process/real-GPU leg for `Props`; site
agreement across peers rests on deterministic stamp order (all `Vec`
iteration, seeded parked-car rolls) and was checked only on synthetic
stamps — a two-process retail check that both sides report the same
site→position table is the next evidence step; traffic/weather/time-of-
day replication; late-join beyond the resend cycle (~sites/8 frames);
drawbridge/mover/sound state still clock-only. Status: implemented
candidate, not independently checked.

## Repair gate results (iteration 3)

`cargo fmt --all -- --check` exit 0; `cargo clippy --locked --workspace
--all-targets --all-features -- -D warnings` exit 0; `cargo test --locked
--workspace` exit 0 (1962 passed, 0 failed; includes the 2 new
`worldprops` ledger tests). No retail/graphical run this iteration. No
test processes left running.
