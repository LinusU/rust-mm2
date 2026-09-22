# Original menu capability audit (F17-AC05)

The documented original menu surface (ledger UI-*/DRV-*/CTL-* rows,
help topics, booklet) mapped onto the current `mm2_app::menu` shell.
Statuses:

- **implemented** — a real, code-tested capability exists today.
- **tracked** — visible as a disabled row naming the reason, or
  explicitly assigned to a feature task. Never a dead-end placeholder
  that navigates to a fake screen.
- **open** — no menu presence yet; the task named owns it.

## Main menu entries (UI-1, help:Main Menu Screen)

| Original entry | Status | Notes |
|---|---|---|
| Crash Course | tracked | `Events → city → Crash Course` row stays visible with "not loadable yet (F21)". |
| Races | implemented | `Events → city → table → event list`, availability gates on rows (CHK-3-style). Per-event condition options landed (below) — deferred fields stay open. |
| Multiplayer | tracked | Disabled root row, reason names F24. |
| Quick Race | implemented | DRV-8 leg: replays the bound profile's stem-keyed `last_event`; stale events disable with the reason. The original's vehicle-select interstitial is folded into the persistent root `Vehicle:` pick — an enhanced-layout choice, not a parity claim. |
| Driver select/create/delete | implemented | Profiles screen binds; New driver is a real text field; delete sits behind a confirm screen; DRV-7's last-profile refusal is a status line. |
| Driver's Stats | tracked | Disabled root row naming this audit. Nothing persists aggregate stats to display. |
| Race Records | implemented (first leg) | `Screen::Records`: persisted per-event finishes/best time/best place, city and race-type filters, re-launch from a record row. Open legs below. |
| Options | tracked | Disabled root row naming F23 — covers CTL-3/4/5 (control, graphics, audio option screens). |

## Race Records open legs (DRV-5, help:Race Records)

- **Sort keys beyond city/race type** — the documented screen also
  sorts by Amateur Times / Pro Times / Pro Points. The persisted
  `EventRecord` keeps one best time (difficulty-agnostic) and no
  Pro-points field at all — DRV-4's points formula is UNK-8 and no
  producer writes points. The screen deliberately shows only stored
  fields rather than fabricating columns.
- **DRV-6 enforcement** — `record_eligibility` excludes modded
  sessions, dev overrides, non-city worlds, the synthetic car and
  (since F17-A.6) any `SessionCustomization` — a run launched with
  changed condition picks is not a default-conditions run
  (`Ineligible::Customized`). The menu only sets the field when the
  picks differ from the session's seed, so opening the options screen
  and launching unchanged keeps eligibility.

## Other screens / behaviors

| Capability | Status | Notes |
|---|---|---|
| Race condition options (UI-2, RACE-3 `customizable`) | implemented (partial) | `Screen::Customize`: an `options` row under every event opens weather, time-of-day and traffic-density picks once `EventAvailability.customizable` (the race is beaten); cruise city rows carry an always-open `options` row (RACE-4). The screen seeds from the authored `RaceParams` block (neutral defaults on cruise), cycles selectors 0-3 / density in quarters, and launches through `SessionConfig::customization` — picks beat authored conditions (lighting binds the picked `.ltNN`) and head the ambient-traffic density chain. An unchanged visit sets no customization, so the run stays default/record-eligible; changed picks are excluded by `record_eligibility` (DRV-6). Out-of-range authored values disable the row with the reason rather than fabricating a seed. Deferred: pedestrian/cop density (no consumers — F19/F20), Circuit laps/opponents (DRV-6's named exclusions, no authored writers yet), Quick Race customization. |
| Vehicle select detail (UI-3) | implemented (partial) | Lock reasons on cars and paints, paint list. The four stats bars and transmission choice are not displayed — open (F22/UI polish, no dedicated task yet). |
| Per-screen Options + Help "?" (UI-4) | open | No help system exists; F23 scope. |
| Results screen (UI-5) | implemented | Results overlay shows placing + total time; C&R points leg is F27 scope. |
| Esc backs toward main menu (UI-4) | implemented | Back pops the screen stack; Esc at root exits. |
| Keyboard/gamepad navigation | implemented | Arrows/WASD, dpad/stick, Enter/Space, X delete, edge-triggered stick. |
| Mouse navigation (spec req 5) | implemented | Hover focuses (edge-triggered), left-click activates the hovered row, right-click backs out. Pause/results overlays intentionally have no mouse path — original overlay clickability is unaudited. |
| In-game menu (CTL-8) | implemented | Pause overlay: Resume / Restart / Quit-to-menu. |
| Original menu art, layout and audio | open | Current shell is a functional text UI; original look is unimplemented and unverified. |

## Design classifications used here

- **Original requirement** — DRV-5 records viewing, DRV-7 last-profile
  protection, DRV-8 Quick Race, UI-2 gating: implemented against the
  documented rules.
- **Enhanced policy** — single flat event tree instead of per-type
  menus; Quick Race without an interstitial vehicle pick;
  edge-triggered mouse hover; text-only presentation. Deliberate
  modernizations, not parity claims.
- **Implementation choice** — `MenuCommand`/`MenuEffect` model,
  persisted `EventRecord` as the records data source.
- **Unknown** — original records-screen sort/filter semantics in full
  (Pro Points formula is UNK-8), original hover/click behavior,
  per-screen help content, Driver's Stats fields. None of these are
  claimed verified.
