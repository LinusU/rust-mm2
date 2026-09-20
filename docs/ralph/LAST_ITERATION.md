# Last implementation iteration

- Task ID and title: F09-B.2 — debug navigation overlays over imported
  city geometry (F09-AC04), aimap override application, and a
  turn-classification reconciliation report on retail data.
- Starting commit and resulting commits: started at
  `0d884825cadeb0d874fc1eb220c82656dc866d2f` (clean tree, branch
  `ralph/night`, F09-B.1 externally checked); result = the commit on
  top of it.
- Why this slice: PLAN named F09-B.2 next — F09-AC04 was the only F09
  acceptance criterion without any evidence, and the B.1 review noted
  aimap `[Exceptions]`/`[Speed Limit]` were parsed but never applied
  and that `ccw_delta` awaited reconciliation against geometry.
- Production code changed:
  - `crates/mm2_game/src/nav.rs`: `NavOverrides` — the contract the
    content producer fills for routing/diagnostics consumers.
    Zero-density `[Exceptions]` rows close roads to ambient routing
    (inferred semantics — documented in the type docs), `[Speed
    Limit]` resolves per-road exception → file default → authored
    `base_speed`, `route_options()` feeds the existing `closed_roads`
    routing hook, out-of-range road ids kept verbatim. Also
    `NavGraph::route_roads` — road-index route probe shared by
    `mm2-inspect nav --route` and `--nav-route`. Self-review catch:
    the first version anchored each end at the arc's *centreline*
    midpoint, which sits equidistant between both travel directions;
    `nearest_lane`'s tie-break could then snap the dead-end-facing
    lane (the synthetic fixture returned `Unreachable{expanded:1}`).
    The shipped version anchors on the first lane's curve midpoint —
    a point on the lane resolves unambiguously to that lane.
  - `crates/mm2_game/src/config.rs` + `lib.rs`: `NavOverlay`
    (`route: Option<(u16,u16)>`) inside quarantined `DevOverrides`.
  - `crates/mm2_content/src/nav.rs` + `lib.rs`:
    `load_nav_overrides(vfs, path)` — absent aimap → `Ok(None)`
    (a modded city may not ship one), malformed →
    `NavLoadError::ParseAimap`, valid → `NavOverrides::from_aimap`.
  - `crates/mm2_app/src/nav_overlay.rs` (new): `CityNav` session
    resource (graph + build issues + overrides + probe result),
    `load_city_nav` (City world + `dev.nav_overlay` only; load
    failures log and the city still runs), `overlay_lines` — a pure
    segment builder classifying every drawable stroke (fwd/bwd lane,
    sidewalk, rail, direction chevron, aimap-closed, route probe,
    intersection marker) so geometry is assertable without a
    renderer, `hud_summary`, `draw_nav_overlay` (Bevy gizmos, lanes
    lifted 0.35 m / route 0.9 m above the surface).
  - `crates/mm2_app/src/session.rs`: `CityNav` inserted only after a
    successful city world load and removed with `RaceState` on
    teardown — nav state cannot leak into the next session.
  - `crates/mm2_app/src/main.rs`: `--nav` + `--nav-route from:to`
    CLI (route implies `--nav`; bad syntax exits 2; `--nav` on the
    dev world warns and draws nothing); draw system on its own
    `add_systems` (the Update tuple hit Bevy's arity limit); HUD
    `nav` summary suffix.
  - `crates/mm2_app/src/smoke.rs`: `nav=` field on the headless
    smoke record when `CityNav` is loaded.
  - `tools/mm2_inspect/src/main.rs`: `nav` gained `--turns` (per-city
    histogram of authored CCW road-index delta vs geometric turn
    kind, by intersection arity) and now applies the city aimap's
    `route_options()` to `--route` probes via `route_roads`.
- Research/documentation: `docs/research/bai.md` records the measured
  reconciliation — at 4-ways Δccw=1→right / 2→straight / 3→left for
  471/486 London (96.9%) and 963/968 SF (99.5%) exits, so the
  documented index arithmetic agrees with authored geometry exactly
  where it is documented; other arities mix (as expected — the
  scheme is only documented for 4-ways). Runtime consumption stays
  UNK-12.
- Retail findings reported by the audit/overlay (not repaired):
  - City aimaps carry no `[Exceptions]` rows (0 closed roads) and a
    15.0 `[Speed Limit]` default on both cities.
  - Route probes through `route_roads` + aimap options reproduce the
    B.1 baseline exactly: SF `13→50` = 4 steps/576 m
    `13- → 12- → 130+ → 50-`; London = 30 steps/1948 m.
  - `--turns`: 1106 London + 1401 SF exits histogrammed; the 4-way
    correlation above; 3-way deltas mix all three kinds; 5-ways are
    sparse and noisy.
- Tests added and why:
  - `mm2_game/tests/nav.rs` (+2, now 22): zero-density exceptions
    close roads to transit routing while nonzero density does not;
    speed-limit precedence (exception → file default → base speed).
  - `mm2_content/tests/nav.rs` (new, 5): missing BAI → Resolve
    error; absent aimap → `None`; exceptions/speed-limit distil into
    overrides; malformed aimap → error not empty overrides; graph
    loads through the VFS.
  - `mm2_app/tests/nav_overlay.rs` (new, 7): lane segments classify
    by kind/direction and lift above the surface; chevrons point
    along each lane's travel direction; closed roads draw Closed;
    the route probe highlights both arcs' lanes at the taller lift;
    `load_city_nav` pulls graph+aimap+route through the real VFS;
    `None` without the flag/city/graph; a full session harness proves
    `CityNav` is inserted on city load and removed on quit.
- Commands actually run and results (this machine, macOS arm64):
  - `cargo fmt --all -- --check` — PASS.
  - `cargo clippy --locked --workspace --all-targets --all-features
    -- -D warnings` — PASS (one `too_many_arguments` on `update_hud`,
    resolved with a targeted allow + explanatory comment per
    AGENTS.md).
  - `cargo test --locked --workspace` — PASS, 31 test-result groups,
    0 failures.
  - `mm2-inspect nav <retail> --route 13:50 --turns` — exit 0;
    numbers above.
  - `mm2 --mm2-path <retail> --city sf --headless --frames 120 --nav
    --nav-route 13:50` — `status=pass`, record includes
    `nav=618a/1212l closed=0 route=4st/576m`.
  - `mm2 --mm2-path <retail> --city london --headless --frames 300
    --nav` — `status=pass`, `nav=606a/1141l closed=0`. (A 30-frame
    run reported `never grounded` — London spawn settling, unrelated
    to nav; 300 frames grounds 4/4.)
  - `mm2 --mm2-path <retail> --city sf --cam=-747.5,42.4,275.0,179,-15
    --frames 90 --screenshot screenshots/nav-sf.png --nav
    --nav-route 13:50` — `status=pass`, 4.5 MB PNG awaited:
    amber route highlight on the probe's arcs + an intersection
    cross; HUD `nav 618a/1212l closed=0 route=4st/576m`.
  - `mm2 --mm2-path <retail> --city sf --cam=-747.5,180,275.0,179,-55
    --frames 90 --screenshot screenshots/nav-sf-high.png --nav` —
    `status=pass`, 6.3 MB PNG: lane polylines on every street,
    direction chevrons (opposing arrows on two-way streets), yellow
    intersection crosses, purple rail curves on the cable-car
    street.
  - `mm2 --mm2-path <retail> --city london
    --cam=99,150,-177,0,-50 --frames 90 --screenshot
    screenshots/nav-london-high.png --nav` — `status=pass`, 6.3 MB
    PNG over Trafalgar Square: lanes + chevrons on the *left* side
    (London's authored left-hand data), intersection markers, rail
    curves. HUD `nav 606a/1141l closed=0`.
- Acceptance IDs satisfied / still open:
  - F09-AC04 — debug overlays for sampled original roads in both
    cities match imported geometry and intended travel direction:
    SATISFIED at rendered/original-data level (both stock cities,
    overlay lines trace authored lane curves, chevrons follow
    authored direction incl. London left-hand, route + closure
    classes exercised synthetically). Candidate pending external
    check.
  - F09-AC01/AC03/AC05/AC06 — unchanged (synthetic level, B.1).
  - F09-AC02 — unchanged (parser/audit level, A.1/A.2); this slice
    adds aimap → consumer plumbing on top.
  - F09 parent: all ACs now carry some evidence; parent stays
    `implemented`/candidate — runtime consumption by traffic
    (F10)/opponents (F15)/police (F20) remains unbuilt and the
    original's runtime semantics unverified (UNK-12).
- Stock data/GPU/audio/network limitations: GPU/render evidence now
  exists for the overlay on both cities (local PNGs, not committed —
  retail content). Still no runtime consumer drives on the graph;
  `NavOverrides` "zero density = closed" is an inference labelled as
  such; aimap `density`/`speed_limit` units unverified (UNK-18 area);
  no audio/network code.
- Unresolved blockers or discovered regressions: none introduced.
  (Noted for later: `--turns` shows 3-way Δccw=0 exits exist —
  ends that reference their own slot's index; carried in the data,
  worth a future look when the original's runtime is understood.)
- Next smallest useful action: F13-A (checkpoint rules; deps
  F02-B/F11-B candidates) or F03-A (prop audit) or F09-C. F10-A
  (ambient traffic) is the first real graph consumer — its deps now
  include this B.2 plumbing.

This is a candidate handoff. External code-gate and separate review results live in the runner state directory and are not implied by this report.
