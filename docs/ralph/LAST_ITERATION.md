# Last implementation iteration

- Task ID and title: F12-B.1 — the RACE-6 objective navigation arrow:
  contract targeting in `mm2_game` (nearest un-cleared gate, explicit
  pick, armed finish, signed bearing), a session-owned UI needle in
  `mm2_app` (green ahead / yellow behind), and X/Z target cycling.
- Starting commit and resulting commit: started at
  `6d1c163fb3c1453d666897715e8a654da1e17909` (clean tree, branch
  `ralph/night`, F12-A externally checked); result = this commit.
- Why this slice: F12-B is next per the reconciled plan ("Implement
  timer/objectives/finish/failure with HUD/navigation feedback").
  Timer/objectives/finish/failure already landed under F12-A; the
  documented navigation instrument (RACE-6, HUD-2, CTL-1) was the
  biggest remaining discrete leg. Split as F12-B.1 — warning cues and
  results presentation stay open on the parent (see below).
- Production code changed:
  - `crates/mm2_game/src/race.rs`: `NavTarget` (`Gate(i)`/`Finish`),
    `TargetSelection` component (`picked`), `navigation_target`
    (nearest un-cleared gate by XZ distance; a valid pick wins until
    its gate clears then falls back; all gates cleared → the armed
    finish; `Ordered` → `None` per HUD-2's instrument list),
    `cycle_target` (authored-order walk, wraps both ways, skips
    cleared gates, empty remaining → `None`), `relative_bearing`
    (signed driver-frame angle, `+` = right, ground-plane,
    coincident → 0), `RaceProgress::remaining`.
  - `crates/mm2_app/src/race.rs`: `NavArrow`/`NavArrowPart` markers,
    `spawn_nav_arrow` (UI needle 6×34 px at screen top-center +
    diamond child at the tip — node-drawn because the embedded font
    is ASCII-only, DSN-8), `nav_target_input` (X forward / Z back —
    the original's X/S conflicts with WASD brake, DSN-8), and
    `update_nav_arrow` (rotation = bearing, `NAV_AHEAD`/`NAV_BEHIND`,
    hidden with no live target: no/stale/complete race, `Ordered`
    definition, resolved participant).
  - `crates/mm2_app/src/session.rs`: the player vehicle gets
    `TargetSelection` with its `RaceProgress`; the needle spawns in
    the event branch so non-event sessions carry no dead UI.
  - `crates/mm2_app/src/main.rs`: `nav_target_input` (frozen during
    `--frames` captures like all input) and `update_nav_arrow`
    registered in `Update`.
  - `README.md`: X/Z rows in the controls table.
  - `docs/original-rules.md`: `DSN-8` records the needle-vs-bitmap
    presentation, the `Ordered` no-arrow decision, the
    finish-targeting inference and the X/S → X/Z key departure.
- Tests added/changed and why:
  - `crates/mm2_game/tests/race.rs` (+6, 18 total): nearest-default,
    pick-wins/fallback/out-of-range, armed-finish targeting +
    position, `Ordered` → no arrow, cycling walk/wrap/skip-cleared/
    empty→None, bearing sign conventions incl. height-independence
    and the coincident case.
  - `crates/mm2_app/tests/race.rs` (+5, 23 total): the needle tracks
    the live objective through `Position` writes (visible/rotation≈0/
    green ahead → swept-gate retarget → yellow/behind), X/Z cycling
    through real `ButtonInput` (edge-triggered — a held key does not
    re-cycle; `Complete` ignores input), live during countdown,
    `Ordered` stays hidden, finish arming → resolved → teardown
    despawns the session-owned needle. Harness now spawns the arrow
    via the production `spawn_nav_arrow` and stamps participants
    with `TargetSelection` like the real session does.
- Commands actually run and results (this machine, macOS arm64):
  - `cargo test -p mm2_game -p mm2_app` — all groups ok (incl. 18
    race contract + 23 app race tests).
  - `cargo fmt --all -- --check` — PASS.
  - `cargo clippy --locked --workspace --all-targets --all-features
    -- -D warnings` — PASS (one `type_complexity` hit refactored into
    the `NavArrowPart` marker rather than allowed).
  - `cargo test --locked --workspace` — PASS, all groups, 0 failures.
  - Retail `--city london --event blitz:0 --headless` —
    `status=pass`, `cp=1/3 tl=18.0s` (arrow systems live, timer
    unchanged).
  - Retail `--city london --event blitz:0 --frames 100 --screenshot`
    — `status=pass`, 4.3 MB PNG: needle + diamond visible top-center
    during `GET READY`, green, tilted slightly right toward the gate.
  - Retail `--city london --event blitz:0 --frames 700 --screenshot`
    — `status=pass`: needle green ahead tracking the gate while
    `time 20.6s` counts down (`cp 0/3` — capture input is frozen, so
    the car idles; expected).
- Acceptance IDs satisfied / still open:
  - F12-AC04 advances: the needle is presentation on the same
    authoritative progress/clock — verified deterministic via
    `Position`-driven segments. Still partial (no audio exists).
  - F12-AC02 leg: cleared-pick fallback + `Complete` input gate mean
    a stale pick cannot aim at a resolved objective — candidate
    level.
  - F12-AC05 leg: the needle and `TargetSelection` are
    session-owned — teardown test proves despawn; no stale pick
    survives a restart. Rewards still don't exist (no reward system),
    so "no duplicate rewards" stays vacuous until F16.
  - F12-AC01/AC03/AC06 unchanged from F12-A's status.
- Stock data/GPU/audio/network limitations: rendered evidence covers
  the ahead/green needle only — the yellow-behind state and X/Z
  cycling are exercised by tests, not rendered (a behind-target frame
  needs input the frozen capture can't supply). No audio system
  exists, so warning cues can only ever be visual; whether the
  original even has a low-time cue is undocumented — a designed-only
  decision is owed, not implemented.
- Unresolved blockers or discovered regressions: none.
- Next smallest useful action: F12-B remainder — decide the low-time
  warning cue (designed policy; no documented original rule) or close
  F12-B if presentation is deferred to F17; then F12-C catalog
  playthrough matrix. Independent ready alternates: F03-A (prop
  audit), F09-A (BAI parser).

This is a candidate handoff. External code-gate and separate review results live in the runner state directory and are not implied by this report.
