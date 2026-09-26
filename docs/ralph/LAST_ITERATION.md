# Last iteration — F22-A.5 the authored nav arrow (iteration 70)

Iteration 70 on `ralph/night` (baseline `3e0126d`, F22-A.4 — external
verify + review green; fifteenth iteration of run `20260925T144723`).
One coherent slice: HUD-2's compass arrow — the `mmArrow` the
recovered `mmHUD` layout owns — bound to the installation's own
`hudarrow*` package geometry and `s_hudarrow_*` tiles instead of the
dev needle/diamond stand-in.

## Task selection

No failing gate or review finding — iteration 69's review passed
(`verdict: pass`, no blocking findings), so the queue reopens. Of the
F22-A remainder (HUD-2 race instruments), the arrow is the
self-contained leg: while auditing the archive for the still-open
checkpoint/lap/place tiles, retail shipped the answer to the needle —
`geometry/hudarrow{01,_blitz01,_cc01}.pkg` are flat-XZ chevron meshes
with exactly two paint jobs apiece (a family tile —
`s_hudarrow_green`/`_red`/`_violet` — then the shared
`s_hudarrow_yellow`), the authored ahead/behind colour pair RACE-6
documents; `mmHUD` names `mmArrow` its owner. The `race_*` labels
(`_chk`/`_lap`/`_opp`/`_rec`) fail the current TGA reader (alpha-masked
variant) — recovery deferred — so the checkpoint/lap/place instruments
stay open in the parent.

## What landed

- `mm2_app::navarrow` (new) — the arrow code moved out of `race.rs`,
  mirroring the `racetime`/`oppind` module precedent. `spawn_nav_arrow`
  selects the package by `EventTableKind` (`hudarrow_blitz01` on Blitz,
  `hudarrow_cc01` on Crash Course — already correct though CC sessions
  still can't reach runtime — `hudarrow01` otherwise), reads the pkg
  through `hudmap::read_pkg`, resolves each paint job's texture stem
  through `city::load_image` (VFS-preferred, so mods can substitute
  `png`/`ktx2`/`tex`), and CPU-rasterizes each of the first two paint
  jobs into an 80 px RGBA sprite: top-down projection of the flat XZ
  chevron, mesh origin (the authored pivot — the tail sits at the
  origin, the tip points −Z) centred on the canvas, per-pixel `y`
  ordering, texture-space fill via the same
  `paint * shaders_per_paint_job + shader_offset` indexing `city.rs`
  uses (negative offsets untextured, non-triangle strips skipped).
  Any failure aborts the whole spawn with `absent:<missing-pkg /
  unparseable-pkg / no-shaders / missing-texture / undecodable-texture /
  no-geometry>` — no substitute art, no half-bound node. One paint job
  still binds (behind reuses ahead's sprite).
- `update_nav_arrow` — same live contract on the authored sprites:
  `UiTransform` rotation = the signed bearing to the active target,
  `ImageNode` swaps ahead/behind across the ±90° line; hidden under
  `Ordered` rules, stale generations, `Complete`/resolved states,
  missing race state, and the `H` gate — while `NavArrowReport`
  (`stem`, `facing` ahead/behind/off) keeps recording like `tmr=`'s
  `display`. `nav_target_input` moved here unchanged (X forward, Z
  back — DSN-8's WASD departure).
- `session.rs` — the event arm hoists the event key so
  `spawn_nav_arrow` sees the family; teardown removes
  `NavArrowReport` (entities die via `SessionEntity`).
- `smoke.rs` — ` arr=<stem>/<ahead|behind|off>` or `absent:<why>`
  appended after `tmr=` on event sessions only; cruise/dev-world
  records stay bit-identical.
- Deleted: `NavArrowPart`, `NAV_AHEAD`/`NAV_BEHIND`, the two
  node-drawn children — the authored art replaces the stand-in.

## Gates

- `cargo test -p mm2_app --test navarrow` — 15/15 new: full binding
  (`SessionEntity` root, both sprites stored, `arr=hudarrow01/ahead`),
  every `absent` cause (missing/unparseable pkg, no-shaders,
  missing/undecodable texture, no-geometry), the Blitz/CC/Checkpoint
  package pick, rasterizer coverage (opaque tip pixel, transparent
  margin, tint preserved), single-paint reuse, bearing→rotation and
  ahead/behind swap with `facing` record, released-state and `H`-gate
  hiding while the report keeps composing, `SessionEntity` teardown,
  synthetic-event `headless_smoke` legs (`arr=hudarrow01/ahead`,
  `arr=absent:missing-pkg`), dev-world field absence.
- `cargo test -p mm2_app --test race` — 43/43 (the bearing/target
  cycling legs now drive the real binding through a synthetic
  `hudarrow01.pkg` + `s_hudarrow_*` mount — the same
  `test_arrow_install`/`write_test_pkg` fixture family the smoke tests
  use); `tests/hud.rs` 8/8 (the stub swaps `NavArrowPart` for
  `ImageNode`).
- `cargo fmt --all -- --check` clean; `cargo clippy --locked
  --workspace --all-targets --all-features -- -D warnings` clean;
  `cargo test --locked --workspace` all suites green.
- Retail headless (`fnv1a64:e91e6cd4b2ae30d9`, read-only):
  `--city sf --event checkpoint:0 --headless --frames 200` →
  `status=pass`, `arr=hudarrow01/ahead` beside `tmr=22g/0:00:33`;
  `--city london --event blitz:0` → `arr=hudarrow_blitz01/ahead`
  beside `tmr=22g/0:24:66` (the family-variant package picks on
  authored data). The first run of this leg exposed a gap: the
  headless app never scheduled `update_nav_arrow`, so `arr=` read
  `off` — fixed (same after-`drive_session` slot as the timer) and
  the smoke test tightened to reject a `off` facing on a live race.
- Retail windowed (Apple M1, Metal): `--city sf --event checkpoint:0
  --frames 150 --screenshot` renders the authored green chevron at
  top-centre pointing at the first gate, over the `GO!` field with
  the `0:00:23` timer row under it and the opponent indicators live
  (`/tmp/f22a5-arrow-sf.png`, local, not committed).

## Classification

The instrument is a documented original member of `mmHUD` (HUD-2 +
mm2hook's `mmArrow`) and the artwork + ahead/behind colour pairing is
verified retail content — the two-paint layout *is* the documented
green/yellow flip (family tile ahead, shared yellow behind). What
stays designed (DSN-8, UNK-33): the on-screen size and top-centre
slot (kept from the dev needle), the 80 px canvas, sprite
rasterization itself (the original likely draws the mesh directly —
no `mmArrow` draw body recovered), and whether the original rotates
about the authored origin or a centroid. `docs/original-rules.md`
updated (RACE-6, HUD-2, DSN-8, DSN-52 refs, UNK-33).

## Remaining open items

- F22-A stays `active` — HUD-2's remaining instruments: checkpoint
  list, lap record, place indicator (the `race_*` tiles — alpha-masked
  TGAs the current reader rejects; recovering that variant is a
  prerequisite), plus AC01–AC03/AC06 evidence legs.
- UNK-33 needs a retail-original capture or a recovered `mmArrow`
  body — our on-screen size/position/pivot are designed readings; the
  windowed capture verifies *our* rendering, not the original's.
- The `race_*` TGA variant (alpha-masked) needs format support
  before the remaining instruments can bind their labels.

---

# Last iteration — F22-A.4 the authored race timer (iteration 69)

Iteration 69 on `ralph/night` (baseline `cf78496`, F22-A.3 — external
verify + review green; fourteenth iteration of run `20260925T144723`).
One coherent slice: HUD-2's stopwatch/countdown pair — the `mmTimer`
instruments the recovered `mmHUD` layout owns — rendered from the
installation's own `digitac_*`/`digi_colon` glyph art instead of the
dev telemetry line's text field.

## Task selection

No failing gate or review finding — iteration 68's review passed
(`verdict: pass`, no blocking findings), so the queue reopens. Of the
F22-A remainder (HUD-2 race instruments), the timer is the
self-contained leg: the recovered `mmHUD` layout names the
stopwatch/countdown `mmTimer` pair explicitly, the install ships the
exact glyph artwork (`digitac_0..9` + `_half`, `digi_colon` +
`_half`), and `RaceState` already exposes `clock`/`time_remaining`
with pause/finish freeze semantics. The checkpoint list, lap record
and place instruments — the `race_*` label tiles' consumers — stay
open in the parent.

## What landed

- `mm2_app::racetime` (new) — `spawn_race_timer` (event-arm spawn in
  `load_session_world`): binds all 22 authored stems through
  `city::load_image` — VFS-preferred so mods can substitute
  `png`/`ktx2`/`tex` — into a `TimerDigits` bank on the row root; any
  miss aborts the whole spawn (`absent:missing-glyphs`, `glyphs`
  counts how far it got — never a substitute glyph or half-bound
  row). `update_race_timer` recomposes the row every frame off the
  authoritative clock: `time_remaining` while a timed (Blitz)
  definition runs, `clock` otherwise — armed through `Countdown`
  (full limit or `0:00:00`), dark on `Complete`/stale generations,
  `H`-gated like every `mmHUD` member while `RaceTimerReport.display`
  keeps composing so `tmr=` records demand (the `ind=` `bound`
  precedent). Runs ungated by `capturing` like `drive_mirror`.
- Presentation — designed reading (DSN-53; the original layout is
  UNK-32): a top-centre row at 96 px under the nav arrow, laid out
  `m:ss:hh` — full digits for minutes/seconds, the authored half-size
  set for centiseconds, `digi_colon` separators, leading-zero
  suppression on minutes capped at `999:59:99`, on a translucent
  plate tinted to the colon tile's own authored background
  (`digi_colon` is an opaque-panel image, so its tile reads as plate).
  `LOW TIME` moved from 108 px to 170 px — its old slot sits inside
  the plate.
- `camera.rs` — `HudNodes` gained `RaceTimer` plus a repair folded in
  while reading the retarget: `NavArrow` and `LowTimeWarning` were
  never in the set, so they fell back to `DefaultUiCamera`'s
  max-order primary-window pick — the mirror strip (order 2) — and
  rendered inside it (or nowhere) whenever it was armed. Every
  HUD-layer root now rides the active world camera.
- `session.rs` — event arm spawns + inserts `RaceTimerReport`;
  teardown removes it (the row dies via `SessionEntity` like the rest
  of the rig).
- `smoke.rs` — `update_race_timer` scheduled in the same slot as
  `drive_opponent_indicators`; the record appends
  ` tmr=<glyphs>g/<m:ss:hh|off>` or `absent:<why>` on event sessions
  only — cruise/dev-world records stay bit-identical.

## Gates

- `cargo test -p mm2_app --test racetime` — 13/13 new: `m:ss:hh` slot
  composition incl. the `999:59:99` cap, full-set binding
  (`22g`, `SessionEntity`-stamped row, 9 slots), partial/missing sets
  report `absent` with no row, untimed count-up (`0:06:25` at 750
  ticks with per-slot image assertions), timed count-down
  (`0:40:00`), countdown arming, `Complete`/stale/cruise release,
  `H` hides the row while `display` keeps composing, synthetic-event
  `headless_smoke` legs (`tmr=22g/…` and `tmr=absent:missing-glyphs`),
  dev-world field absence.
- `cargo fmt --all -- --check` clean; `cargo clippy --locked
  --workspace --all-targets --all-features -- -D warnings` clean;
  `cargo test --locked --workspace` all suites green.
- Retail headless (`fnv1a64:e91e6cd4b2ae30d9`, read-only):
  `--city sf --event checkpoint:0 --headless --frames 200` →
  `status=pass`, `tmr=22g/0:00:33` — the display matches the
  authoritative `ticks=40` exactly (40 × 5/6 = 33 cs); `+ --no-hud`
  → same record plus `hud=off`, `tmr=` still composing.
- Retail windowed (Apple M1, Metal): checkpoint `--frames 400` shows
  `0:04:44` agreeing with the telemetry line's `4.4s`; blitz
  `--frames 120` shows the countdown banner `1` beside the armed
  `0:30:00` deadline; `--cockpit` shows `0:02:73` retargeted to the
  cockpit camera; `--mirror` parks the strip over the nav-arrow band
  with the timer just under its edge (documented overlap — the strip
  is a world-camera-order-2 viewport, so band instruments it covers
  clip under it; same as the nav arrow pre-change). Captures local
  (`/tmp/f22a4-*.png`, not committed).

## Classification

The instrument itself is a documented original member of `mmHUD`
(HUD-2 + mm2hook's `mmTimer` pair) and the glyph art is verified
retail content; the composed layout — position, `m:ss:hh` fielding,
zero suppression, plate — is a designed reading (DSN-53) because no
retail capture or recovered draw body pins it (UNK-32, including the
`race_*` label tiles' real placement and whether the original shows
two timers at once). The retarget repair is an implementation fix —
no original-behavior claim. `docs/original-rules.md` updated (HUD-2
row + DSN-53/UNK-32).

## Remaining open items

- F22-A stays `active` — HUD-2's remaining instruments: checkpoint
  list, lap record, place indicator (the `race_*` label tiles' real
  consumers) over the dev line, plus AC01–AC03/AC06 evidence legs.
- UNK-32 needs retail captures or a recovered `mmTimer` draw body —
  position/padding/dual-timer semantics are designed readings.
- Mirror-strip overlap: UI targeted to the world camera renders under
  the strip's order-2 pass in its band (pre-existing; nav arrow
  suffers it too). An overlay-order camera or strip-below-instruments
  layout is future designed work.
- The armed countdown leg (`GO!` flash under a live timer) and
  timeout-at-zero edge renders are unverified visually.

---

# Iteration 68 — F22-A.3 the `H` HUD toggle (iteration 68)

Iteration 68 on `ralph/night` (baseline `507abf8`, F22-A.2 — external
verify + review green; thirteenth iteration of run `20260925T144723`).
One coherent slice: the documented `H` toggle (HUD-3/CTL-1) over the
whole driving-HUD layer — the last unbound HUD-3 control and the
smallest remaining piece of the F22-A race-HUD remainder.

## Task selection

No failing gate or review finding — iteration 67's review passed
(`verdict: pass`, no blocking findings), so the queue reopens. Of the
F22-A remainder (HUD-2 race instruments + `H`), the toggle is the
self-contained leg: mm2hook recovery shows `mmHUD` is one node owning
`mmHudMap` (which draws the opponent indicators), `mmArrow`, the
stopwatch/countdown `mmTimer`s, `mmDashView` and `mmCRHUD`, with
`Enable`/`Disable`/`Toggle` — so `H` is a master gate over the whole
layer, not a dev-text switch. The authored race instruments
(checkpoint list, lap, place, stopwatch) stay open in the parent.

## What landed

- `mm2_app::hud` (new) — `HudVisible(bool)` session-agnostic toggle
  resource (designed on by default — the same lifecycle contract
  `RearView`/`OpponentIndicators` hold: a restart respawns the
  session-owned HUD entities while the driver's choice survives),
  `hud_input` (`H` in `Playing`/`Countdown` only — pause/results/menu
  overlays keep the key), and `update_hud` moved here from the bin
  target so tests reach it. The telemetry line writes `Hidden` under
  the gate; `ErrorText` is excluded (a load-failure surface, not a
  driving instrument).
- Per-driver gating (state keeps computing, only rendering is
  suppressed): `race.rs` nav arrow / countdown banner / low-time
  warning write `Hidden`; `hudmap.rs` parks the map camera unless the
  *fullscreen pause map* is up — a menu surface, deliberately outside
  the gate; `oppind.rs` hides the marker pool; `dash.rs` folds `hud.0`
  into the cockpit split so the dash cluster goes dark while the
  cockpit *camera* keeps rendering (`is_active` is a camera's only
  render gate). The rear-view strip stays independent — a camera,
  not an instrument.
- `config.rs`/`main.rs` — `DevOverrides::no_hud` + `--no-hud`
  (render-only like `--mirror`, excluded from `record_eligibility`);
  `init_resource::<HudVisible>` seeded from the override in the
  windowed app and headless smoke.
- `smoke.rs` — `hud=off` appends to the record only when the gate is
  off; default records stay bit-identical.
- Visibility-propagation fixes the retail capture exposed: `dash.rs`
  pivot/holder nodes and `car_visual.rs` wheel-spin/fender nodes now
  spawn `Visibility::Inherited` — Bevy's explicit `Visible` overrides
  a `Hidden` ancestor, so the authored subtrees ignored both the new
  HUD gate and (for wheels/fenders) the existing cockpit split.

## Gates

- `cargo test -p mm2_app --test hud` — 8/8 new: phase gating
  (Menu/Paused/Results keep `H`), gate survives session teardown,
  indicator markers / map camera / instrument line / dash cluster /
  race instruments each hide with the layer, `hud=off` records.
- `cargo fmt --all -- --check` clean; `cargo clippy --workspace
  --all-targets --all-features -- -D warnings` clean; `cargo test
  --workspace` all suites green (0 failures).
- Retail headless (`fnv1a64:e91e6cd4b2ae30d9`, read-only):
  `--city sf --event checkpoint:0 --headless --frames 200` →
  `status=pass`, record identical to baseline (no `hud=` field);
  `+ --no-hud` → same record plus `hud=off`, all other fields
  (`map=inset/…`, `ind=on/6m/6b`, `dash=11p/cam`) preserved.
- Retail windowed (Apple M1, Metal): `--cockpit --no-hud` renders the
  bare windshield — dash cluster, telemetry, minimap, indicators and
  banner all suppressed while the authored cockpit camera stays live;
  `--cockpit` alone renders the full cluster + HUD layer;
  `--pause-map --no-hud` renders the fullscreen authored map under the
  gate. Captures local (`/tmp/f22a3-*.png`, not committed).

## Classification

The `H` binding is a documented original control (HUD-3/CTL-1) and
`mmHUD`'s recovered membership fixes the scope — the whole driving-HUD
layer including the map, indicators and dash view (DSN-52 records the
designed reading that each driver suppresses rendering rather than one
root flipping). UNK-31 records the still-open semantics: whether the
original suppresses the rear-view mirror or the fullscreen pause map,
whether the toggle persists across sessions, and the original start
state. `docs/original-rules.md` updated (HUD-3 row + DSN-52/UNK-31,
plus the DSN-51/UNK-30 rows A.2 referenced but never added); README
controls table gains the `H` row.

## Remaining open items

- F22-A stays `active` — HUD-2's authored race instruments remain:
  checkpoint list, lap record, place indicator, stopwatch, countdown
  presentation refinement, plus AC01–AC03's evidence legs.
- F22-B stays `active` — camera occlusion handling, the
  chase-near/far split, and the unresolved original mirror semantics
  (UNK-29).
- UNK-31's original-semantics legs need retail captures — mirror and
  pause-map behavior under `H` are designed readings, not recovered
  facts.

---

# Iteration 67 — F22-A.2 opponent indicators (iteration 67)

Iteration 67 on `ralph/night` (baseline `f64e2a3`, F22-B.2 review
repair — external verify + review green; twelfth iteration of run
`20260925T144723`). One coherent slice: the documented `I` opponent
indicator (HUD-3/CTL-1) — the smallest complete piece of the F22-A
race-HUD remainder.

## Task selection

No failing gate or review finding — iteration 66's repair passed
external review (`verdict: pass`), so the queue reopens. The F22-A
remainder was named the likely next slice: HUD-2's race instruments
over the developer telemetry line and the two documented HUD-3
controls still unbound (`H` HUD, `I` opponent indicator). The
indicator is the smallest self-contained piece of that remainder —
one control, one instrument — while the compass arrow/checkpoint
list/lap/place/stopwatch presentation and the `H` HUD toggle stay
open in the F22-A parent.

## What landed

- `mm2_app::oppind` (new) — the indicator module:
  - `OpponentIndicators(bool)` — session-agnostic toggle resource
    (same lifecycle contract as `RearView`): a restart respawns the
    pool and the drive system re-applies the driver's choice. On by
    designed default (DSN-51 — the original's start state is
    unrecovered).
  - `OppIndReport` — session-scoped load report: `markers` (pool
    slots = authored roster size), `bound` (live opponents bound on
    the last drive pass — counts demand even while toggled off),
    `absent:<why>` (`missing-pkg`/`unparseable-pkg`/`empty-pkg`).
    Inserted only by event sessions; cruise/dev-world records stay
    bit-identical.
  - `spawn_opponent_indicators` — event-arm spawn sized to
    `roster.entries.len()`, every marker `SessionEntity`-stamped.
    Marker geometry/materials bind the authored
    `geometry/hudmap_tri.pkg` through the VFS — a missing or
    unparseable package records `absent`, never a substitute mesh —
    scaled from its measured authored extent to a designed in-world
    size (1.5 m), painted per-slot from the shared
    `TRI_PAINT_OPPONENTS` authored palette so an opponent's arrow
    matches its minimap tri (both pools bind in entity order).
  - `indicator_input` — `I` toggles in `Playing`/`Countdown` only, so
    pause/results/menu overlays keep the key (the same contract
    `mirror_input` holds for BACKSPACE).
  - `drive_opponent_indicators` — rebinds the pool every frame to
    live non-local `Player` participants (`control != Local` — AI
    today, remote drivers once F25 exists; never ambient traffic,
    never the local car), sorted by entity. Each marker rides the
    opponent's authored collider/`chassis_size` roof plus a designed
    gap — per-car height, so tall vehicles clear it — stands the flat
    tri upright apex-down, and yaw-faces the active `WorldCamera3d`
    camera (map and mirror cameras can never be the facing source).
    Despawned or vehicle-less participants free their slot the same
    update — no marker can hover over a stale or invalid opponent
    (AC03's stale-participant leg for this instrument). Runs ungated
    by `capturing`, like `drive_mirror`, so `--frames`/`--screenshot`
    runs render the markers.
- `hudmap.rs` — `read_pkg`, `authored_extent`, `paint_material` and
  `TRI_PAINT_OPPONENTS` promoted to `pub(crate)`; `oppind` binds the
  same authored content rather than duplicating the loaders.
- `session.rs` — the event arm spawns the pool after
  `spawn_opponents` and inserts the report; teardown removes
  `OppIndReport` with the other session-scoped reports (the markers
  die via `SessionEntity` like the rest of the rig).
- `main.rs` — `init_resource::<OpponentIndicators>`,
  `indicator_input` gated `not(capturing)` beside `mirror_input`,
  `drive_opponent_indicators` ungated in the
  after-`drive_session` slot.
- `smoke.rs` — the headless app gets the same resource + systems
  (own schedule slot — the big Update tuple is at Bevy's system-arity
  limit; the windowed app already splits this way for the map/mirror
  drivers) and records ` ind=<on|off>/<pool>m/<bound>b` or
  `absent:<why>` on event sessions only.

## Gates

- `cargo test -p mm2_app --test oppind` — 5/5 new:
  `i_toggles_only_in_the_live_phases` (Menu/Paused/Results keep the
  key; Countdown/Playing toggle), `markers_ride_only_live_opponents`
  (two opponents on a two-slot pool — per-car heights from the
  dev chassis roof vs an authored 3 m hull; local car never marked;
  `Remote` control binds; despawn frees the slot same update;
  vehicle-drop unbinds; toggle-off hides all while `bound` still
  reports demand), `missing_tri_package_reports_absent`,
  `event_session_reports_the_bound_pool` (synthetic
  `race/testcity/` checkpoint event wiring one `vpt` opponent +
  authored tri through the real `headless_smoke` pipeline →
  `ind=on/1m/1b`), `dev_world_has_no_indicator_field`.
- `cargo fmt --all -- --check` clean; `cargo clippy --locked
  --workspace --all-targets --all-features -- -D warnings` clean;
  `cargo test --locked --workspace` all suites green (oppind.rs +5,
  0 failures).
- Retail headless (`fnv1a64:e91e6cd4b2ae30d9`, read-only):
  `--city sf --event checkpoint:0 --headless --frames 200` →
  `status=pass`, `ind=on/6m/6b`, `pos=3/7` — six authored opponents
  bound over the full field; roster warnings preserved verbatim
  (aimap wires 6, table authors 7).
- Retail windowed (Apple M1, Metal): `--city sf --event checkpoint:0
  --frames 100 --screenshot` renders the countdown grid with a
  painted down-pointing arrow over each visible staged opponent
  (paint 4 orange over the left car, paint 1 blue over the right).
  Capture is local (`/tmp/f22a2-ind-sf.png`, not committed).

## Classification

The `I` toggle is a documented original control (HUD-3/CTL-1). The
indicator's *presentation* is unrecovered — the documentation records
only the toggle — so the arrow shape/extent (1.5 m), the
collider-roof + 0.6 m lift, the authored-palette mapping and the
on-by-default start state are designed readings (DSN-51, UNK-30).
What is original-scope: the instrument binds authored
`hudmap_tri.pkg` content through the VFS and covers every non-local
participant. `docs/original-rules.md` updated (HUD-3 row, DSN-51,
UNK-30); README controls table adds the `I` row.

## Remaining open items

- F22-A stays `active` — the race-HUD remainder is still the dev
  telemetry line plus this slice: HUD-2's compass arrow, checkpoint
  list, lap record, place indicator, stopwatch/countdown instruments
  and the documented `H` HUD toggle stay open, alongside AC01–AC03's
  evidence legs.
- The indicator presentation (DSN-51/UNK-30) needs original
  verification — no retail indicator captures exist to compare
  against; the authored `hudmap_tri` reuse is a designed reading,
  not a recovered fact.
- `ind=bound` covers `PlayerControl::Remote` by construction but no
  remote participants exist yet (F25 groundwork only).
- Atypical vehicle sizes still need the manual capture passes
  recorded under F22-AC05 — the per-car roof math is tested
  synthetically but uninspected on `vpbus`/`vpsemi`.

---

# Iteration 66 — F22-B.2 review repair: HUD retarget excludes the strip (iteration 66)

Iteration 66 on `ralph/night` (baseline `9817490`, F22-B.2 — external
verify green but review **failed**; eleventh iteration of run
`20260925T144723`). One piece: the review's single blocking finding —
`retarget_hud` was the one "the active camera" consumer still missing
the `WorldCamera3d` filter.

## Task selection

Repair precedes feature work per the regression-first policy. The
iteration-010 external review rejected `9817490` with one blocking
finding: `retarget_hud` picked the active camera with
`Query<(Entity, &Camera), Without<HudMapCamera>>`, leaving the
`MirrorCamera` an eligible pick. Under `CameraMode::Cockpit` with
`RearView(true)` — the combination `dash.rs`'s visibility exemption
exists to support — the strip spawns ahead of the cockpit camera
(`load_session_world` parents it to the vehicle before `spawn_dash`
runs), so the first-active pick lands on it deterministically and
pins the `Hud`, `ErrorText`, `PauseUi`, `ResultsUi` and
`CountdownBanner` roots into the ⅓×⅛ top strip and off the main
view. Chase mode escaped only by spawn-order luck. The review's
suggested fix: apply `hudmap::WorldCamera3d` (same one-line shape as
the F22-B.1 repair's other picks) plus a regression test that the HUD
target stays on the world camera while the strip is active in Cockpit
mode.

## What landed

- `camera.rs` — `retarget_hud` + the `HudNodes` set moved here from
  the `mm2` bin target (the bin is unreachable from `tests/` — the
  same reason `active_cam_pose` moved in the F22-B.1 repair) and the
  camera query now takes `crate::hudmap::WorldCamera3d`: the strip
  and the map camera are never the UI target, and a stray menu
  `Camera2d` can't be picked either. The doc comment records the
  spawn-order mechanism that made the unfiltered pick deterministic.
- `main.rs` — schedules `camera::retarget_hud`; the local copy and
  its `HudNodes` alias are gone; the `update_hud` comment is updated
  to the new path.
- `pause.rs`/`results.rs` — doc references repointed at
  `camera::retarget_hud` (prose only).
- `tests/mirror.rs` — `the_strip_is_never_the_hud_target` reproduces
  the defective combination: Cockpit mode + armed strip + production
  spawn order (strip first, then the `CockpitCamera`), HUD/
  `ErrorText`/`CountdownBanner` roots live. Asserts the strip stays
  active (the hazardous pick exists) and every UI root's
  `UiTargetCamera` is the cockpit camera. Mutation-checked: with the
  old `Without<HudMapCamera>` filter the test fails, pinning
  `UiTargetCamera` to the strip entity.

## Gates

- `cargo test -p mm2_app --test mirror` — 8/8 (+1 above).
- `cargo fmt --all -- --check` clean; `cargo clippy --locked
  --workspace --all-targets --all-features -- -D warnings` clean;
  `cargo test --locked --workspace` all suites green (72 result
  lines, 0 failures).
- Retail headless (`fnv1a64:e91e6cd4b2ae30d9`, read-only):
  `sf --headless --mirror --frames 200` → `status=pass`, `mir=on`,
  `dash=11p/cam`, `pvs=687r/5932h/7324` — unchanged record.
- Retail windowed (Apple M1, Metal): `--city sf --mirror --cockpit
  --frames 90 --screenshot` renders the authored cockpit (dash,
  wheel, windshield) full-window with the rearward strip at
  top-centre, the HUD telemetry line on the main view at top-left
  and the map inset bottom-right — the exact combination the finding
  described, now with the UI on the right camera. Capture is local
  (`/tmp/f22b2-fix-mirror-cockpit.png`, not committed).

## Classification

Implementation repair only — no original-behavior claim changes
(DSN-50, UNK-29 stand). The `WorldCamera3d` pick is the same
implementation choice as the F22-B.1 repair's other consumers; the
review's suggested fix shape is what landed.

## Remaining open items

- F22-B stays `active` — unchanged open scope: camera
  obstacle/occlusion handling, the chase-near/far pair split (Free
  occupies the documented Chase-Far slot — DSN-48), plus prior
  unverified legs (retail `dash=` counts on london/vpbus; atypical
  vehicle sizes, wall proximity and reset transitions still need
  manual capture passes).
- A true mirror needs a flipped projection or clip-plane reflection —
  the strip is a plain rearward camera (designed, UNK-29).
- F22-A remainder: AC02/AC03 map legs and HUD-2 race instruments stay
  open.

---

# Iteration 65 — F22-B.2 rear-view mirror strip + F4 restart (iteration 65)

Iteration 65 on `ralph/night` (baseline `33607dc`, F22-B.1 — external
verify + review green; ninth iteration of run `20260925T144723`,
resuming the truncated iteration-009 session that ended mid-exploration
with an empty diff). One coherent slice: the documented `BACKSPACE`
rear-view mirror (HUD-3/CTL-1), which also frees the dev build's
borrowed restart binding to its documented `F4` key.

## Task selection

No failing gate or review finding — iteration 009's review passed on an
empty candidate (the session was truncated before implementation), so
the open F22-B remainder stands. The mirror slice is the smallest
complete piece of that remainder: it binds a documented control
(BACKSPACE rearview, F4 restart — both CTL-1/HUD-3) while the original's
mirror presentation stays honestly unrecovered (UNK-29). Occlusion
handling and the chase-near/far pair remain open in F22-B.

## What landed

- `camera.rs` — `MirrorCamera` component + `RearView(bool)` resource +
  `spawn_mirror` + `mirror_input` + `drive_mirror`. The strip is a
  rearward `Camera3d` (`order 2`, over the world view and map inset)
  parented to the player vehicle — pitch/roll move the view like a
  windshield mirror, and session teardown/reset can never strand it.
  Eye: authored `camPovCS` `Offset` when the car carries one (the seat
  position a real mirror reflects from), else a designed
  `chassis_size.y × 0.55` fallback; FOV/near/far bind the authored
  record with designed fallbacks. `drive_mirror` writes `is_active`
  from `RearView` (suppressed under `CameraMode::Free`) and maintains a
  top-centre `Viewport` strip (⅓ × ⅛ of the physical window,
  write-on-diff) — DSN-50. `mirror_input` toggles on BACKSPACE in
  `Playing`/`Countdown` only, so pause/results/menu overlays keep the
  key for Back. `RearView` is session-agnostic like `CameraMode`: a
  restart respawns the strip camera armed.
- `hudmap.rs` — `WorldCamera3d` now excludes `MirrorCamera`: the strip
  can never become the audio listener, PVS source, sky-dome anchor,
  billboard-facing view or HUD `cam` pose readout.
- `dash.rs` — `sync_dash_visibility` skips `MirrorCamera` children
  (Bevy auto-inserts `Visibility` on `Camera3d`; the split would have
  claimed and hidden it under Cockpit). The strip renders over the
  cockpit view; `is_active`, not `Visibility`, is its render gate.
- `session.rs` — `session_control_input` binds restart to `F4` (the
  documented original binding, CTL-1); `load_session_world` spawns the
  strip under the player vehicle carrying the same `DistanceFog` as
  the other cameras (an unfogged rear view would read as a different
  weather slot).
- `config.rs`/`main.rs` — `DevOverrides::mirror` + `--mirror` (arms
  `RearView` at spawn — the capture path while `--frames` freezes live
  input); render-only like `--pause`/`--cam`, deliberately out of
  `record_eligibility`, and counted for menu-mode/direct-launch.
  `drive_mirror` runs ungated by `capturing` like `drive_hud_map`.
- `smoke.rs` — headless app schedules `mirror_input`/`drive_mirror`,
  seeds `RearView` from the config, and records `mir=on|armed`
  on-activity only (off records stay bit-identical); `armed` without
  `on` is a printed discrepancy, never a silent pass.

## Gates

- `cargo test -p mm2_app --test mirror` — 7/7 new: overlay phases keep
  the key, Playing/Countdown toggle, Free-camera suppression,
  `WorldCamera3d` exclusion + `active_cam_pose` still reporting the
  forward camera, authored `PovCamSpec` eye/FOV/clips vs designed
  fallback, rearward yaw, and the cockpit-visibility exemption.
- `cargo test -p mm2_app --test session` — 19/19 (+1:
  `f4_restarts_and_backspace_is_the_mirror` — BACKSPACE toggles
  `RearView` and never queues restart; F4 drives the full
  Unloading→Menu→Loading cycle and the respawned strip re-arms).
- `cargo test -p mm2_app --test environment` — 17/17 (the fogged-camera
  count honestly moved 2→3: the strip binds the authored fog row too).
- `cargo fmt --all -- --check` clean; `cargo clippy --locked
  --workspace --all-targets --all-features -- -D warnings` clean (one
  targeted `too_many_arguments` allow on `sync_dash_visibility` — the
  filter-query count is intrinsic); `cargo test --locked --workspace`
  all suites green.
- Retail headless (`fnv1a64:e91e6cd4b2ae30d9`, read-only):
  `sf --headless --mirror --frames 200` → `status=pass`, `mir=on`,
  `dash=11p/cam`, `pvs=687r/5932h/7324` unchanged.
- Retail windowed (Apple Silicon, Metal): `--city sf --mirror
  --frames 90 --screenshot` renders the top-centre strip showing the
  rearward view (flush top edge, under the HUD telemetry line) while
  the HUD `cam` pose still reports the forward chase camera.
  Capture is local (`/tmp/f22b2-mirror-sf.png`, not committed).

## Classification

BACKSPACE-mirror and F4-restart bindings are documented original
controls (HUD-3/CTL-1). The strip's geometry, the un-mirrored
projection, the fallback eye and the Free-camera suppression are
designed readings — DSN-50; the original's mirror presentation is
UNK-29. `docs/original-rules.md` updated (HUD-3 row, DSN-50, UNK-29);
README controls table corrected (`C` cycle description, BACKSPACE→
mirror, F4→restart).

## Remaining open items

- F22-B stays `active` — the mirror leg lands; still open: camera
  obstacle/occlusion handling, the chase-near/far pair split (Free
  occupies the documented Chase-Far slot — DSN-48), plus prior
  unverified legs (retail `dash=` counts on london/vpbus).
- F22-AC05's visual legs are only partially met: the strip is verified
  on `vpbug` only; atypical vehicle sizes, wall proximity and reset
  transitions still need manual capture passes.
- A true mirror needs a flipped projection or clip-plane reflection —
  the strip is a plain rearward camera (designed, UNK-29).
- F22-A remainder: AC02/AC03 map legs and HUD-2 race instruments stay
  open.

---

# Iteration 64 — F22-B.1 review repair: world-space camera consumers

Iteration 64 on `ralph/night` (baseline `01c78a2`, F22-B.1 review
repair — external verify green but review **failed**; eighth iteration
of run `20260925T144723`). One piece: the review's single blocking
finding — every "active camera" world-space consumer read the cockpit
camera's car-local `Transform` as if it were world-space.

## Task selection

Repair precedes feature work per the regression-first policy. The
iteration-007 external review rejected `01c78a2` with one blocking
finding: `CockpitCamera` spawns as a child of the player vehicle
(`dash.rs`), so its `Transform` is the authored eye offset
(~(0,1.19,-0.55) m), but three consumers read `&Transform` as a world
pose:

1. `environment::drive_sky_dome` re-centred the 900 m dome on that
   local offset — under `CameraMode::Cockpit` the dome parked near the
   world origin permanently, so the cockpit view this slice adds
   rendered a clear-colour sky across most of each city.
2. `active_cam_pose` (the HUD `cam` readout and screenshot filenames)
   reported car-local coordinates, breaking the documented contract
   that a screenshot's pose round-trips into `--cam`.
3. `apply_city_pvs` resolved the local offset as a bogus extra source
   position near origin — over-show only (the player `Position` stays
   a correct source), but wrong.

The review's suggested fix: read the active camera's `GlobalTransform`
— `damage_fx`'s billboard query is the precedent — plus a regression
test that a vehicle-child active camera feeds the world pose to these
paths.

## What landed

- `camera.rs` — `active_cam_pose` moved here from `main.rs` (the bin
  target was unreachable from `tests/`); it now reads
  `GlobalTransform::compute_transform()` and takes the
  `crate::hudmap::WorldCamera3d` filter in its signature.
- `environment.rs` — `drive_sky_dome` reads the active camera's
  `GlobalTransform::translation()`; the pick tightened from
  `Without<HudMapCamera>` to `WorldCamera3d` (a stray active menu
  `Camera2d` on a transition frame is never the world view).
- `pvs.rs` — `apply_city_pvs` reads `GlobalTransform::translation()`
  for the view source (its `Camera3d` filter was already right).
- `main.rs` — `update_hud` and `screenshot_input` queries switched to
  `(&Camera, &GlobalTransform)` + `WorldCamera3d` and call
  `camera::active_cam_pose`; the bin-local helper is gone. The
  schedule comment is corrected: the propagated pose is one frame
  stale at worst, and the player `Position` source still covers the
  room under the car.
- All other camera consumers audited clean: `damage_fx` billboards
  already read `GlobalTransform`; `audio_listener` follows `is_active`
  (the cockpit camera gets `SpatialListener` automatically);
  `chase_follow`/`free_fly`/`cockpit_look` write `Transform` on their
  own entities correctly; `retarget_hud`/`drive_hud_map` touch no
  camera transform.

## Gates

- `cargo test -p mm2_app --lib pvs` — 7/7 (+1:
  `system_uses_a_child_cameras_world_pose` — a `Camera3d` parented to
  a vehicle stand-in resolves the room under the parent's world pose
  through real `TransformPlugin` propagation; the local-offset read
  would land near origin and never reach it).
- `cargo test -p mm2_app --test dash` — 10/10 (+1:
  `cam_pose_reports_a_child_cameras_world_pose` — the propagated
  vehicle-child camera reports `500.0,11.2,-300.5,0,0`, the world eye
  pose, not the local offset).
- `cargo test -p mm2_app --test environment` — 17/17 (+1:
  `dome_follows_a_vehicle_child_camera` — the dome re-centres on the
  child camera's propagated world pose; the existing
  `dome_follows_the_active_camera` updated for the one-frame
  propagation latency).
- `cargo fmt --all -- --check` clean; `cargo clippy --locked
  --workspace --all-targets --all-features -- -D warnings` clean;
  `cargo test --locked --workspace` — 71 result lines, 0 failures.
- Retail headless (`fnv1a64:e91e6cd4b2ae30d9`, read-only):
  `sf --headless --frames 300` → `status=pass`, `dash=11p/cam`,
  `pvs=687r/5932h/7324` — bit-identical PVS resolution; the production
  path still binds the full authored rig.
- Retail windowed (Apple M1, Metal): `--cockpit
  --spawn=-1300,63.5,250,0 --frames 90 --screenshot` renders the
  authored sky dome (clouds) 1.3 km from the origin — the exact
  scenario the finding described — with the HUD reporting the
  world-space `cam -1300.1,64.8,250.3,2,5` pose (the `--cam`
  round-trip restored), the authored dash/gear glyph/minimap intact.
  Capture is local (`/tmp/cockpit_far.png`, not committed).

## Classification

Implementation repair only — no original-behavior claim changes
(DSN-47/48/49, UNK-27/28 stand). Reading `GlobalTransform` for
world-space consumers and tightening the camera picks to
`WorldCamera3d` are implementation choices; the review's suggested
fix shape is what landed.

## Remaining open items

- F22-B stays `active` — unchanged open scope: mirror (BACKSPACE),
  occlusion handling, the chase-near/far pair split, plus the review's
  unverified legs (retail `dash=` counts on london/vpbus — not re-run
  this iteration; the sf/vpbug headless and windowed cockpit legs
  above are fresh evidence for this diff).
- F22-A remainder: AC02/AC03 map legs and HUD-2 race instruments stay
  open.
- `N`/`D` gear slots have no trigger in our sim; look magnitudes and
  `WheelFact` units are designed readings pending original recovery.
- Minor pre-existing wart unchanged: `DevOverrides::cockpit` is dead
  plumbing in the headless app (hardcodes `CameraMode::Chase`) — the
  windowed `--cockpit` path exercised above is the flag's evidence
  leg.

---

# Iteration 63 — F22-B.1 review repair: visibility-ownership + dead-camera fallback (iteration 63)

Iteration 63 on `ralph/night` (baseline `2a13e23`, F22-B.1 — external
verify green but review **failed**; seventh iteration of run
`20260925T144723`). One piece: the review's two blocking findings —
both correctness defects in the same slice.

## Task selection

The external review rejected `2a13e23` with two blocking findings;
repair precedes new feature work per the plan's regression-first
policy.

1. **`sync_dash_visibility` clobbered `Hidden` states it did not own.**
   Every non-Cockpit frame (i.e. every frame in default Chase) it wrote
   `Visibility::Visible` onto all non-`CockpitPart` direct vehicle
   children. Two ownership collisions: `BreakPartVisual` nodes are
   hidden once by `detach_breaks` and restored by `restore_rig` — the
   sweep re-showed a detached panel *attached* to the car while its
   fragment body also rendered (a permanent double-render regression of
   F05-B.3 needing no cockpit interaction); and `GlowPart` nodes are
   rewritten every frame by `update_glows`, which the sweep fought with
   ambiguous ordering (an unlit glow could render permanently).
2. **The Cockpit→Chase fallback never activated a camera.** With
   `CameraMode::Cockpit` in effect at `load_session_world`, chase and
   free cameras spawn `is_active:false`; if no `camPovCS` bound
   (dashless car, or the `None`-def arm where `spawn_dash` never ran —
   dev world, unauthored rig, or a `Cockpit` mode persisted across a
   session reload), the fallback only inserted `CameraMode::Chase` —
   nothing set `is_active` anywhere, so zero 3D cameras rendered until
   the user pressed `C` twice. The same review arm noted a menu-phase
   `C` press drifted the mode through `toggle_camera`'s `have()`-loop
   instead of being a no-op.

## What landed

- `dash.rs` — new `CockpitHidden` tag component. `sync_dash_visibility`
  now hides non-cockpit children under Cockpit mode as before but
  *tags* each node it turns `Hidden` (`GlowPart` carriers excepted —
  `update_glows` re-derives them from vehicle state every frame, so
  they never need restoring) and, in every other mode, restores
  `Visible` on **tagged children only**. A node already `Hidden` when
  the sweep reaches it is never tagged — its `Hidden` belongs to its
  owner and the sweep leaves it alone.
- `breakaway.rs` — `detach_breaks` removes `CockpitHidden` when it
  hides a node: the detach claims the `Hidden`, so leaving Cockpit
  mode can never re-show a panel that detached mid-cockpit.
- `main.rs` — `sync_dash_visibility.after(car_visual::update_glows)`:
  the split's cockpit hide deterministically wins over a lit lamp's
  `Visible` write, and outside Cockpit the split only restores its own
  tags, so the two systems cannot fight over an unlit glow.
- `session.rs` — the effective camera mode resolves **before** the
  session cameras spawn: `load_pov_cam` (new `dash.rs` helper — the
  `camPovCS` read `spawn_dash` used to do internally, now shared) is
  probed first, and `CameraMode::Cockpit` with no authored record falls
  back to `Chase` while the chase camera still spawns — so it is the
  active one. Covers `--cockpit` on a dashless car, the dev car/`None`
  arm, and a `Cockpit` mode persisted across reload. `spawn_dash`
  takes the pre-resolved `pov`; the dead post-spawn fallback is gone.
- `camera.rs` — `toggle_camera`'s `C` press returns early when zero
  marked session cameras exist (menu phase / empty world) instead of
  settling on an arbitrary step and drifting the mode.

## Gates

- `cargo test -p mm2_app --test dash` — 9/9 (+2:
  `cockpit_split_respects_other_visibility_owners` — owner-hidden nodes
  never re-shown, glows stay `update_glows`' business incl. a lit lamp
  losing inside the cockpit; `camera_cycle_without_session_cameras_is_a_no_op`).
- `cargo test -p mm2_app --test breakaway` — 12/12 (+1:
  `detached_panels_stay_hidden_across_cockpit_cycles` — real
  `detach_breaks` reclaim exercised mid-cockpit through the production
  impact pipeline).
- `cargo test -p mm2_app --test session` — 18/18 (+1:
  `cockpit_without_authored_camera_falls_back_to_an_active_chase` —
  mode held as Cockpit at load lands Chase with the chase camera
  active, exactly one camera rendering).
- `cargo fmt --all -- --check` clean; `cargo clippy --workspace
  --all-targets --all-features -- -D warnings` clean; `cargo test
  --workspace` all suites green (71 result lines, 0 failures).
- Retail headless (`fnv1a64:e91e6cd4b2ae30d9`, read-only):
  `sf --headless --frames 300` → `dash=11p/cam`, `status=pass` —
  the production path still binds the full authored rig.

## Classification

Implementation repair only — no original-behavior claim changes
(DSN-47/48/49, UNK-27/28 stand). The `CockpitHidden` ownership model
and the pre-spawn mode resolution are implementation choices; the
review's suggested fix shape ("mark hidden-by-sync and only restore
those" / "resolve the effective mode before spawning the session
cameras") is what landed.

## Remaining open items

- F22-B stays `active` — unchanged open scope: mirror (BACKSPACE),
  occlusion handling, the chase-near/far pair split, plus the review's
  unverified legs (retail `dash=` counts on london/vpbus, windowed
  cockpit captures — not re-rendered this iteration; the sf/vpbug
  headless leg above re-confirms `11p/cam` through the repaired path).
- F22-A remainder: AC02/AC03 map legs and HUD-2 race instruments stay
  open.
- `N`/`D` gear slots have no trigger in our sim; look magnitudes and
  `WheelFact` units are designed readings pending original recovery.

---

# Iteration 62 — F22-B.1 authored cockpit/dashboard view (iteration 62)

Iteration 62 on `ralph/night` (baseline `1d3b069`, F22-A.1 review
repair — external verify + review green; sixth iteration of run
`20260925T144723`). One coherent slice: the authored cockpit and
dashboard instrument view, which covers the F22-A remainder's HUD-1
instrument leg by *binding the original dashboard content* rather than
drawing over the dev telemetry line.

## Task selection

No failing gate or review finding to repair. The review's open items
named the F22-A remainder (HUD-1/HUD-2 instruments, AC02/AC03 map
legs) as next. Discovery showed every stock `vp*` ships a complete
authored dash rig — `_dash.pkg` geometry, `_dash.asnode` gauge
calibration, `_dash.campovcs` camera — so the cockpit/instrument slice
(F22-B.1, opening F22-B) subsumes the HUD-1 instrument leg with
authored content. The AC02 pixel-alignment and AC03 live-marker legs
of F22-A stay open by plan.

## What landed

- `mm2_formats::dash` — `DashSpec` (`_dash.asnode`: `DashPos`,
  `RoofPos`, `WheelPos`, per-gauge `*Offset`/`*PivotOffset`,
  `*RotMin/Max` sweep radians, `WheelFact`) and `PovCamSpec`
  (`_dash.campovcs`: `Offset`/`ReverseOffset`/`TrackTo`/`Pitch`, FOV,
  near/far), sparse-record tolerant, wrong-block rejecting.
- `VehicleConfig::top_speed_mps` — authored `vehCarSim.Trans.High`
  (mph→m/s); dev cars/trailers `None`, presentation never fabricates.
- `mm2_app::dash` — `spawn_dash` loads the three records
  independently (camera needs `camPovCS`; cluster needs asnode+pkg),
  spawns the authored parts as vehicle children via the shared
  `build_model`/`group_mesh`/`group_material` path, and emits a
  `DashReport` (`dash=<n>p/<cam|nocam>` smoke field). `drive_dash`
  drives needles off `VehicleState`/`VehicleDamage` using the authored
  sweeps (speedo full-scale `top_speed_mps`, tach redline), rolls the
  wheel by `steer_angle/lock × WheelFact`, and — the recovered
  mechanism — the `gear_indicator` quad's paint-job table is
  repurposed as gear slots (shader 4 names `R`,`N`,`One`…`Six`,`D` on
  every sampled stock dash), so the engaged gear swaps the quad's
  material (`GearGlyph`) rather than sliding a strip; an earlier
  slide-strip reading was falsified by the retail capture and
  retracted. `sync_dash_visibility` flips direct vehicle children
  between exterior and cockpit sets (descendants propagate).
- `camera.rs` — `CameraMode::Cockpit`; `C` cycles
  Chase→Cockpit→Free marker-driven (`ChaseCamera`/`CockpitCamera`/
  `FreeCamera`), skipping a mode whose camera never spawned and never
  writing `is_active` on unmarked cameras — the A.1 review's
  map-camera blink wart is repaired. `V` is the dash toggle (the
  documented `D` conflicts with enhanced WASD steering — DSN-48);
  numpad 4/6/2/8 drive the authored-anchored look, `Numpad2` swaps in
  `ReverseOffset` (magnitudes designed, DSN-49). `--cockpit` selects
  it at spawn, falling back to chase when no authored camera exists.
- Session integration — the rig spawns/teardowns with the player
  vehicle; `DashReport` is a session resource removed on unload.
- `mm2-inspect` gained `tex --ascii`/`--ppm` and `pkg --verts` —
  the inspection legs that recovered the gear-slot mechanism.

## Evidence

- Parsers: 4 unit tests (retail-shaped records, sparse tolerance,
  wrong-block rejection). Runtime: `tests/dash.rs` 7 tests — absence
  policy, needle/wheel/gear drive, visibility split, marker-driven
  cycle incl. absent-cockpit skip, cockpit binding, numpad look.
- Retail headless (`fnv1a64:e91e6cd4b2ae30d9`, read-only): sf `vpbug`
  and london `vpbug` + sf `vpbus` all report `dash=11p/cam` —
  11 mesh parts bound plus the authored cockpit camera, on a second
  archetype and both cities.
- Retail windowed: `--cockpit --frames 90 --screenshot` renders the
  authored interior (wheel left-of-centre, readable speedo, fascia,
  roof card, exterior hidden, inset map alive); the gear window now
  reads `1` under `D1` (was `R` before the material-swap fix).
- Gates: `cargo fmt --all -- --check` clean; `cargo clippy --workspace
  --all-targets --all-features -- -D warnings` clean; `cargo test
  --workspace` all suites green.

## Classification

Instrument bindings verified against authored data; composition
(`*Offset` semantics) and `N`/`D` slot triggers remain designed/
unrecovered — recorded as DSN-47/48/49 and UNK-27/28 in
`docs/original-rules.md`; HUD-1/HUD-3 rows updated with the asset
evidence and deviations.

## Remaining open items

- F22-B stays active: mirror (BACKSPACE), occlusion handling, and the
  chase-near/far pair split (Free currently occupies that slot —
  documented deviation) remain.
- F22-A remainder: AC02/AC03 map legs and HUD-2 race instruments
  (checkpoint list/laps/place/stopwatch over the dev line) stay open.
- `N`/`D` gear slots have no trigger in our sim (no neutral state;
  `D` reachable only via the table clamp past gear 6).
- Cockpit look glance magnitudes and `WheelFact` units are designed
  readings pending original recovery.

---

# Last iteration — F22-A.1 review repair: pause-map input leak (iteration 61)

Iteration 61 on `ralph/night` (baseline `4ea8638`, F22-A.1 — external
verify green but review **failed**; fifth iteration of run
`20260925T144723`). One piece: the F22-A.1 review's single blocking
finding plus its two minor same-class items.

## Task selection

The external review rejected `4ea8638` with one blocking finding: while
`Paused` with `HudMap.fullscreen`, the visually-hidden pause menu stayed
input-live — `pause_input` gated on `phase == Paused` alone, so
`Enter`/`Space` activated the invisibly-focused row (Resume → `Playing`
leaves the order-1 map camera covering live gameplay with no `Playing`
input path that closes it; an invisibly-drifted focus could fire
Restart/Quit), and Backspace/gamepad East/Start resumed through
`MenuCommand::Back` into the same stuck state. The same missing exit
invariant let `dev_pause_map_once` strand `fullscreen` on a
non-pausable authority (`drive_session` rejects the intent, the map
stays up over `Playing`). Repair precedes new feature work per the
plan's regression-first policy.

## What landed

- `pause_input` (`mm2_app::pause`) early-returns while a non-stale
  `HudMap.fullscreen` holds — the map *replaces* the overlay, so the
  hidden rows take no input; `hudmap_input` (scheduled ahead) still
  owns the map's Q/Esc close.
- `hudmap_input`'s exit invariant tightened: `fullscreen` survives only
  in `Paused`, or while a `control.pause` intent is still queued in the
  same update (the flag is set alongside the intent; `drive_session`
  consumes it later in the update). Any other state — `Playing` after a
  rejected intent or a resume, teardown phases — clears it next frame,
  so the map camera can never strand over live gameplay.
- `dev_pause_map_once` gained the same MP-6 `allows_pause` gate the Q
  key carries plus a staleness check — a non-pausable authority never
  fires it. `dev_pause_once` gained the same gate (the
  `SessionControl::pause` contract already documents the intent as
  produced only for a pausable authority).
- `tests/session.rs`'s harness now schedules `hudmap_input` and
  `dev_pause_map_once` in the same slots the binary uses, and gained
  +3 regression tests (14 → 17):
  - `pause_map_owns_the_keys_while_the_menu_is_hidden` — Q opens the
    pause map (`Paused`, `fullscreen`, zero overlay rows), then
    arrows/W/Enter/Space/Backspace are all inert (phase stays `Paused`,
    `fullscreen` holds, `PauseMenu.focus` stays 0, no intent leaks);
    Q closes straight to `Playing`.
  - `fullscreen_map_clears_itself_outside_pause` — `fullscreen` up on a
    `Playing` session with no pending intent self-clears next update.
  - `pause_map_dev_override_respects_pause_authority` — `--pause-map`
    under `SessionAuthority::Host` never fires: `Playing` holds,
    `fullscreen` stays false, no pause intent queued.

## Gates

`cargo fmt --all -- --check` clean; `cargo clippy --locked --workspace
--all-targets --all-features -- -D warnings` clean; `cargo test
--locked --workspace` — 70 suites, 0 failures (tests/session.rs 17/17,
tests/hudmap.rs 4/4 unchanged green).

## Classification

Implementation repair only — no original-behavior claim changes
(DSN-46/UNK-26 stand as recorded). The review's minor doc note is also
corrected in the iteration-60 entry: `--pause-map` is deliberately
*out of* `record_eligibility` (render-only, consistent with
`--pause`/`--cam`), not "record-ineligible".

## Remaining open items

- F22-A stays `active` — unchanged open scope: HUD-1/HUD-2 race
  instruments, AC02 map-pixel correctness, AC03 live marker-binding
  verification, and gamepad bindings for the map controls (none exist —
  that is F23 rebind scope).
- The review's other verification gaps remain open: the windowed
  captures were not re-rendered, and no retail leg was re-run this
  iteration — nothing in this diff touches tile/marker binding, so the
  `4ea8638` retail evidence stands unmodified.

---

# Iteration 60 — F22-A.1 authored in-race HUD minimap (iteration 60)

Iteration 60 on `ralph/night` (baseline `e452468`, F14-C.1 — external
verify + review green at `e452468`; fourth iteration of run
`20260925T144723`). One piece: F22-A's first child — the authored
in-race HUD minimap on original data.

## Task selection

No failing gate or open review finding to repair — the F14-C.1
review passed with verification gaps only (all disclosed, none
blocking). Auditing the plan's ready candidates found F22-A's deps
(F01-A/F02-B/F11-B) satisfied, and the minimap slice proved
unusually well-evidenced: HUD-4 is a documented rule (help:
"Displaying a Map of the City"), the retail exe carries the
`mmHudMap` class with `hudmap_%s.pkg`/`hudmap_{square,tri}`/
`IOID_MAP`/`MAPORIENT`/`FMAP` references, mm2hook (R4) recovers the
class's member layout, and the authored payload is fully present —
`geometry/hudmap_{sf,london}.pkg` tiles authored in *world-space XZ*
(spanning the city extent → world→map alignment is identity),
flat-XZ marker meshes with authored `*_DOT` paints, and
`tune/{sf,london}.mmhudmap` carrying the layout/zoom/icon-scale/
ocean-color fields. Chosen over F19-A (pedestrians), whose formats
(`.anim`/`.skel`/`.mod`) have no parsers yet — a heavier reverse-
engineering slice. F22-A's parent stays `active`: the race-HUD
instrument remainder is open scope.

## What landed

- `mm2_formats::hudmap` — `HudMapSpec` parser over the shared tune
  grammar (spaced field names tokenize a qualifier word into the
  values; `Approach Rate`/`Ocean Color` consume it). +5 tests.
- `mm2_game::hudmap` — `HudMap` session resource:
  `MapView::{Inset,Large,Off}` (TAB's cycle), `MapOrientation`,
  authored zoom pair eased at the authored `Approach` rate,
  fullscreen flag, generation staleness. +6 tests.
- `mm2_app::hudmap` — session-owned spawn of the authored tiles and
  marker pool under a dedicated orthographic `Camera3d` on its own
  `RenderLayers`; per-frame binding of player/opponent positions,
  checkpoint cleared-state colors, `navigation_target` highlight,
  unlock-gated finish marker; `hudmap_input` for TAB/E/F/Q; the
  `map=` smoke detail. Q opens the fullscreen map and pauses only
  under `SessionAuthority::allows_pause`, replacing (not overlaying)
  the pause menu; `E`/`Q` stay free-camera-owned in that mode.
- `WorldCamera3d` filter alias — the map camera is an *active*
  `Camera3d`, so every "the active camera" pick now excludes it:
  `audio_listener` (fixes the multiple-`SpatialListener` warnings the
  first screenshot run emitted), `apply_city_pvs`, `drive_sky_dome`,
  `retarget_hud`, `update_hud`/`active_cam_pose`/`screenshot_input`,
  and damage billboards.
- `--pause-map` dev override for the fullscreen-map smoke leg
  (render-only — deliberately out of `record_eligibility`, like
  `--pause`/`--cam`; corrected from "record-ineligible" per the
  external review's doc note).
- `MaterialCache::unlit_copy` — marker paints render unlit.

## Evidence

- Synthetic: `tests/hudmap.rs` +4 — authored bind on a synthetic
  install (hand-built PKG3 tile/marker bytes), `absent:` reporting
  on a city with no map content, dev-world records carry no `map=`
  field, fullscreen pause-map record.
- Retail headless (`fnv1a64:e91e6cd4b2ae30d9`, Apple M1):
  `sf --city sf` → `map=inset/north/z1195/hudmap_sf.pkg/6t/1m`;
  `london` → `4t/1m`; `sf circuit:0 --bot` → `14m` (player + 4
  opponents + 9 gate dots); `sf --pause-map` → `phase=paused`,
  `map=…/z1574/fs/…` (mid-ease toward the authored 1581 extent).
- Windowed screenshots: `--city sf --frames 90 --screenshot`
  renders the authored SF tiles inset bottom-right; `--pause-map`
  renders the fullscreen map over the paused world. Captures local
  (`/tmp/map_sf.png`, `/tmp/map_fs_sf.png`).

## Gates

`cargo fmt --all -- --check` clean; `cargo clippy --workspace
--all-targets --all-features -- -D warnings` clean; `cargo test
--workspace` all suites green.

## Classification

Authored-content consumption with documented controls — the tune
parse, world-space tiles, marker models and TAB/E/F/Q behavior are
verified/documented (HUD-4). Presentation readings are designed
(DSN-46): the second inset view's layout, `ZoomIn==0` start
semantics, icon-scale interpolation, palette→marker bindings and
the camera-yaw rotation reading; the mmHudMap Cull/Draw bodies stay
unrecovered (UNK-26). No complete-HUD-parity claim: F22-A's
instrument remainder is open.

## Remaining open items

- F22-A stays `active`: HUD-1/HUD-2 dashboard/race instruments over
  the dev line; AC02/AC03 coordinate/marker-correctness legs beyond
  the smoke checks (authored-position → map-pixel verification) are
  the natural next slices.
- The named remainder list stands unchanged: F05-B (UNK-13/F27/
  F25+), F11-C (review judgment), F13-C (original-fidelity
  comparison), F17-B (needs F17-C's mode), F14-C (F15-B-gated
  completability + UNK-11), F18-A (→ F18-B/C), F17-A AC03
  (interactive), F16-C AC01 (interactive finish), F10-B AC03
  (manual), F07-B (no output device).
- Cosmetic: the dev `--pause-map` fires on the first `Playing`
  frame, before `VehicleTelemetry` attaches, so the HUD shows its
  `loading…` line under that leg — a dev-flag artifact only; real
  Q pauses mid-drive with telemetry present.

---

# Iteration 59 — F14-C.1 mid-race restart + authored-edge Circuit legs (iteration 59)

Iteration 59 on `ralph/night` (baseline `424cca6`, F14-B.2 — external
verify + review green at `424cca6`; third iteration of run
`20260925T144723`). One piece: F14-C's unblocked evidence scope — a
mid-race Ordered restart on retail circuits plus the authored-data
finish-line-spawn edge leg.

## Task selection

No failing gate or open review finding to repair — the F14-B.2
review passed with verification gaps only (all disclosed, none
blocking). The plan's named next slice is F14-C's unblocked legs:
catalog/multi-lap/opponent/restart evidence on retail content —
its traversal-stall completability claims stay gated on F15-B's
research. Auditing the named scope found two real gaps in the
existing evidence: every restart leg fired at tick ~0 (`--restart`
queues the intent on the first `Playing` frame), so nothing had
shown the teardown/rebuild resetting *banked* race progress; and
the B.2 edge tests are all synthetic — no leg had exercised an
exploit-negative case against an authored gate volume. A third
gap: no leg had driven a retail Ordered event to `Results` through
the production `advance_race` at all (matrices cap at 12000
frames; the scripted driver stalls before finishing most
circuits).

## What landed

- `DevOverrides::restart_at: Option<u64>` (mm2_game `config.rs`) +
  `record_eligibility` arm (`Ineligible::DevOverride("restart-at")`)
  + CLI `--restart-at <ticks>` (120 Hz session-clock units — the
  record's `ticks=` field) + `dev_restart_at` system scheduled next
  to `dev_restart_once` in the windowed and headless `Update`
  chains, ahead of `drive_session`. Same one-shot latch, same
  production `Unloading → Menu → begin` path — the deferral is the
  only difference. Tests: +3 `tests/smoke.rs`
  (`restart_at_defers_the_restart_to_the_configured_tick` — gen-2
  `ticks=` proves the restart fired mid-run, not at spawn;
  `restart_at_fires_once_not_once_per_generation` — gen-2 crossing
  the threshold does not refire; `restart_at_beyond_the_run_never_
  fires`) + the `record_eligibility` arm in `tests/progression.rs`.
- Retail legs (`fnv1a64:e91e6cd4b2ae30d9`, Apple M1, headless):
  - Control: `sf circuit:0 --bot --frames 4000` → `cp=4/9 lap=1/3`
    at `ticks=7640` — the banked-progress baseline the restart
    interrupts.
  - `sf circuit:0 --bot --restart-at 7200 --frames 12000` → `rs=1`,
    gen-2 `ticks=16076` re-racing `cp=2/9 lap=2/3 pos=1/5`,
    `dup=0`, roster respawned once — the teardown at ~60 s banked
    playing time destroyed gen-1's 4 gates and rebuilt a fresh,
    separately-counted generation.
  - `london circuit:0 --bot --restart-at 7200 --frames 12000` →
    `rs=1`, gen-2 `ticks=16076`, `cp=1/6 lap=3/3`, and **a
    generation-scoped opponent finish**: `vpcoop` slot 1 resolved
    `6c/3l/F` minting `results=1` while the local raced lap 3 —
    the ledger is generation-scoped (gen-1's banking does not
    contaminate it) and the session stays `Running` while the
    field races on.
  - `sf circuit:0 --parked --spawn=-1689.286,44.974,-62.809
    --frames 3600` → the finish-line-spawn edge on authored
    geometry: the car dwells inside circuit0's closing-gate
    cylinder (waypoint row 0, radius 11, +0.5 lift) the entire run
    (`final` ~2 m from spawn, `peak=0.1`) and banks `cp=0/9
    lap=1/3 results=0` while the field races a lap — dwelling
    inside the not-yet-`next` volume grants nothing.
  - `sf circuit:0 --finish --frames 4000` → `phase=results`,
    `cp=9/9 lap=3/3 outcome=finished place=1`, `results=1` at
    `ticks=53`: the dev sweeper drove the full 9-gate × 3-lap
    Ordered sequence through production `advance_race` — the
    lifted row-0 start-line copy armed and banked as the closing
    gate each lap on authored data; the 4 opponents stayed
    unresolved (`opp=0/4`, `still racing` under DSN-11). Dev-flag
    run — record-ineligible by construction (`finish` is in
    `record_eligibility`).

## Gates

`cargo test -p mm2_app --test smoke` +3 green;
`cargo test -p mm2_game --test progression` green. Full
fmt/clippy/test gate results in the commit.

## Classification

The `--restart-at` plumbing is an evidence-runner capability
(implementation choice; `record-ineligible` like `--restart`/
`--finish`/`--spawn`). The retail legs are original-content
runtime evidence: the Ordered lap model and teardown semantics
stay designed/UNK-11 — what they demonstrate is the implementation
behaving to its contract on authored geometry, not that the
original game did the same.

## Remaining open items

- F14-C stays open (deps F15-B for the completability claims):
  traversal-stall legs (london-2/5/6/9, sf-7/9) remain the F15-B
  controller class; original-fidelity of Ordered accounting is
  UNK-11; the `--finish` leg is a dev-swept resolution, not a
  driven completion — no local circuit finish exists yet.
- The whole named remainder list from iterations 56–58 stands
  unchanged: F05-B (UNK-13/F27/F25+), F11-C (promotion = external
  review judgment), F13-C (original-fidelity comparison), F17-B
  (needs F17-C's mode), F18-A (→ F18-B/C), F17-A AC03
  (interactive), F16-C AC01 (interactive finish), F10-B AC03
  (manual), F07-B (no output device).

---

# Iteration 58 — F14-B.2 Ordered lap-validation edge legs + F14-B promotion (iteration 58)

Iteration 58 on `ralph/night` (baseline `6f62159`, F14-A.6 — external
verify + review green at `6f62159`; second iteration of run
`20260925T144723`). One piece: the F14-B parent's remaining
implementation scope — the Ordered edge cases from the spec's edge
list, which every existing negative leg covered only under
`AnyOrder`.

## Task selection

No failing gate or open review finding to repair — the F14-A.6
external review passed with verification gaps only (all disclosed
residuals stay open under F15-B/UNK-11 as recorded). With F14-A
closed at `6f62159`, F14-B became the plan's next ready slice. Its
named scope audited against the tree: B.1's live running order
landed long ago (`live_order`/`pos=`/DSN-13), participant ranking
came from F13-B.1's standings, the HUD already carries HUD-2's
Circuit instrument set (`lap x/y`, checkpoint count, place,
stopwatch), and the opponent hooks landed across F14-A.3–.5. What
remained was lap-validation edge coverage under `Ordered` — the
spec's "finish-line spawn; overlapping start/finish volumes;
skipped gate; last-lap tie; DNF participant; reset on finish" list.

## What landed

- `tests/race.rs` 36 → 43 (+7), all through the production
  `advance_race`/`reanchor_teleported_participants` path on a
  synthetic closed course in the retail shape (course gates in
  authored order + the lifted start-line copy last, WPT-2):
  - `ordered_skipped_gate_clears_nothing_until_revisited_in_order`
    — sweeping gate 1 while gate 0 is owed banks nothing, not even
    a `crossings` tick; the skipped gate must be re-visited.
  - `ordered_finish_line_is_inert_until_it_is_next` — repeated
    both-direction line sweeps before its turn bank no lap (AC02's
    "repeated finish hits" + "backward" legs under Ordered).
  - `ordered_spawn_inside_the_line_grants_nothing` — staged dead
    centre on the closing gate, dwell + movement inside the volume
    banks nothing (the "finish-line spawn" edge).
  - `ordered_overlapping_closing_gate_banks_one_lap_once` — one
    segment through an overlapping last-gate/line pair banks the
    lap once; post-finish re-sweeps mint nothing ("overlapping
    start/finish volumes").
  - `ordered_last_lap_tie_records_both_deterministically` — two
    shared-clock final-lap finishes both record; standings break
    the tie by `PlayerId`.
  - `ordered_reset_over_the_line_still_owes_the_crossing` — the
    production `ResetVehicle` jump sweeping the closing gate banks
    nothing; the line must be physically re-crossed ("reset on
    finish").
  - `an_unresolved_participant_does_not_block_the_local_result` —
    a never-resolving opponent keeps the race `Running`, but the
    local finish still reaches `Results` with exactly the local
    result banked and the drifter unplaced ("DNF participant" +
    req 4's bounded result handling).
- F14-B promoted to `implemented` (candidate): all four named items
  now carry evidence — lap validation (the `Ordered` swept-sequence
  contract + these edge legs), participant ranking (B.1 +
  F13-B.1), HUD instruments (HUD-2's Circuit set in `update_hud`),
  opponent hooks (roster spawn/drive + DSN-45 route-bound progress,
  retail 60-leg Pro matrix). F14-C's catalog/exploit legs stay
  open and still dep on research-gated F15-B.

## Gates

`cargo test --locked -p mm2_app --test race` — 43/43 green (all 7
new legs pass on the unchanged `Ordered` contract; the slice is
evidence-only, no production delta). `cargo fmt --all -- --check`
clean; clippy/test full-suite results below in the Gates section of
the commit (run at checkpoint).

## Classification

Synthetic integration evidence only — the Ordered edge legs drive
the production race driver with deterministic `Position` segments.
No retail-data, rendered or audio legs this iteration; no
original-fidelity claim (the Ordered accounting model stays
designed/UNK-11).

## Remaining open items

- F14-C stays queued: catalog validation + multi-lap/opponent/
  restart evidence on retail content; its F15-B dep is still
  research-gated (`unkFlag`/`cornerBrakingThreshold`/
  `weirdPathfinding` semantics unverified; traversal stalls on
  london-2/5 + sf-7 pack are the F15-B controller class).
- The whole named remainder list from iterations 56–57 stands
  unchanged: F05-B (UNK-13/F27/F25+), F11-C (promotion = external
  review judgment), F13-C (original-fidelity comparison), F17-B
  (needs F17-C's mode), F18-A (→ F18-B/C), F17-A AC03 (interactive),
  F16-C AC01 (interactive finish), F10-B AC03 (manual), F07-B (no
  output device).

---

# Iteration 57 — F14-A.6 circuit restart leg + AC promotion (iteration 57)

Iteration 57 on `ralph/night` (baseline `86bae3e`, F14-A.4 repair —
external verify + review green at `6ea22d7`; this is the first
iteration of run `20260925T144723`, whose counter restarted at 001).
Two pieces: preserve the interrupted iteration-56 work that sat
uncommitted in the tree, then the F14-A remainder's last named
open item — the AC04/AC05/AC06 promotion.

## Task selection

Iteration 56 died mid-handoff: the doc-repair commit `86bae3e`
landed but the completed F14-A.5 evidence write-up (LAST_ITERATION,
PLAN, race-coverage hold tables) was never committed. Committed
verbatim as `204373d` — the write-up was complete and internally
consistent; nothing was regenerated.

No failing gate or open review finding remained after `86bae3e`
(the A.4 review passed with verification gaps — all addressed).
The plan's named top candidate is the F14-A remainder: with all
three Professional driver legs banked, the only un-evidenced AC was
AC05's *Ordered* leg — every existing restart test covered
AnyOrder/checkpoint events, nothing covered a lapped, rostered
circuit's counters (laps, `RouteGateLine` high-waters, chase
indices). Every other candidate stays blocked as iteration 56
recorded (F05-B UNK-13 / F27 / F25+, F17-B needs F17-C, F15-B
research-gated, F16-C interactive finish, F11-C review judgment,
F13-C original-fidelity comparison, F18-A → F18-B/C scope, F07-B
no authored sample/output device, F10-B manual player-hit leg,
F17-A AC03 interactive).

## What landed

- `204373d` — the preserved F14-A.5 docs (see the entry below).
- `restart_restores_the_circuit_grid_counters_and_objects`
  (`tests/opponents.rs` 42 → 43): a synthetic `mmcircuitdata` event
  (NumLaps 2, 2-car roster, authored `cir0` grid, closed `.opp`
  loops) rides the real `load_session_world` → `opponent_drive` →
  `advance_race` path. The field banks mid-race progress
  (`RacePhase::Running`, gates/lap/route credit non-zero), then
  `SessionControl.restart` drives the production teardown/reload
  and generation 2 asserts each AC05 clause: `RaceState` re-minted
  (`generation=2`, `Countdown`, `clock=0`); the lineup respawned
  exactly once on authored slots `index+1` under `SessionEntity(2)`;
  `RaceProgress` zeroed (lap/next/cleared/crossings/route_clears);
  `OpponentDriver.next` restored to its spawn-time value per roster
  index; each `RouteGateLine.arc_high` equal to a fresh `bind` at
  the respawned pose; the player back on authored slot 0; one
  marker per Ordered gate; `standings_in(2)` empty.
- F14-A promotion recorded honestly: parent row → `implemented`
  (candidate), A.5 + A.6 table rows, the race-coverage AC mapping
  now lists all six ACs with their evidence, and the residuals the
  task does not close stay named (`[Exceptions]`/density consumers
  F15/F10; traversal stalls F15-B; original-fidelity comparisons
  unverified — matrices are smoke records, not completability).

## Gates

`cargo fmt --all -- --check` clean; `cargo clippy --locked
--workspace --all-targets --all-features -- -D warnings` clean;
`cargo test --locked --workspace` all suites green (69 test
binaries, including `tests/opponents.rs` 43/43). No production
code changed — one test file plus docs.

## Classification

Synthetic integration evidence only (the AC05 leg exercises the
production session/race/teardown systems on a synthetic install).
No retail-data, rendered or audio legs this iteration; no
original-fidelity claim.

## Remaining open items

- F14-A is `implemented` pending external review; the F15-B/F10
  residuals it names are other tasks' scope, not closure blockers.
- The whole named remainder list from iteration 56 stands
  unchanged: every candidate is research-gated, mode-blocked,
  manual/interactive or needs an output device. Next pick should
  re-derive from the selection-policy list rather than looping on
  F14.

---

# Iteration 56 — F14-A.5 Professional Circuit hold legs

Iteration 56 on `ralph/night` (baseline `6ea22d7`, F14-A.4 —
external verify + review green). Two pieces: the review's flagged
doc findings (re-verified; the confirmed subset repaired) and the
F14-A remainder's last named driver leg — the Professional
hold-driver legs, the F13-C.5 pattern applied to the Circuit
catalog.

## Task selection

The A.4 review passed with verification gaps; the actionable ones
were checked against the retained logs before any edit:

1. Two lap cells flagged as miscounts — **re-verified accurate**:
   london-3-parked's opps row reads `2l,2l,2l,1l,2l,2l` = 5/6 and
   sf-3-bot reads `1l,1l,2l,2l` = 2/4, exactly as published. Left
   unchanged; the reviewer's "actual" values do not match the
   retained evidence in `/tmp/mm2-circuit-matrix-pro/`.
2. Anomaly disclosure thinner than the F13-C.5 precedent —
   confirmed, repaired: sf-5's scripted leg `rcv=0w/132f` named
   alongside parked's `222f`; end-pose `wheels=0/4` (sf-0-bot),
   `3/4` (london-5-bot), `1/4` (sf-7-parked) disclosed; lesser
   spawn-adjacent `rcv` churn covered.
3. The `0d`-phrasing — confirmed, tightened: six events are `0d`,
   the three authored-miss residuals (london-2/5, sf-7) plus three
   all-physical stalls (london-7, sf-2, sf-9) the sentence omitted.
4. Repair commit `86bae3e`.

Then the highest-value ready slice: the Professional Circuit
hold-driver legs — the one driver leg the A.4 handoff named open,
mirroring F13-C.5's Checkpoint hold legs. All other candidates
stayed unchanged-blocked (F05-B UNK-13 / F27 / F25+, F17-B needs
F17-C, F15-B research-gated, F16-C interactive finish, F11-C review
judgment, F13-C original-fidelity comparison, F18-A → F18-B/C
scope, F07-B no authored sample/output device, F10-B manual
player-hit leg, F17-A deferred consumers).

## What landed

- Doc repair (commit `86bae3e`): the confirmed findings above; the
  two flagged cells stand on re-verification.
- F14-A.5 (docs + evidence only — no code change): 20 legs = 20
  cataloged Circuit events × the blind `Hold` driver (no driver
  flag — settles ≤2 s, then full throttle, no steering) at `--pro
  --frames 12000`, retail `fnv1a64:e91e6cd4b2ae30d9`, every log
  stamping `commit=6ea22d7` (code-identical to `86bae3e`; the delta
  is docs-only). Published in `docs/race-coverage.md`; raw logs in
  `/tmp/mm2-circuit-matrix-pro-hold/` (uncommitted per the
  large-capture rule).

## Evidence

**20/20 `rc=0 status=pass`, `dup=0`** (~27 min wall, ≤102 s/leg).
The Professional Circuit matrix is complete at 60 legs = 20 events
× scripted + parked + hold. Headline outcomes:

- **Second Professional finish**: london-0 again — `vpcoop2k`
  slot 2 `6c/4l/F`, `results=1`, `phase=playing`, `pos=8/8` — a
  *different* finisher than the scripted leg's slot 0; the
  once-only ledger + local-races-on semantics hold under the third
  driver. The hold car never resolves anywhere.
- Multi-lap on 7/20 hold legs (london-0/1/3, sf-0/1/3/5) vs 10
  under scripted/parked; london-0's whole field laps again (15
  completions).
- Spawn-edge classes driver-independent: london-8 `58f` identical
  on all three drivers, sf-4 56f/58f/58f; sf-5's loop scales with
  the driver (132f/222f/100f).
- Blind-driver hazards: london-2 `rcv=328w/5f` — the worst Thames
  loop of the matrix (240w scripted, 223w parked); london-6 `185w`;
  london-4 `dropped=493` — worst Circuit leg to date (prior:
  amateur sf-4's 261); sf-8 `peak=64.8 m/s` / `moved` 607 m;
  sf-2 `wheels=3/4` end pose.
- Traversal residuals unchanged: london-2/5 and the sf-7 pack
  `0d`, zero laps under every driver.

## Gates

`cargo fmt --all -- --check` clean; `cargo clippy --locked
--workspace --all-targets --all-features -- -D warnings` clean;
`cargo test --locked --workspace` — all 69 suites green (run before
the repair commit; the iteration's changes are docs-only).

## Classification

Runtime matrix is original-content validation evidence
(fingerprinted install, authored `.aimap_p` data) — `status=pass`
legs are smoke records, not completability claims. No
original-fidelity assertion; no rendered/manual leg this iteration.

## Remaining open items

- F14-A stays active pending external review; AC04/AC05/AC06
  promotion stays open. The Pro driver-leg matrix is now complete
  (60 legs) — the hold-legs gap the A.4 handoff named is closed.
- Traversal-skill residuals at Pro: london-2/5, sf-7 pack, london-6
  gate-1, sf-9 gate-4 — F15-B controller class.
- sf-5/london-8/sf-4 spawn-edge fall loops and london-2/6 Thames
  punt collateral remain disclosed anomalies (driver-independent).

---

# Iteration 55 — F14-A.4 Professional Circuit matrix

Iteration 55 on `ralph/night` (baseline `4a1db5a`, F14-A.3 — external
verify + review green). Two pieces: the review's flagged doc repair
(the F14-A.3 test-count claim) and the F14-A remainder's named-open
Professional leg — the same scripted+parked matrix shape the Amateur
catalog ran in A.2 and the Checkpoint catalog ran in F13-C.4.

## Task selection

The A.3 review passed with one repairable finding: the handoff docs
claimed `tests/race.rs 34 → 35 (+9)` where the verified diff is
28 → 35 (+7 in `tests/race.rs`, +2 in `tests/opponents.rs` — 9 only
across both suites). Repaired first (commit `e43ee20`), then the
highest-value ready slice: the Professional Circuit legs the A.2/3
reviews and `docs/race-coverage.md` explicitly named open. Every
other candidate stayed unchanged-blocked (F05-B UNK-13 / F27 / F25+,
F17-B needs F17-C, F15-B research-gated, F16-C interactive finish,
F11-C review judgment, F13-C original-fidelity comparison, F18-A →
F18-B/C scope, F07-B no authored sample/output device, F10-B manual
player-hit leg, F17-A deferred consumers).

## What landed

- Doc repair (commit `e43ee20`): LAST_ITERATION.md and PLAN.md now
  state `tests/race.rs` 28 → 35 (+7) and `tests/opponents.rs` 40 → 42
  (+2), nine new tests across both suites — matching the external
  review's verified numbers.
- F14-A.4 (docs + evidence only — no code change): the Professional
  Circuit matrix, 40 legs = 20 cataloged events × {scripted `--bot`,
  parked control} at `--pro --frames 12000`, retail
  `fnv1a64:e91e6cd4b2ae30d9`, binary built at `e43ee20` (docs-only
  delta on `4a1db5a`; all 40 logs stamp it). Published in
  `docs/race-coverage.md`'s new Professional section; raw logs in
  `/tmp/mm2-circuit-matrix-pro/` (uncommitted per the large-capture
  rule).

## Evidence

**40/40 `rc=0 status=pass`, `dup=0` on every leg** (~54 min wall
total, ≤106 s/leg). Headline outcomes:

- Pro measurably selects authored `.aimap_p` rosters + parameter
  blocks: `diff=professional` on every leg, distinct lineups
  (london-0 fields 7 `vpcoop2k` vs amateur's 7 `vpcoop`), distinct
  `NumLaps` (london `*/4` vs `*/3` except c1/c2/c9 `*/2`; sf `*/4`
  except c8/c9 `*/2`).
- **First Professional circuit finish**: london-0 scripted leg — a
  `vpcoop2k` banks all 4 laps (`6c/4l/F`, `results=1`, `opp=1/7`)
  while the local raced on (`phase=playing` — remote resolution ends
  nothing). The whole 7-car field laps there (16/18 completions
  across the two legs; four cars reached lap 4).
- Multi-lap churn on 10/20 events: sf-0 all-four opponents complete
  lap 0 scripted; sf-1 5–6/7; london-1 3–4/6; london-3 5/6; sf-5 one
  car completes *two* laps (parked). Scripted driver never finishes
  at Pro (best london-0 `lap3/4`) — authored Pro is measurably
  harder, consistent with the checkpoint matrix.
- First catalog-wide run with DSN-45 route credit live: `/Nd` on
  14/20 events; sf-1 is the extreme (5–6/7 opponents bank lap 0 with
  `8–10d` of 10 gates — the driven path threads almost no cylinders,
  progress nearly all route-derived — disclosed, not smoothed).
  `0d` exactly where fields never reach the binds: london-2 (g0
  ~700 m), london-5 (g2 ~1050 m), sf-7 pack (g5 ~985 m) — traversal
  residuals unchanged, the model cannot invent progress.
- Parked control honest: `cp=0` all 20 legs, `moved` ≤53 m contact
  displacement, `peak` ≤15.6 m/s.
- Anomalies disclosed per event: london-8 `58f` on both legs,
  sf-5 parked `rcv=222f` (new spawn-edge fall loop), london-6 Thames
  punt on both legs (`243w`/`239w`), sf-9 the matrix's only
  `dropped` (105/112) + `peak` 35.1 m/s.

## Gates

`cargo fmt --all -- --check` clean; `cargo clippy --locked
--workspace --all-targets --all-features -- -D warnings` clean;
`cargo test --locked --workspace` — all 69 suites green (run on the
doc-repair commit before the legs; the iteration's own changes are
docs-only).

## Classification

Runtime matrix is original-content validation evidence
(fingerprinted install, authored `.aimap_p` data) — `status=pass`
legs are smoke records, not completability claims. No
original-fidelity assertion: roster/lap-count differences are
authored-difficulty measurements, and original pacing/AI competence
stays unverified. No rendered/manual leg this iteration.

## Remaining open items

- F14-A stays active: Professional hold-driver legs (the F13-C.5
  pattern) are the remaining driver leg; AC04/AC05/AC06 promotion
  stays open pending external review of this evidence.
- Traversal-skill residuals at Pro: london-2/5, sf-7 pack, london-6
  gate-1, sf-9 gate-4 — F15-B controller class.
- sf-5 parked `222f` joins the spawn-edge fall-loop class
  (london-4-amateur's `209f`); london-6's Thames punt collateral now
  reaches the scripted leg.

---

# Iteration 54 — F14-A.3 route-bound Ordered AI progress

Iteration 54 on `ralph/night` (baseline `8b615cf`, F14-A.2 — external
verify + review green). One coherent slice of the F14-A remainder:
the second defect class the A.2 matrix disclosed — authored `.opp`
lines that physically drive near a course but never enter one or more
checkpoint cylinders (london `circuit:2`/`4`/`5`, sf `circuit:7`;
measured misses ~11–120 m) — which stalls a trigger-only Ordered
field at that gate index forever.

## Task selection

No failing gate or review finding to repair — the A.2 review passed
with verification gaps and explicitly names the authored-miss /
UNK-11 question as open. The A.2 analysis's own inference — original
AI Ordered progress must be route-derived, not trigger-bound — was
recorded as the next F14/F15 candidate, so this is it. Remaining
candidates stayed unchanged-blocked (F05-B UNK-13, F15-B
research-gated, F16-C interactive finish, F11-C review judgment,
F13-C original-fidelity comparison, F18-A → F18-B/C scope, F07-B no
authored sample/output device, F10-B disclosed edge gaps).

## What landed

- `mm2_game::race` — `RouteGateLine`: binds every gate to its
  closest-approach arc on the driven polyline, re-based to the spawn
  arc, clamped non-decreasing in authored order so the Ordered
  sequence is always earnable by driving the line (a gate's physical
  crossing can never precede its bind — the bind *is* the line's
  closest approach). `measure` projects a pose onto the chased leg
  (absolute arc + lateral distance); `wrap()` counts a traversal per
  closed-route chase-index wrap; `reanchor()` walks the traversal
  count down at a stuck-recovery landing until the landing reads at
  or below the stuck pose's own measure — a walk-back that crosses
  the route boundary cannot bank arc the car did not drive.
- `RaceProgress::advance_route` — Ordered-only credit: banks the next
  required gate once the driver's high-water arc passes its bound;
  closed routes offset gate bounds by `lap × loop_len`; open routes
  bind their first traversal only (no retail lapped event ships an
  open route — disclosed bound, not a measured hole). Physical
  `advance` keeps full trigger authority and wins wherever the car
  really crosses — a triggered gate never double-counts;
  `route_clears` tallies route-derived clears separately.
- `mm2_app` — `spawn_opponents` binds a `RouteGateLine` per `Ordered`
  roster entry with a resolved route (player and `AnyOrder`
  participants never carry one; a route-less entry binds nothing);
  `opponent_drive` grows `arc_high` only while the pose projects
  within `ROUTE_ARC_LATERAL` (25 m) of the chased leg — a car punted
  onto a parallel road earns nothing — and resyncs traversals on the
  re-anchor landing; `advance_race` applies `advance_route` to bound
  participants; the smoke `opps=` row suffixes `/Nd` when
  route-derived clears occurred.
- `docs/original-rules.md` — DSN-45 records the designed policy; the
  original's own AI Ordered accounting stays UNK-11 (unverified), and
  the re-anchor resync is named in the entry.

## Evidence

Synthetic tests:

- `tests/race.rs` 28 → 35 (+7 new route tests): negative pre-wrap
  measure, authored-line-miss credit, no double-counting a triggered
  gate, closed-route lap wrap, AnyOrder non-binding, open-route
  first-traversal bound, and the boundary-crossing re-anchor resync.
- `tests/opponents.rs` 40 → 42 (+2 production-path integration): a
  route-less Ordered opponent binds no line; an authored route that
  misses a gate cylinder still earns ordered progress through the real
  `load_session_world` → roster → `opponent_drive` → `advance_race`
  path on a synthetic VFS install. Nine new tests across the two
  suites.

Retail (`fnv1a64:e91e6cd4b2ae30d9`, this work-tree's binary — logs in
`/tmp/mm2-circuit-matrix-v3/`, published in `docs/race-coverage.md`'s
v3 section): the four authored-miss events × {`--bot`, `--parked`},
Amateur `--frames 12000`, **8/8 `rc=0 status=pass`**.

- **london-4 parked**: two opponents complete lap 0 (`0c/2l`,
  `/2d` each) — the first opponent lap completions on an
  authored-miss event; the missed g0/g9 bank by route arc.
- **sf-7** both legs: one opponent completes lap 0 (`2c`/`5c` on
  `2l`, `/10d`); the pack holds the v2 `5c` plateau at gate 5's
  ~985 m bind — the leader crosses, the rest never reach it.
- **london-2** (gate-0 bind ~700 m) and **london-5** (gate-2 bind
  ~1050 m): `0d` on every opponent both legs — the high-water arc
  never reaches the first missed gate's bound under permanent
  spawn-pile-up churn (`opp_rec` 12–33, stuck peaks to 900w). These
  fold into the traversal-skill residual class (F15-B), honestly —
  the model earns by driving and cannot invent progress.
- `results=0` on every leg — no inflated finishes; physical
  crossings still count as `crossings` (`7c/0d` rows exist — cars
  that wander into cylinders).

Offline bind probe (temporary diagnostic, removed): retail `.opp`
routes of all four events produce sensible projected gate arcs —
confirms the binds exist and the london-2/5 `0d` outcome is "field
never arrives", not "line never bound".

## Gates

`cargo fmt --all -- --check` clean; `cargo clippy --locked
--workspace --all-targets --all-features -- -D warnings` clean;
`cargo test --locked --workspace` — all 63 suites green
(tests/race.rs 35, tests/opponents.rs integration +2).

## Classification

DSN-45 is a designed policy end to end: bind geometry, the 25 m
corridor, traversal accounting, the re-anchor resync, open-route
first-traversal bound, and the separate `route_clears` tally are all
implementation choices. The original's AI Ordered accounting is
unverified (UNK-11) — `2l` opponent lap rows that pre-date this model
in the v2 logs are consistent with route-derived progress being the
plausible original rule, but nothing here verifies it. Player and
`AnyOrder` progress remain trigger-bound; `status=pass` legs are
smoke records, not completability claims.

## Remaining open items

- F14-A stays active: AC04/AC05/AC06 legs and Professional/hold
  coverage remain open; the authored-miss defect class is addressed
  at the progress-model level while the traversal stalls it exposed
  (london-2/5, sf-7 pack) move to the F15-B controller class.
- UNK-11's Ordered-progress clause stays open — the designed model
  satisfies the "must produce honest progress" constraint, not the
  original-rule question.
- Open-route Ordered laps past the first traversal still need
  physical crossings (no retail lapped event ships an open route —
  disclosed bound).
- The scripted `--bot` player still cannot climb off-network ramps;
  its cp counts are a controller limit, not course feasibility.

---

# Iteration 53 — F14-A.2 Circuit runtime matrix + densify gate-coverage repair

Iteration 53 on `ralph/night` (baseline `0fe4787`, F10-B.15 —
external verify + review green). Two coupled pieces: the F14-A
remainder — the representative-playability runtime matrix over the
complete authored Circuit catalog — and one bounded repair the
matrix's own analysis surfaced (`densify_route` re-paths could
abandon checkpoint coverage the authored `.opp` line had, stalling
whole Ordered fields at cp 0).

## Task selection

No failing gate or review finding to repair — the B.15 review passed
with verification gaps. Among ready candidates, the Circuit matrix
was the only major race family with zero runtime evidence (Blitz has
F12-C's, Checkpoint the F13-C matrix), and F14-A names the AC06
representative-playability leg as its remaining work. Circuits also
exercise `CheckpointRule::Ordered`, lap counting and start-line reuse
that the any-order matrix never touched. Remaining candidates stayed
unchanged-blocked (F05-B UNK-13, F15-B research-gated, F16-C
interactive finish, F11-C review judgment, F13-C original-fidelity
comparison, F18-A → F18-B/C scope, F07-B no authored sample/output
device, F10-B.15's disclosed 4-car-chain gap — test-only, lower
value).

## What landed

### The matrix (20 events × 2 drivers, Amateur, `--frames 12000`)

- Denominator: `mm-inspect events` — **20 cataloged Circuit rows**
  (10 London `circuit0..9`, 10 SF `circuit0..9`), all `ready`.
  `circuit10`/`circuit11` rows are uncataloged extras, kept visible
  not counted.
- 40 legs, scripted `--bot` + stationary `--parked` per event, the
  F13-C command pattern. Results + per-leg logs local in
  `/tmp/mm2-circuit-matrix/` (pre-fix) and `/tmp/mm2-circuit-matrix-v2/`
  (post-fix); each log stamps its commit. Published in
  `docs/race-coverage.md`'s new Circuit section.

### The repair — `densify_route` gate coverage

Analysis found uniform field-wide plateaus — every opponent (and the
scripted player) stalling at the same gate index on several events.
Tracing london `circuit:6` (all 6 opponents + player at cp 0,
repeated re-anchors, a rendered capture showing the field jammed at
the start junction) isolated it: gate 0 sits on a flyover whose ramp
is dressed with breakable construction bangers and not covered by
routable BAI lanes; the authored `.opp` leg crosses the cylinder at
4.7 m (r10), but the nav re-path — `leg_leaves_corridor` →
`route_candidates` — detoured ~500 m around the block and missed the
trigger by **198 m**. Every Ordered participant required a physical
crossing it could never make.

- `crates/mm2_game/src/nav.rs` — `densify_route` gains a
  `gates: &[Checkpoint]` parameter: a re-path that drops a trigger
  the authored segment crossed (`Checkpoint::crossed` over the
  authored a→b and every consecutive pair of a → lane samples → b)
  is rejected and the authored leg stands — the same fallback
  unroutable legs already used. Re-paths that preserve coverage —
  including ones that newly cross a gate the authored line missed —
  still replace the leg.
- `crates/mm2_app/src/opponents.rs`/`session.rs` —
  `driving_route(route, nav, gates)`; both callers pass the event's
  `RaceDefinition.checkpoints` (opponent roster + scripted bot
  route).
- `docs/original-rules.md` — DSN-44 records the constraint as an
  implementation choice (the original never densifies; the route is
  the AI course, UNK-11).
- `tests/nav.rs` — +2: a re-path that would drop the authored-crossed
  gate keeps the leg verbatim; a re-path that still crosses the gate
  densifies and the published line still crosses it. 34/34 nav tests
  green.

Retail verification of the repair (this commit's binary, install
`fnv1a64:e91e6cd4b2ae30d9`): london `circuit:6 --bot --frames 3000`
— driven-route min distance to every gate ≤ 4.7 m (was 198 m at gate
0); `cp=1/14`, three opponents banking `1c` within 50 s, banger
impacts registering where the field smashes the ramp barriers —
vs everyone parked at `0c/900w` before.

### Post-fix matrix (v2, same 40-leg pattern)

The rebuilt work-tree binary reran all 40 legs (logs stamp the
`0fe4787` base commit — the repair was uncommitted at run time;
disclosed in `docs/race-coverage.md`). **40/40 `rc=0 status=pass`,
`dup=0`.** Verified outcomes:

- **london-6**: every opponent `0c → 1c` — gate 0 crossed by the whole
  field. Plateau moved to gate 1 with heavy escape/re-anchor churn;
  the restored course flows the field past the parked control and
  punts it into the Thames (`rcv 5w → 294w`). Residual reads as
  traversal difficulty past restored coverage (F15-B class), not a
  coverage defect — driven line ≤ 4.7 m of all 14 gates.
- **london-9 / sf-9**: coverage restored on the driven lines, but the
  uniform plateaus persist (`1c`/`4c` all-six) — same traversal
  residual class, disclosed per event.
- **london-4 parked**: field `7c → 9c` — a kept re-path now crosses a
  gate the authored line missed; the stall lands on the authored-miss
  gate 9 (32 m vs r11).
- **sf-2 scripted**: `cp 3/13 → 9/13` — densification improvement.
- **Authored-miss events unchanged**: london-2 @0c, london-5 @2c,
  sf-7 @5c — verbatim routes can't gain coverage, as designed.
- Minor disclosures: `wheels=0/4` end poses on four scripted legs;
  `dropped` 159/261 on two; london-2 parked `rcv=251w` unchanged.

## The second defect class (identified, not repaired)

Four events plateau *even verbatim*: the authored `.opp` line itself
never enters some gate cylinders — measured min distances
london-2 gate0 11 m vs r7, london-4 gate0 16 m vs r11 + gate9 32 m,
london-5 gate2 36 m, sf-7 gates5/6/7 34/120/63 m — uniform across
every `-a-*`/`-p-*` route of the event. Original opponents following
these lines could not have physically crossed either, so the
original's AI progress accounting must be route-derived rather than
trigger-based (inference under UNK-11 — unverified). Repairing it
means deciding how AI Ordered progress is bound to the driven route
(gate→route-position binding, per-lap, per-opponent) — a separate
coherent task recorded as the next F14/F15 candidate, not bundled
into this slice.

London-2 additionally showed the parked control car water-recovered
241× (`rcv=241w/19f`) — the stalled field punts it into the Thames
repeatedly; collateral of the same stall, kept visible.

## Gates

`cargo fmt --all -- --check` clean; `cargo clippy --locked
--workspace --all-targets --all-features -- -D warnings` clean;
`cargo test --locked --workspace` — all suites green
(tests/nav.rs 32 → 34).

## Classification

The densify constraint is an implementation choice (DSN-44). The
matrix is original-content validation evidence (fingerprinted
install, authored data) — `status=pass` smoke records only; several
events remain visibly uncompletable under the current AI-progress
model, disclosed not claimed. The second defect's original rule is
inference under UNK-11, not verified.

## Remaining open items

- F14-A/F14-C stay open: AC04 real-Circuit-with-opponents is now
  evidenced for the courses whose routes cover their gates; the four
  route-miss events need the AI-progress model task first.
- Opponent Ordered progress model: bind gates to route positions
  (per-opponent, per-lap) so authored lines that never thread a
  cylinder still produce honest progress — or find and verify the
  original's actual rule.
- The scripted `--bot` driver still aims straight at the next gate —
  it cannot climb off-network ramps a human would; its cp counts are
  a controller limit, not course feasibility.
- Re-anchor counts stay high on penned fields — recovery is bounded
  and disclosed, not a course fix.

---

# Iteration 52 — F10-B.15 multi-edge collision accounting repair

Iteration 52 on `ralph/night` (baseline `1a5b521`, F10-B.14 —
external verify + review green). One coherent slice of the F10-B
AC03 remainder, repairing the multi-edge defect found while
working the two symmetric edges the B.14 review disclosed
untested (one striker → two cars; a daisy chain / 3+ pileup).

## Task selection

No failing gate or review *finding* to repair — the B.14 review
passed with verification gaps, two of them actionable coverage
gaps in the same system: the one-striker-two-cars drain and the
3+ pileup. While studying `knock_ambient`'s per-edge striker
corrections a real defect surfaced: corrections are *velocity
targets* along the push direction, so a striker's second edge in
one drain rewrote the target and erased the first edge's payment
— both struck cars launched while the striker paid once
(momentum injection). Relatedly, a striker the same pass itself
flipped was skipped outright, so a same-tick daisy chain's last
car launched uncharged. That repair plus the disclosed edge
coverage is this slice. The remaining candidates were
unchanged-blocked (F05-B UNK-13, F17-B needs F27/F17-C, F15-B
research-gated, F16-C interactive finish, F11-C review judgment,
F13-C original-fidelity comparison, F18-A → F18-B/C scope, F07-B
no authored sample/output device).

## What landed

- `crates/mm2_app/src/traffic.rs` — `knock_ambient`'s apply is
  now two passes. The flip pass is unchanged semantically (the
  `Lane` re-check still dedups multi-edge hits; each car flips at
  most once) but now also records `Knock.struck_pre` — the struck
  car's velocity along `dir` the instant before its launch — and
  appends `(car, its edge's striker)` to `handed_over`. The new
  correction pass compounds per striker: the striker's *first*
  committed edge writes the wall-returning velocity target
  (`struck_pre + severity − J/m_s`), and every later edge of the
  same striker is a pure `−dir·J` impulse debit — a second target
  write along a shared direction would erase the first edge's
  payment. A car this pass flipped on a *different* pair owes the
  pure debit from its post-flip velocity (the kinematic–kinematic
  edge charged it no wall), while the follower-follower *mutual*
  pair — where the striker is the same pair's other side — still
  owes nothing: its struck-side launch already is its share.
  `handed_over` therefore keys on the pair, not just the entity.
- Avian 0.7 source confirmed the topology assumption behind the
  design: the broad phase *does* create kinematic–kinematic pairs
  for moved proxies (only the solver skips solving them), so a
  lane car really can be a striker, and the mutual-pair edge
  really does produce both orientations in one drain.
- `tests/traffic.rs` — new `spawn_shaped_follower` helper
  (caller-chosen hull width and mass) +4 integration tests;
  `two_lane_install`'s parallel lanes stage the side-by-side
  contacts.

## Evidence

Synthetic tests (`cargo test -p mm2_app --test traffic` 34 → 38):

- `one_striker_pays_both_cars_it_flips` (new) — a wide 2600 kg
  block sliding down the gap between two parked followers'
  lanes contacts both on the same update (proven through the
  `Collisions` graph): `knocked == 2`, `kns x == 2`, the striker
  reads ≈7 m/s — both transfers paid; a per-edge target write
  would leave it ≈14 having paid one. Both wrecks `Knocked` +
  `Dynamic` and launched (>6 m/s).
- `a_same_tick_chain_charges_the_middle_car` (new) — a 6 m-wide
  driving follower's front face reaches a striker block and a
  light (200 kg) parked neighbour on the same step; the
  neighbour's mass puts the mutual `B←C` orientation under the
  impulse floor, so B's only flip edge is the block's and the
  pair that flips it can never alias the pair it strikes:
  `knocked == 2`, `kns x=1 a=1`, B debited past its own launch
  (~0.9 m/s vs ~2.7 uncorrected), C launched (~6 m/s, the corner
  contact's normal splits it lateral/forward), the block
  target-corrected (~4.8 m/s exchange share).
- `a_mutual_follower_edge_charges_the_exchange_once` (new) — a
  driving follower clipping a parked neighbour flips *both* on
  the same pair (`kns a=2`); the exchange splits (~7–8 m/s
  shares) and the mover is not debited a second time — the
  same-pair skip this rework had to preserve.
- `a_same_tick_three_striker_pileup_flips_the_car_once` (new) —
  B.14's edge extended to three strikers: `knocked == 1`, one
  `x` charge, single-transfer wreck launch, one corrected
  striker + two uncorrected wall-shove losers
  (`sorted[1,2] − sorted[0] > 4`).

Retail headless (install `fnv1a64:e91e6cd4b2ae30d9`, read-only,
this commit's binary):

- sf `--headless --frames 3000` → `status=pass traf=16/16 sp=41
  rec=25 dead=0 stuck=0 crx=56 jmp=0 kn=4 kns=2p/2a/0x
  dmg=23a/0d/0r` — **bit-identical to B.14/B.13** (the staged
  multi-edge drains do not occur in this cruise; the repair only
  engages when they do).
- london `--headless --frames 1200 --spawn 0.4,5.5,-720,0` →
  `status=pass … crx=33 kn=2 kns=0p/0a/2x` — bit-identical.

## Gates

`cargo fmt --all -- --check` clean; `cargo clippy --locked
--workspace --all-targets --all-features -- -D warnings` clean;
`cargo test --locked --workspace` — all 69 suites green
(tests/traffic.rs 34 → 38, tests/banger.rs 24/24 unchanged,
mm2_app lib 51/51 unchanged).

## Classification

Implementation choice end to end — the compounding correction is
the designed transfer accounting extended across a striker's
edges in one drain; the original's ambient crash response stays
unverified (UNK-12).

## Remaining open items

- F10-B stays active: AC03's "player-hit feel" leg is manual
  evidence (no rendered/interactive capture this run). Original
  junction/spawn timing and crossing geometry (UNK-12) and
  signal-prop model fidelity remain.
- A striker's *dropped* edge (its knock loses the `Lane`
  re-check) still owes nothing — the pre-existing first-flip-wins
  semantics, bounded and momentum-losing rather than injecting.
- Multi-edge coverage exercises one striker → two cars and a
  dynamic→lane→lane chain; a three-car chain A→B→C→D where the
  middle two are both strikers is geometrically stageable but was
  not separately asserted (same code path, longer chain).
- The striker-correction *linear* write stays unclamped (bounded
  velocity target) — same shape as before, disclosed not changed.

---

# Iteration 51 — F10-B.14 striker-correction spin bound + same-tick pileup coverage

Iteration 51 on `ralph/night` (baseline `3515105`, F10-B.13 —
external verify + review green). One coherent slice of the F10-B
AC03 remainder, repairing the two actionable gaps the B.13 review
named on the same handover: the unclamped striker-correction spin
write (pre-existing on the banger path too) and the untested
same-tick pileup edge.

## Task selection

No failing gate or review *finding* to repair — the B.13 review
passed with verification gaps, two of them actionable code gaps in
the same system: (a) `write_striker_correction`'s
`angular_share` write was unclamped on both the banger and ambient
striker paths — and on the ambient path a `Player` striker carries
*no* solver-side `MaxAngularSpeed` at all, so the write was
genuinely unbounded there, not merely write-side-unbounded; (b) the
same-tick pileup edge (two strikers, one lane car) was untested. The
remaining candidates were unchanged-blocked: F05-B detachment is
UNK-13 research, F17-B needs F27's mode, F15-B fields are
research-gated, F16-C's AC01 leg needs an interactive finish,
F11-C's remainder is a review judgment, F13-C's is original-fidelity
comparison, F18-A's remainder is F18-B/C scope, F07-B's scrape leg
has no authored sample and its AC05 needs an output device.

## What landed

- `crates/mm2_app/src/contracts.rs` — `write_striker_correction`
  now clamps the striker's post-write angular velocity at
  `MAX_BANGER_ANGULAR_SPEED`, the same bound the solver-side
  `MaxAngularSpeed` stamps on banger and ambient bodies. One shared
  write-side bound covers all three callsites: the banger
  `apply_striker_correction`, the ambient non-car striker, and the
  ambient wreck striker. Linear writes untouched.
- `crates/mm2_app/src/traffic.rs`, `banger.rs` — doc comments
  record the bound; the spawn comment's "inert while kinematic" is
  corrected to *non-binding*: verified in avian3d 0.7 source that
  `clamp_velocities` iterates every `SolverBody` including
  `IS_KINEMATIC`-flagged ones (the B.13 review's unverified
  dependency question — inconsequential either way at ~15 m/s lane
  speeds vs the 200/60 caps).
- `tests/traffic.rs` — the wreck fixture in
  `a_wreck_striker_counts_as_ambient` now carries the production
  wreck's solver bounds (the review's fixture-shape nit).
- `contracts.rs` gains a `#[cfg(test)]` module (+2 unit tests) for
  the shared write; `tests/traffic.rs` gains the pileup test (+1).

## Evidence

Synthetic tests:

- `a_huge_correction_share_clamps_at_the_banger_bound` (new) — a
  10⁶ rad/s share writes 60.0 rad/s in the share's direction, and
  the linear leg is untouched. Non-vacuous: the unclamped value
  would read ~10⁶.
- `an_under_bound_share_lands_verbatim_and_counts_the_prior_spin`
  (new) — a 30 rad/s share lands verbatim on a calm striker, while
  a striker already at 55 rad/s clamps its *total* at the bound —
  matching solver-side `MaxAngularSpeed` semantics.
- `a_same_tick_pileup_flips_the_car_once` (new, integration) — two
  strikers resting side by side across a driving follower's path;
  the `Collisions` graph proves both pairs' contact begins on the
  same update (the `CollisionStart` drain lags one fixed step for
  both). Asserted: `knocked == 1`, exactly one `x` class charged,
  the same entity `Knocked` + `Dynamic` carrying a single
  transfer's launch (~5–9 m/s band, not ~2×), and the
  winner/loser split — the corrected striker holds its exchange
  share (~6 m/s) while the dropped edge's striker keeps the faster
  kinematic wall shove (~14 m/s), so `max − min > 4`. 60 further
  ticks of resting re-contact add no flip.

Retail headless (install `fnv1a64:e91e6cd4b2ae30d9`, read-only,
this commit's binary):

- sf `--headless --frames 3000` → `status=pass traf=16/16 sp=41
  rec=25 dead=0 stuck=0 crx=56 jmp=0 kn=4 kns=2p/2a/0x
  dmg=23a/0d/0r` — **bit-identical to B.13's record**: the bound
  never engaged at ordinary speeds (inert by design; it only caps
  the transient-spike class).
- london `--headless --frames 1200 --spawn 0.4,5.5,-720,0` →
  `status=pass … crx=33 kn=2 kns=0p/0a/2x` — bit-identical.

## Gates

`cargo fmt --all -- --check` clean; `cargo clippy --locked
--workspace --all-targets --all-features -- -D warnings` clean;
`cargo test --locked --workspace` — all 69 suites green
(tests/traffic.rs 33 → 34, mm2_app lib 49 → 51, tests/banger.rs
24/24 unchanged).

## Classification

Implementation choice end to end — the bound is the designed
banger convention extended write-side to the shared correction;
the pileup dedup is the existing decide-then-apply design under
test. The original's ambient crash response stays unverified
(UNK-12).

## Remaining open items

- F10-B stays active: AC03's "player-hit feel" leg is manual
  evidence (no rendered/interactive capture this run). Original
  junction/spawn timing and crossing geometry (UNK-12) and
  signal-prop model fidelity remain.
- The striker-correction *linear* write stays unclamped (a bounded
  velocity target on the ambient path, transfer-math-bounded on
  the banger path) — same shape as before, disclosed not changed.
- Same-tick edge covers two strikers on one car; the symmetric
  one-striker-two-cars and three-plus pileups share the mechanism
  (each edge decided independently, apply re-checks `Lane`).

---

# Iteration 50 — F10-B.13 wreck bound + striker-class disclosure

Iteration 50 on `ralph/night` (baseline `10f26dfb`, F10-B.12 —
external verify + review green). One coherent slice of the F10-B
AC03 remainder, repairing the two verification gaps the B.12
review named on the same handover: the wreck's unbounded spin and
the record's inability to say who struck each handover.

## Task selection

No failing gate or review *finding* to repair — the B.12 review
passed with verification gaps. Two of those gaps were actionable
code gaps in the same system: (a) the wreck's contact-lever
`angular_share` write was unclamped and flipped ambient cars
carried no solver speed bound — unlike banger bodies, which clamp
60 rad/s write-side and solver-side — so a transient spike could
leave a fast-spinning wreck whose spin fed later
`normal_speed` readings (the sf-8 cascade class); (b) `kn=` could
not say whether a participant ever struck a car, which is exactly
what AC03's checklist asks. The remaining candidates were
unchanged-blocked: F05-B's detachment is UNK-13 research, F17-B
needs F17-C's mode, F15-B's fields are research-gated, F16-C's
AC01 leg needs an interactive finish, F11-C's remainder is a
review judgment, F13-C's remainder is original-fidelity
comparison, F18-A's remainder is F18-B/C scope, F07-B's scrape
leg has no authored sample and its AC05 needs an output device.

## What landed

- `crates/mm2_app/src/traffic.rs` — `spawn_ambient_car` stamps
  `MaxLinearSpeed(MAX_BANGER_LINEAR_SPEED)` /
  `MaxAngularSpeed(MAX_BANGER_ANGULAR_SPEED)` (the bounds every
  banger body carries; inert while kinematic — `drive_ambient`
  owns the ~15 m/s lane velocity — binding once the body flips
  dynamic). `knock_ambient`'s wreck spin write now clamps at
  `MAX_BANGER_ANGULAR_SPEED` — bounded write-side *and*
  solver-side like `angular_kick`/`banger_bundle`. The
  striker-correction path keeps B.12's banger parity (its share
  pre-exists unclamped there).
- The same system counts each handover's striker class —
  `knocked_by_participant` (`Player` marker, local or AI) /
  `knocked_by_ambient` (`AmbientCar`, lane follower or wreck) /
  `knocked_by_other` (banger bodies, break fragments, world-side
  bodies) — into the smoke record's new `kns=Np/Na/Nx` field,
  emitted only when `knocked > 0` (knock-free records stay
  bit-identical).
- `crates/mm2_app/src/smoke.rs` — the `kns=` field beside `kn=`.
- `tests/traffic.rs` — the fixture app now wires
  `damage::apply_impact_damage` (+ `DamageEvent` message) in
  production order after `collect_impacts`, and fixture followers
  carry the production spawn's solver bounds.

## Evidence

Synthetic tests (`cargo test -p mm2_app --test traffic` 31 → 33):

- `a_participant_striker_takes_damage_and_names_the_class` (new) —
  the session's *real* player vehicle (stamped with authored-style
  `VehicleDamage` bounds; the fixture car loads no
  `vehcardamage`) slides into a parked follower: the car flips
  once (`knocked_by_participant == 1`, `RigidBody::Dynamic`,
  solver bounds present on the wreck), an `ImpactEvent` emits,
  and the striker's damage accrues `severity × follower mass`
  through the production `collect_impacts → apply_impact_damage`
  chain — AC03's "player-hit … damage" leg proved end to end,
  not just through generic block strikers.
- `a_wreck_striker_counts_as_ambient` (new) — a sliding dynamic
  wreck flips a queued follower and counts `a`: pile-ups and
  player hits now read differently on the record.
- `a_light_striker_shares_the_exchange_not_its_speed` extended —
  the plain block striker asserts `x` (other).

Retail headless (install `fnv1a64:e91e6cd4b2ae30d9`, read-only):

- sf `--headless --frames 3000` → `status=pass … kn=4
  kns=2p/2a/0x … dmg=23a/0d/0r` — the same four handovers B.12
  recorded, now attributed: **two participant strikes** (the Hold
  driver's own hits, damage applied through the pipeline) and two
  ambient-car strikes.
- london `--headless --frames 1200 --spawn 0.4,5.5,-720,0` →
  `status=pass … kn=2 kns=0p/0a/2x` — both `x` class: neither a
  participant nor an ambient car (banger bodies and break
  fragments are the remaining class).

## Gates

`cargo fmt --all -- --check` clean; `cargo clippy --locked
--workspace --all-targets --all-features -- -D warnings` clean;
`cargo test --locked --workspace` — all 69 suites green
(tests/traffic.rs 33/33 incl. the 2 new tests, tests/banger.rs
24/24 unchanged).

## Classification

Implementation choice end to end — the bounds are the designed
banger convention extended to ambient wrecks, the class
disclosure is evidence plumbing; the original's ambient crash
response stays unverified (UNK-12).

## Remaining open items

- F10-B stays active: AC03's "player-hit feel" leg is manual
  evidence (no rendered/interactive capture this run); the damage
  leg now has synthetic + retail-counter evidence. Original
  junction/spawn timing and crossing geometry (UNK-12) and
  signal-prop model fidelity remain.
- The striker-correction angular share pre-exists unclamped on
  the banger path too (bounded solver-side there) — left
  untouched for B.12 parity; a shared write-side clamp is a
  follow-up candidate if a cascade ever measures through it.
- Same-tick pileup edge (two strikers, one lane car) remains
  untested — the first flip wins, the second keeps its solver
  wall response; bounded by decide-then-apply, not exercised.

---

# Iteration 49 — F10-B.12 momentum-correct collision handover

Iteration 49 on `ralph/night` (baseline `63ffb20`, F11-C.2 doc repair —
external verify + review green). One coherent slice of the F10-B
collision-fidelity remainder: `knock_ambient` carried the same
double-energy defect F04-C.4 fixed for bangers — the solver answers a
kinematic traffic car as infinite mass (the striker takes a wall
response), then the handover added a free approach-speed kick on top.
The flip now replays the hit as a two-body transfer.

## Task selection

No failing gate or review finding to repair. Among the listed
remainders, F10-B's collision-fidelity scope was the ready one: the
scrape leg of F07-B has no authored sample to bind (car audio tables
carry horn/clutch/engine rows only — confirmed via the VFS), F15-B's
fields are research-gated, F17-B needs F17-C's mode, F16-C's AC01 leg
needs an interactive finish. The defect itself was already visible in
B.6's code.

## What landed

- `crates/mm2_app/src/contracts.rs` — the banger transfer math
  extracted for reuse: `Transfer`/`resolve_transfer`
  (`(1+e)·v·μ` impulse, launch = J/m_struck, `None` when the striker
  mass cannot be resolved), the `StruckMut` query tuple,
  `angular_share` (contact-lever Δω), `striker_correction` /
  `write_striker_correction`.
- `crates/mm2_app/src/banger.rs` — consumes the shared helpers
  unchanged (24/24 banger tests pass).
- `crates/mm2_app/src/traffic.rs` — `knock_ambient` rewritten
  decide-then-apply: a `Knock` record per qualifying edge (deepest
  contact, push direction from the manifold normal on either collider
  side, bounded launch, impulse, both levers, transfer); the apply
  pass flips `Lane`→`Knocked` on the same entity (`Lane` re-check
  dedups multi-edge hits), writes the mass-correct launch plus the
  contact-lever spin, inserts `RigidBody::Dynamic`, departs the
  junction, counts `traffic.knocked` → the `kn=` smoke field.
- Striker correction is a **velocity target**, not a returned
  impulse: instrumentation showed Avian's recorded `total_impulse`
  accumulating penetration-recovery and restitution passes (13641 /
  22929 recorded vs ~9800 / 19500 actual Δv·m), so the striker's
  push-direction component is rewritten to
  `struck_pre + severity − J/m_s` — conserving by construction. A
  lane-follower striker takes no correction (`drive_ambient` owns its
  velocity) and a striker this pass already flipped is skipped, so a
  follower-follower edge never charges the exchange twice; unresolved
  masses keep the approach-speed launch with no correction.
- Same authority/phase gate and reader drain as `drive_ambient` — no
  predicted-session handover, no stale burst after pause.

## Evidence

Synthetic tests (`cargo test -p mm2_app --test traffic` 31/31,
`--test banger` 24/24):

- `a_hard_hit_hands_the_follower_to_dynamics` extended — the wreck
  slows to its share range instead of the old dead-stop, the striker
  is rewritten to its share instead of the ~15 m/s wall match, a
  no-injection momentum bound holds, and exactly one flip occurs
  across 60 re-contacting ticks (same entity, dynamic, frozen cursor).
- `a_light_striker_shares_the_exchange_not_its_speed` (new) — a
  400 kg block into a parked 1200 kg car leaves both at the ~6 m/s
  inelastic common velocity with momentum conserved — the exact-share
  leg.
- `a_light_touch_leaves_the_car_lane_following` — sub-threshold
  contacts stay kinematic (unchanged).
- Fixture followers now carry production `Mass`/
  `CollisionEventsEnabled`.

Retail headless (install `fnv1a64:e91e6cd4b2ae30d9`, read-only):

- sf `--headless --frames 3000` → `traf=16/16 sp=41 rec=25 dead=0
  uns=0 q=0 jq=2 stuck=0 crx=56 jmp=0 kn=4 sig=647 sigd=3` — four real
  handovers, all counters finite.
- london `--headless --frames 1200 --spawn 0.4,5.5,-720,0` → `…
  crx=33 kn=2` — two real handovers on the flat-road spawn.
- london `--headless --frames 3000` plain → kn=0 (the Hold driver
  grounds out on props at 95 m — no handover exercised; honestly
  recorded, not filtered).

## Gates

`cargo fmt --all -- --check` clean; `cargo clippy --workspace
--all-targets --all-features -- -D warnings` clean; `cargo test
--workspace` — all 69 suites green.

## Classification

Implementation choice end to end — the original's ambient crash
response is unverified (UNK-12). The transfer math is the designed
two-body exchange shared with banger activation; no original-behavior
claim.

## Remaining open items

- F10-B stays active: AC03's player-hit feel/damage legs, original
  junction/spawn timing and crossing geometry (UNK-12), signal-prop
  model fidelity.
- Single-point impulse pair rather than per-contact impulses; no
  rendered/manual evidence of the handover.

---

# Iteration 48 — F11-C.2 handoff-doc repair

External review of iteration 47's candidate `6aab29d` (F11-C.2)
returned one blocking finding: a stale recorded test-count
baseline. This file and PLAN.md's F11-C.2 row claimed the
`mm2_inspect` suite went 12 → 15; the actual suite went 27 → 30
(`event.rs` module 7 → 10). Root cause: the baseline was copied
from F11-C.1's commit-time count ("5 → 12", correct at `1720a53`),
but ~113 commits landed between C.1 and C.2 and grew the suite to
27. The `+3` delta and the `30/30` gates line were already right.

Repair (docs-only, no code touched): corrected both claims to
`27 → 30`. Verified by recounting `#[test]` at base `93db26b`
(27 total / 7 in `event.rs`) and at `6aab29d` (30 total / 10 in
`event.rs`); `cargo test --locked -p mm2_inspect` re-run below.

The iteration-47 record follows, unchanged and still accurate.

---

# Iteration 47 — F11-C.2: catalog-wide deep event audit (`event --all`)

Iteration 47 on `ralph/night` (baseline `93db26b`, F07-B.9 scripted
drive-sequence evidence — external verify + review green). One
coherent slice of the F11-C remainder: the catalog-wide strict-audit
evidence leg the plan owed, backed by a small tooling change so the
whole catalog runs through the single-event deep check in one
command.

## Task selection

No failing gate or review finding to repair (F07-B.9 review passed,
verification gaps only). Among the listed remainders, F11-C's
run-and-record leg was the ready one: the other candidates are
research-gated (F15-B's `unkFlag`/`cornerBrakingThreshold` fields,
F18-A's `.ldef`/`.lmap` semantics under UNK-24, F05-B's detachment
rule under UNK-13), blocked on missing features (F17-B → F17-C,
F16-C's AC01 process leg → an interactive finish — scripted-driver
results are deliberately ineligible), blocked on an audio output
device (F07-AC05), or entirely designed policy (F07-B's scrape leg
has no authored sample to bind). F11-C won because the per-event
deep audit existed but had only ever been run on single rows — the
catalog-wide leg needed one small production change plus the retail
evidence run.

## What landed

- `tools/mm2_inspect/src/event.rs` — `CitySweep` + `sweep()`: run
  `inspect_event`'s full dependency-closure check on every cataloged
  event in a city (per-record deep parse incl. the aimap/pathset
  records the catalog scan leaves `Unparsed`, `RaceDefinition` and
  `OpponentRoster` builds at both difficulties, wired vehicle ids
  cross-checked against `VehicleCatalog`). The vehicle catalog is
  scanned once per run and shared — `inspect_event` now takes the id
  set instead of rescanning per row. `CitySweep::failures()`
  aggregates the same conditions `EventReport::failures()` reports
  per event plus table errors and an empty catalog.
- `mm2-inspect event` CLI — `--all` sweeps the whole catalog
  (`--city` restricts it to one stem; without `--all`, `--city` and
  `--event` stay required as before, and `--event` conflicts with
  `--all`). Output: table statuses, one line per event
  (`ready`/`incomplete`, record count, `defs ok/ok`, `rosters
  ok+Ni`), indented per-event failure detail, then a per-city
  summary carrying the extras count.
- `docs/race-coverage.md` — `--all` added to the instrument list and
  the sweep's retail numbers recorded in the denominator section.

## Evidence

Synthetic tests (`tools/mm2_inspect` suite, 27 → 30; `event.rs`
7 → 10):

- `sweep_reports_every_cataloged_event` — all three authored rows of
  the synthetic install appear in row order; the fully-wired row is
  clean, the two record-less rows are `incomplete` and named in the
  strict failure list — the denominator is never filtered;
- `sweep_surfaces_a_record_failure` — a malformed `circuit0.aimap`
  lands under `circuit:0` in the sweep failures;
- `sweep_empty_catalog_is_a_failure` — a city with no race data
  reports the empty catalog as a failure, not silence.

Retail run-and-record (`fnv1a64:e91e6cd4b2ae30d9`, read-only
install, this commit's binary):

- `mm2-inspect event <install> --all` — **90/90 cataloged events
  `ready`** (45/city), 0 incomplete, 0 failed records, 0 failed
  `RaceDefinition`/`OpponentRoster` builds at either difficulty.
- `--strict` exits 2 on **96 authored anomalies**, all previously
  disclosed classes: 77 orphan `.opp` route records (46 amateur +
  31 professional), the `sf/race0` aimap 6-vs-7 table mismatch, and
  18 per-record diagnostics the catalog-wide audits count but don't
  attribute per row — 8 `AmbDenisty` header misspells, 6 omitted
  `Filename` labels (london `crash8` + sf `crash4`/`crash9` data
  pairs), and 4 short rows (8 of 9 fields) skipped in london's
  `exam1_1.csv` (a `crash3` midterm waypoint file — authored data,
  disclosed not repaired).
- Sibling legs re-run the same commit: `events --strict` rc 0,
  `race-defs --strict` rc 0 (64 defs built per city, 26 crash-course
  rows `unsupported` by design), `opponents --strict` rc 2 on the
  same 79 authored anomalies.

## Gates

`cargo fmt --all -- --check` clean; `cargo clippy --locked
--workspace --all-targets --all-features -- -D warnings` clean;
`cargo test --locked --workspace` — all suites green (mm2_inspect
30/30 incl. the 3 new sweep tests).

## Classification

Implementation choice end to end — the sweep is an audit view over
the existing deep check; it makes no original-behavior claim. The
retail numbers are original-content validation evidence (named
fingerprint, full denominator, failures enumerated not filtered).

## Remaining open items

- F11-C stays active: AC02–AC05 rest on the landed runtime slices'
  test evidence (swept triggers, countdown/restart lifecycle,
  once-only ledger results) — promotion of those ACs is a review
  judgment, not new work this slice. AC06's "loaded" leg is the
  `mm2 --event` headless smoke records (F13-C matrix).
- The 96 strict findings are authored retail anomalies — they stay
  visible under `--strict` rather than being whitelisted away.
- F07-B continues: sustained-scrape semantics (no authored scrape
  sample — entirely designed) and the AC05 audible capture (needs a
  real output device).
