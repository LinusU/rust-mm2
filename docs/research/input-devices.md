# Input-device capability record (F23-AC06)

F23 req 5: audit wheel / force-feedback support by actual platform and
device capability, and report unsupported or untested hardware honestly.
This is that record, one line per capability. `mm2_app::devices::RECORDS`
holds the same lines (a unit test fails if the two drift) and the app logs
them at startup, then names each pad the OS reports as it connects or is
removed.

Status vocabulary:

- **synthetic only** — implemented, exercised by raw events fed through the
  production systems (`tests/input.rs`, `tests/device_transitions.rs`,
  `tests/menu.rs`, `tests/session.rs`). No physical device, real window
  focus change or live keyboard session has been recorded.
- **not implemented** — the code does not do this. Nothing is claimed.

No row is hardware-verified. Recording a hardware session means adding a
status to `devices::Status`, a row here naming the device, OS and date, and
the command that reproduced it — not editing a note.

## Capabilities

- keyboard driving: synthetic only — bound keys drive the normalized input; no real keyboard session recorded
- key rebinding: synthetic only — driving and in-game keys rebind on the main-menu and pause Controls pages and persist in controls.json; driven by synthetic key events
- gamepad driving: synthetic only — stick, triggers and South through the deadzone/sensitivity map; any physical pad is untested
- gamepad menus: synthetic only — D-pad, stick edges, South/East/West/Start over every connected pad; synthetic events only
- gamepad hot-plug: synthetic only — connection and disconnection events clear and restore driving; no physical unplug performed
- focus-loss release: synthetic only — KeyboardFocusLost and unfocused windows release held input; no real window focus change
- pad rebinding: synthetic only — every digital in-session pad button (handbrake, manual shifts, camera, mirror, map, HUD...) rebinds on the main-menu and pause Gamepad buttons pages and persists in controls.json; stick and triggers stay analog, Start/Mode are app-owned; synthetic pad events only
- mouse driving: synthetic only — an opt-in Controls row (off by default): the cursor's offset from the window centre steers, left button throttles, right brakes/reverses (CTL-2's documented scheme, exact original feel unverified); synthetic window and button state only, no real mouse
- manual transmission: synthetic only — a Controls-page policy pins the gearbox through the shift keys (G/B) or the pad shoulders (right up, left down; they stop cycling the nav-arrow target while manual), inert on a predicted multiplayer client; the brake pedal still doubles as reverse once stopped
- steering wheel: not implemented — no wheel-specific mapping; a wheel the OS exposes as a gamepad would be read as a pad, axis layout untested and no wheel hardware available
- force feedback: not implemented — no rumble or force-feedback requests are sent to any device; untested on hardware

## Platform

Gamepads arrive through Bevy's gilrs backend; the pad's name and USB ids in
the startup log are whatever the OS reports. The capture/evidence runs
(`--frames`) read no devices, so none of this is exercised by them.
