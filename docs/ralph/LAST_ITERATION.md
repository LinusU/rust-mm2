# Last implementation iteration

- Task ID and title: F10-B.7 — authored traffic-signal indicators
  (the F10-B plan remainder's "signal-prop rendering" line). The BAI
  `trafficLightOrigin`/`trafficLightAxis` road-end pairs were parsed
  but unconsumed; this slice surfaces them as session-owned lamps
  driven by the authoritative `Junctions` controller.
- Starting commit: `76ce14d5e1d82ae542579added0825e013578eb6` on
  `ralph/night`; tree was clean, previous external review verdict
  pass (F10-B.6), so this is feature work, not a repair.

## What changed

- `mm2_game::nav` — `NavArc::exit_light: Option<NavSignal>` carries
  the downstream end's authored marker per approach; `NavGraph::
  signals: Vec<EndSignal>` (road, resolved junction, raw rule code,
  verbatim `NavSignal { origin, axis }`) carries the *full* authored
  head set. The distinction matters: an `mm2-inspect bai` census
  proved arc-exit-only traversal covers just 522/650 lit ends on SF
  and 440/828 on London — the rest sit on one-way upstream ends and
  arc-less (pedestrian/tram-only) roads, and the original draws them
  anyway (R3: lights render when the origin is nonzero). `nav_signal`
  drops zero/non-finite origins and sanitises a non-finite axis to
  zero; heads on unresolved ends list nowhere (none exist on the
  retail cities).
- `mm2_game::traffic` — `SignalAspect { Green, Red, Stop }` +
  `Junctions::signal_aspect`: `gate`'s rule admission minus the
  box-yield. A light member is green exactly while it holds the
  phase (red through the all-red clearance); `NeverStop`/unruled and
  non-member ends stay green; `StopSign` ends show `Stop` (the FCFS
  admission cannot be read off a lamp); `AlwaysStop` stays red.
- `mm2_app::traffic` — `TrafficSignal` component + one session-owned
  indicator per authored head inside `SIGNAL_MAX_DISTANCE` (60 m,
  designed sanity bound; `signals_dropped` counts outliers — 3 on
  retail SF at up to 686 m). Shared `Sphere` mesh and three unlit
  aspect materials (no `vasignalunit`/`vastopunit` geometry ships —
  textures only, so the indicator is a designed presentation, not an
  original claim). `drive_signals` runs after `maintain_ambient` in
  every `FixedLast` chain (app, smoke, test harness), swapping the
  material only on aspect change under the same authority/phase
  gate as the driver. `AmbientTraffic::signals`/`signals_dropped`
  counters; `sig=`/`sigd=` join the `traf=` smoke record (sig only
  when nonzero, sigd only when nonzero).
- `tools/mm2_inspect` — the `bai` audit gains the traffic-light
  census (lit ends by rule, non-finite count, Y range, distance
  outliers, unconnected count, arc coverage classification, a sample
  coordinate) used to size this slice.

## Evidence

- `cargo test -p mm2_game --test traffic` — 31 pass (+3:
  `exit_light_carries_the_authored_marker_verbatim` — verbatim
  origin/axis on the arc, zero/non-finite origins → `None`,
  non-finite axis → `[0;3]`; `signals_lists_every_lit_resolved_end`
  — entry-only and arc-less lit ends list, unconnected lit end
  skipped, member set still arcs-only;
  `signal_aspect_follows_the_authoritative_phase` — 30-tick sweep:
  green iff member holds phase, both red in clearance, rule-table
  aspects, non-member fallback green).
- `cargo test -p mm2_app --test traffic` — 25 pass (+3:
  `authored_signals_spawn_at_their_origins_and_despawn_on_teardown`,
  `signal_heads_track_the_junction_phase` — never two greens, each
  member sees both aspects, all-red appears,
  `a_wild_signal_origin_drops_and_stays_counted`).
- `cargo fmt --all -- --check` — PASS.
- `cargo clippy --locked --workspace --all-targets
  --all-features -D warnings` — PASS.
- `cargo test --locked --workspace` — PASS, 673 tests, 0 failures.
- `mm2-inspect bai` (install `fnv1a64:e91e6cd4b2ae30d9`): SF 650 lit
  connected ends (533 light-ruled + 117 other-rule), 3 beyond 60 m
  (max 686.8 m); London 828 (519 + 309), max 39.7 m; arc-exit
  coverage 522/440, entry-only 96/166, arc-less 32/222.
- Retail headless smoke (same install):
  - `--city sf --frames 600` → `status=pass … traf=16/16 sp=23
    rec=7 dead=0 uns=0 q=0 jq=5 stuck=0 kn=1 sig=647 sigd=3` —
    650 authored − 3 outliers, matching the census exactly.
  - `--city london --frames 600` → `status=pass … traf=16/16
    sp=20 rec=4 dead=0 uns=0 q=0 jq=4 stuck=0 kn=1 sig=828` —
    all 828 authored heads spawn.
  - `kn=1` and `jq=` hold their F10-B.6 values — the indicators add
    no behaviour change to the driving path.
- Rendered capture (local, not committed): `--city sf
  --cam=-1799,36,-2374,40,-19 --frames 90 --screenshot` shows a
  green lamp standing at the authored kerb-side anchor of a lit
  junction approach on retail geometry.

## Still open

- Everything visual about the indicator is designed, not original:
  the unlit sphere at the authored anchor, the green/red/amber
  aspect mapping, the 60 m outlier bound, and the stop-signed amber
  choice. The original's signal-unit draw (no unit geometry ships;
  `vasignalunit`/`vastopunit`/`s_trafficlightr`/`s_stoplight_f` are
  textures only — the head may be drawn from PSDL junction geometry
  or textured quads) and its exact state semantics are unverified
  (UNK-12). `trafficLightAxis` is preserved verbatim but unused —
  its convention is unverified.
- Heads on ends no vehicle arc approaches (388 London / 128 SF)
  show the non-member green fallback — physically plausible (they
  govern no traffic) but not verified against the original.
- AC02's signal leg is now presentation-covered but remains
  behaviour-tested only through `gate` — no rendered/manual
  observation of the phase change on retail. No signal-pole prop
  model exists; a closer unit (pole + head) would need the axis
  convention verified first.
- F10-B remains active: queue-through-intersection priority,
  multiplayer union-of-interest, collision fidelity vs AC03's full
  checklist (player-hit feel, damage), original junction/spawn
  timing (UNK-12), F10-AC evidence vs the spec, F10-C.
