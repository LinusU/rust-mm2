# Authored-number audit — unchecked arithmetic and indexing on file-supplied values

Sweep of `rust-mm2-night` @ `ralph/night` (`fb81714`) for further, not-yet-reported
instances of the recurring defect class: arithmetic, casts, moduli and indices applied
to numbers that came out of authored data (retail CSV/binary/tune files, or mod content
mounted over them through the VFS). **11 confirmed-reachable findings** (14 code sites),
**5 speculative**. By finding count the worst module is
`crates/mm2_content/src/convert.rs` (2), but the highest-leverage *root cause* is the
un-validated tune scalar readers — `crates/mm2_formats/src/veh.rs`'s
`req_f32`/`opt_f32`/`req_i64` over `crates/mm2_formats/src/tune.rs:425`'s bare
`text.parse::<f64>()`, which accept `nan`, `inf` and the whole `i64` range with no
finiteness or plausibility check and feed **5 of the 11** findings. The most severe
single site is `crates/mm2_app/src/opponents.rs:604`, which does not panic at all — it
hangs. **Seven findings sit next to a doc comment, constant doc or test name that claims
the malformed case degrades to a diagnostic**; those contradictions are listed inline and
summarised at the end.

Build profiles matter for severity. The workspace sets no `overflow-checks` or
`debug-assertions` override (only `opt-level`), so dev/test get `overflow-checks = true`
and `debug-assertions = true` — for dependencies too — while release gets both off.
**An infinite loop, an index-out-of-bounds, and `f32::clamp`'s `assert!(min <= max)` all
fire in *both* profiles**; those are ranked first. (`f32::clamp` uses `const_assert!`,
not `debug_assert!` — `core/src/num/f32.rs:1565-1572` in the pinned toolchain.)

**Method and limits.** Every finding was traced from the expression back to the parser
function and field that produces the value, and every candidate was checked for an
existing guard before being listed — including whether a `validate()` on the path is a
real gate or merely reports diagnostics. Rejected candidates are recorded below so the
next pass does not re-walk them. This was static reading only: **no finding here was
reproduced by executing code**, because every reproduction needs a hand-built malformed
file and this pass was read-only. Findings 1–4 are the ones worth turning into
regression tests first; finding 2 is one line away from a test, because
`crates/mm2_formats/src/bai.rs:1105` already *constructs* the exact `Bai` that triggers
it.

**Status (post-sweep iterations on `ralph/night`).** All eleven
findings are fixed and covered by regression tests; the per-finding
`Status` lines below name the change. Findings 5, 6, 8, 10 and 11 were
closed in the second repair iteration; the speculative list (S1–S5) and
the F19-A.4 follow-up caveat below remain as recorded. A third pass
(iteration 004) extended finding 11's contract to every other consumed
field in the `camTrackCS`/`camPovCS`/`asNode` family, and a fourth
(iteration 005) hardened it from component-finite to a designed
magnitude bound (`USABLE_BOUND`) — see the finding-11 follow-up
notes. A fifth (iteration 006) carried the same bound to the
`.mtx`/pkg-geometry family at `build_model`, the sole producer of
`ModelPart.origin`/`pivot`, `WheelVisual.origin` and `body_aabb`:
`Mtx::validate()` names each unusable field, gated fields read
unauthored, and hostile pkg vertex measurements (wheel extents,
geometry-centre fallbacks, body bounds) are excluded the same way.
Retail audit: all 819 `.mtx` records (9,828 components across every
archive, city props included) — zero non-finite, max `|v|` = 2566.79,
so the bound rejects nothing authored. `.bnd` bound data and record
families outside `build_model` keep verbatim readers. One deliberate
deviation from a suggested fix shape: finding 7's
plausibility bound landed in `veh.rs` decode (`gear_count` /
`MAX_GEARS`), not in `convert()` — the decode boundary keeps the
authored value in the error message instead of a saturated `u32::MAX`,
and it bounds `ManualNumGears`'s identical cast for free. The
`vehCarDamage`/`vehStuck`/`vehGyro` records keep verbatim readers by
design: they decode non-finite values and report them through
`validate()` (`check_f32`/`DamageIssue::NonFinite`), so the new finite
readers (`req_finite_f32`, `opt_finite_f32`, `req_finite_vec3`,
`opt_finite_vec3`) apply only to the validate-less records
(`vehCarSim`, `vehTrailer`, `aiVehicleData`, `asNode`).

**Coverage gap — concurrent writer.** While this sweep ran, a separate autonomous
iteration was editing the same checkout and added ~1067 uncommitted lines of F19-A.4
skinned-`.mod` work to `crates/mm2_game/src/ped.rs`, `crates/mm2_formats/src/ped.rs` and
`tools/mm2_inspect/src/peds.rs`. Everything below reflects the **committed** state at
`fb81714`; that new pedestrian-mesh code is *not* covered. A spot check of its one
obvious candidate — `PedSkin::deform`'s `world[c.bone as usize]` — found it correctly
gated by `world.len() < self.bones_needed` plus a finite-transform check, so nothing is
reported there, but it needs its own pass once it lands.

**Follow-up (post-landing, F19-A.4 review).** That pass happened at review: the new
`matrix_bucket` (`crates/mm2_game/src/ped.rs`) accumulated the full-range authored
`mtxv`/`mtxn` counts with a plain `at += count` — overflow-panic under
`overflow-checks`, wrong-but-in-range bone binding in release. The cursor now
saturates (`at = at.saturating_add(count)`), which still buckets every in-range
index correctly: `index` is always an already-range-checked resource index, so the
first bucket whose running total saturates owns every not-yet-claimed index. The
same class in pre-existing `PedMod::validate` — six `iter().sum()` accumulations
over authored i64s (the `mtxv`/`mtxn` partition pre-checks, the three
`claimed_*` material-count sums, and the trailer `sums to` check) — is repaired
alongside it by accumulating in `i128`, keeping the diagnostics' printed totals
exact. `mm2-inspect peds` no longer panics on `mtxv 1 9223372036854775807 1`.
Tests: `skin_buckets_indices_past_a_saturating_mtxv_count` (`mm2_game::ped`),
`mod_validate_survives_unbounded_authored_counts` (`mm2_formats::ped`),
`audit_survives_hostile_mod_partition_counts` (`mm2_inspect::peds`). The rest of
the F19-A.4 ped code still wants a dedicated sweep pass; only the summation class
has been checked.

---

## Confirmed reachable

### 1. `crates/mm2_app/src/opponents.rs:604` (loop body 601-629) — `reanchor_pose` spins forever on a degenerate closed route

```rust
let mut remaining = REANCHOR_BACK;          // 601, = 4.0
let mut walked = 0.0f32;
loop {
    while remaining > 0.0 {                 // 604
        if remaining <= d { … }
        else {
            remaining -= d;                 // 610   d == 0.0 → no progress
            walked  += d;                   // 611   d == 0.0 → no progress
            if leg == 0 && !closed { d = 0.0; remaining = 0.0; }   // escape disabled by `closed`
            else { leg = (leg + leg_count - 1) % leg_count; d = len(leg); }
        }
        if walked >= REANCHOR_WALK { remaining = 0.0; }             // 620  never fires
    }
```
with `let len = |i| { let (a, b) = geom(i); (b.x - a.x).hypot(b.z - a.z) };` (`:569-572`).

* **Trigger.** A **closed** route whose every leg has zero **XZ** length — i.e. every `.opp` anchor shares the same x/z (all-identical rows, an all-zeros placeholder, or a vertical stack; `len` ignores y). Then `d == 0.0` on every leg, so `remaining` stays `4.0` and `walked` stays `0.0` forever; the `leg == 0 && !closed` escape is switched off by `closed`, and `walked >= REANCHOR_WALK` (60.0) can never become true. `route_is_closed` (`:396-403`) is `points.len() > 1 && last-to-first XZ distance <= ROUTE_LOOP`, so all-coincident anchors are *always* classified closed.
* **Origin (traced parser → expression).** `.opp` text → `OppFile::parse` (`crates/mm2_formats/src/opp.rs:64`), each coordinate a bare `cell.parse::<f32>()` → `distill_route` (`crates/mm2_content/src/opponents.rs:283-298`) copies positions **verbatim with no dedup, degeneracy or finiteness check** → `OpponentSpec.route` → `driving_route` (`crates/mm2_app/src/opponents.rs:640-654`), which is `route.clone()` when no nav graph is bound and `NavGraph::densify_route` otherwise (that only *inserts* points, it never rejects a degenerate line).
* **Guarded?** No. `n == 0` and `n == 1` return early (`:549-554`); nothing checks leg length. The index arithmetic (`leg_count - 1`, `(i-1).min(...)`, `% leg_count`) is itself fine for `n >= 2` — the defect is purely the non-terminating walk.
* **Reachability.** Established end to end. Call sites: `crates/mm2_app/src/opponents.rs:1210` (opponent recovery) and `crates/mm2_app/src/scripted.rs:689` (the `--bot` scripted player). The gate is `REANCHOR_FRAMES = 900` frames without `REANCHOR_DIST` of displacement — which is exactly what a route collapsed to one point produces: the car drives to the single anchor, parks on it, and 15 s later the re-anchor fires and hangs.
* **Severity.** **Infinite loop — a frozen fixed-update schedule, in both profiles.** No panic, no unwind, no diagnostic, no log line. Under the project's diagnose-not-panic rule this is the furthest possible outcome from the contract: the malformed row produces neither a diagnostic nor even a crash.
* **Doc contradiction (three separate claims).**
  * `opponents.rs:184-187` (`REANCHOR_WALK`): "past this the landing point stands wherever the walk reached … **disclosed, not silently unbounded**".
  * `opponents.rs:193-196` (`REANCHOR_CLEAR`): "a route jammed solid still lands **bounded** (disclosed)".
  * `reanchor_pose`'s own doc (`:597-599`): the walk is "**bounded** by `REANCHOR_WALK` and, for an open route, the start point".
  * And the class *was* anticipated elsewhere: `route_target`'s doc (`:410-411`) says "the bounded retry keeps a degenerate all-in-reach route from looping forever", and `crates/mm2_game/src/traffic.rs:628` caps its transfer walk with `for _ in 0..64` / "a degenerate graph must terminate". The same input is handled at two neighbouring sites and unhandled here.
* **Fix shape (diagnose-not-panic).** Cap the walk by leg count as well as by distance (an iteration bound like `traffic.rs:628`'s), and — the real fix — reject the route at distillation: raise an `OpponentIssue` for a route whose total XZ length is ~0 or whose anchors are non-finite, so the roster reports the `.opp` unusable and the opponent is not spawned. Do **not** silently substitute the first anchor and carry on; that hides an `.opp` the modder needs to fix.
* **Status: fixed.** `REANCHOR_MAX_STEPS` bounds the walk in `reanchor_pose`, which also returns the input pose rather than a non-finite candidate; `OpponentRoute::drivable` (`mm2_game::opponent`) rejects non-finite/XZ-collapsed routes at distillation into `OpponentIssue::DegenerateRoute`, keeping the authored slot with no wired route. Tests: `reanchor_pose_bounds_a_collapsed_closed_route`, `reanchor_pose_bounds_a_nonfinite_route` (`mm2_app/tests/opponents.rs`), `a_degenerate_route_is_reported_not_wired` (`mm2_content/tests/opponents.rs`).

### 2. `crates/mm2_game/src/nav.rs:1183` (panic lands at `nav.rs:2289`) — unguarded union-find over authored `.bai` road indices

```rust
// nav.rs:1179-1184
let mut parent: Vec<usize> = (0..bai.roads.len()).collect();
for int in &bai.intersections {
    for w in int.roads.windows(2) {
        union(&mut parent, w[0] as usize, w[1] as usize);
    }
}
// nav.rs:2288-2293
fn find(parent: &mut Vec<usize>, i: usize) -> usize {
    if parent[i] != i { ... }
```

* **Origin.** `Intersection::roads: Vec<u32>` (`crates/mm2_formats/src/bai.rs:257`), filled verbatim by `parse_intersection` from the `.bai` byte stream. Its own doc says the values "index into `Bai::roads`". `Bai::parse` performs **no range check** on them.
* **Guarded?** No. `parent` is sized `bai.roads.len()`. Note the contrast 25 lines above at `nav.rs:1155`, where the *same* authored value is read defensively with `bai.roads.get(s_idx as usize)`; here it indexes `parent` directly.
* **Reachability.** Established. `mm2_content::nav::load_nav_graph` (`crates/mm2_content/src/nav.rs:45-53`) resolves `city/<name>.bai` through the VFS (mods mount above stock), parses it, and calls `NavGraph::build(&bai)` with **no `validate()` gate**. `Bai::validate` already has a `BaiIssue::DanglingIntersectionRoad` variant for exactly this input, and `crates/mm2_formats/src/bai.rs:1105` is a test named `validate_catches_dangling_references` that builds a `Bai` with `intersections: vec![(3, vec![0, 5])]` against one road — handing that same `Bai` to `NavGraph::build` panics in `find` with "index out of bounds: the len is 1 but the index is 5".
* **Severity.** Index-out-of-bounds — **panics in debug and release**, deterministically at city load. Equally aborts `mm2-inspect`'s nav audit on the file it was asked to diagnose.
* **Doc contradiction (two claims).**
  * `nav.rs:862-866`: "Structural problems are reported in `NavBuild::issues`; **the graph itself is always produced**, degrading bad ends to dead ends and bad lanes out of their arcs rather than inventing connectivity."
  * `crates/mm2_content/src/nav.rs:42-44`: "Structural problems inside the file are reported on `NavBuild::issues`, **never hidden**."
* **Fix shape.** Skip a pair whose either index is `>= bai.roads.len()` and push a `NavIssue::DanglingIntersectionRoad { intersection, road }` so the component census reports the file as unusable. Do **not** clamp or `%`-wrap the index: that silently welds unrelated road components together and makes `stats.components` a plausible lie.
* **Status: fixed.** Exactly the suggested shape — `NavIssue::DanglingIntersectionRoad` plus bounds-checked `union` calls; valid arcs still build. Test: `a_dangling_intersection_road_is_an_issue_not_a_panic` (`mm2_game/tests/nav.rs`).

### 3. `crates/mm2_app/src/city.rs:2573-2580` (`emit_strip`) — authored PKG triangle indices reach `Collider::trimesh` and `compute_normals` unchecked

```rust
for t in strip.indices.as_chunks::<3>().0 {
    b.tri(base + t[0] as u32, base + t[1] as u32, base + t[2] as u32);
    col.tris.push([col_base + t[0] as u32, col_base + t[1] as u32, col_base + t[2] as u32]);
}
```

* **Origin.** `PkgStrip::indices: Vec<u16>` read verbatim by `parse_geometry` (`crates/mm2_formats/src/pkg.rs:400-405`). `pkg.rs` caps the *counts* (`n_vertices`/`n_indices` at `1 << 22`) but **never checks an index against `vertices.len()`**, and `Pkg` has no `validate()` at all (confirmed).
* **Guarded?** No. Compare `crates/mm2_formats/src/bnd.rs:292-307`, where the project's own `.bnd` parser rejects "vertex index {i} out of range ({nverts} verts)" at parse time — the discipline exists, it is just absent from `pkg.rs`.
* **Reachability.** Established: `PropCache::build` (`city.rs:2684-2712`) resolves `geometry/<name>.pkg` through the VFS, `Pkg::parse`, then `pkg_to_parts` → `emit_strip` → `PropCollision::into_collider` (`city.rs:2395-2400`) → `Collider::trimesh(positions, tris)`.
* **Severity — two independent panics, both profiles.**
  * Collider: `avian3d-0.7.0/src/collision/collider/parry/mod.rs:874` is `try_trimesh(...).unwrap_or_else(|error| panic!("Trimesh creation failed: {error:?}"))`, and `parry3d-0.27.0`'s BVH build indexes `self.vertices[idx[0] as usize]` (`src/shape/trimesh.rs:1665-1672`, `:1881-1888`) → index-out-of-bounds.
  * Render: when the PKG's FVF carries no vertex normals, `emit_strip` takes the `b.vert(p, uv)` branch, `MeshBuilder::build` (`city.rs:362-377`) leaves `ATTRIBUTE_NORMAL` unset and calls `mesh.compute_normals()` **after** `insert_indices`, and `bevy_mesh-0.19.1/src/mesh.rs:1403-1405` does `positions[a]` straight off the index buffer.
  * An out-of-range index that happens to land inside the accumulated `positions` buffer (multi-strip chunks) instead silently draws — and collides with — the wrong vertex.
* **Doc contradiction.** `crates/mm2_app/src/navarrow.rs:314-320` walks the *same* `PkgStrip` data and guards it explicitly: "File-supplied indices index the strip's own vertex table — a corrupt or hostile pkg (the VFS mounts mod overrides above stock) **must fail the spawn, not panic inside it**" → `return Err("bad-index")`, with a test at `crates/mm2_app/tests/navarrow.rs:533`. `navarrow.rs:237` even says it rasterizes "the same chunks `city.rs` `pkg_to_parts` would draw" — the production builder is the one missing the check.
* **Same missing check, milder sibling — `crates/mm2_content/src/model.rs:261-262.`** `g.indices.extend(t.iter().map(|&i| base + i as u32))` does the identical unchecked widening for *vehicle* PKG parts. It is not a panic today only because `model.rs:249` pushes `v.normal.unwrap_or([0.0, 1.0, 0.0])` for every vertex, so `car_visual.rs:92` always finds matching normal/position lengths and never reaches `compute_normals`, and the vehicle collider comes from the `.bnd`/AABB convex hull rather than the mesh. Result: a silently mis-shaped car body. There are exactly three PKG-strip consumers in the tree (`city.rs:2488`, `model.rs:240`, `navarrow.rs:309`) and only the last one checks — which is the argument for fixing it once in `pkg.rs::parse_geometry`.
* **Fix shape.** Add the range check to `parse_geometry` so every consumer inherits it, returning `FormatError::InvalidValue` for the strip; or reject the strip in `emit_strip` and count it into the existing `missing_prims` report. Do **not** `.min(vertices.len()-1)` the index — that yields a silently mis-shaped collider the player drives into.
* **Status: fixed.** `parse_geometry` range-checks each `PRIMTYPE_TRIANGLES` index against the strip's vertex count (the only prim type observed on retail and the only one consumers interpret); the error degrades the chunk to `PkgChunk::Raw` — the module's documented corrupt-geometry path — now with a `tracing::warn!` naming the reason, and `mm2-inspect scan` reports such chunks as `partial`. Test: `out_of_range_triangle_indices_degrade_to_raw` (`mm2_formats::pkg`).

### 4. `crates/mm2_content/src/convert.rs:588` — NaN `SteeringLimit` becomes a `clamp` bound

```rust
let low_angle = sim.wheel_front.steering_limit.clamp(0.05, 0.7);                       // :586 NaN passes through
let high_angle = (low_angle * (1.0 - sim.wheel_front.steering_offset * 0.8))
    .clamp(0.04, low_angle);                                                           // :588 NaN as the max bound
```

* **Origin.** `steering_limit: req_f32(b, ctx, "SteeringLimit")?` in `decode_wheel` (`crates/mm2_formats/src/veh.rs:255`), reading the `vehWheel` block of a `tune/vehicle/<id>.vehcarsim`. `req_f32` (`veh.rs:59-67`) is `Some(n) => Ok(n as f32)` over `TuneValue::number`, itself `v.text.parse::<f64>().ok()` (`tune.rs:425`). Rust's `f64: FromStr` accepts `nan`/`inf`/`-inf`, so authored `SteeringLimit nan` parses cleanly. **No `is_finite()` check exists on this field anywhere** — `veh.rs`'s `check_f32`/`DamageIssue::NonFinite` machinery covers only the `vehCarDamage` block.
* **Guarded?** No. Line 586's clamp has constant bounds so its own assert holds, and it *propagates* NaN (`NaN < 0.05` and `NaN > 0.7` are both false). Line 588 then passes that NaN as `max`, and `0.04 <= NaN` is false.
* **Reachability.** Established, and *before* the validate gate: `crates/mm2_content/src/assemble.rs:382` calls `convert(&input)`; only at `:383` does it run `config.validate()`. `assemble::load_by_id`/`load_vehicle` run at startup (`crates/mm2_app/src/main.rs:627`, `:646`, `:1656`), from the car picker (`crates/mm2_app/src/menu.rs:892`), for opponents (`crates/mm2_app/src/opponents.rs:685`) and in the audit tool (`tools/mm2_inspect/src/main.rs:4397`, `:4696`).
* **Severity.** `assert!` inside `f32::clamp` — **panics in debug and release**, and aborts `mm2-inspect`'s vehicle audit on the very file it was asked to diagnose.
* **Fix shape.** `convert()` already returns `Result<_, String>`; reject the vehicle naming `SteeringLimit`. Better: make `req_f32`/`opt_f32` in `veh.rs` reject non-finite numbers the way `check_f32` already does for the damage block, so every `.veh` float inherits it — that single change also covers findings 7 and 9. Do **not** substitute a default: a car whose steering limit silently became `0.35` is a wrong-but-drivable car.
* **Status: fixed.** With a boundary refinement: `vehCarDamage`/`vehStuck`/`vehGyro` deliberately decode non-finite values verbatim so `validate()` can report them (`DamageIssue::NonFinite`, and an existing test asserts `MedDamage NaN` decodes) — tightening `req_f32` globally would have broken that contract. Instead the validate-less records (`vehCarSim`, `vehTrailer`, `aiVehicleData`, `asNode`) read through new finite readers (`req_finite_f32`, `opt_finite_f32`, `req_finite_vec3`, `opt_finite_vec3`). Tests: `vehcarsim_rejects_non_finite_scalars`, `vehcarsim_vec3_fields_must_be_finite`, `aivehicledata_rejects_non_finite_scalars` (`mm2_formats/tests/vehicle_formats.rs`).

### 5. `crates/mm2_app/src/session.rs:609`, `:620-624`, `:1096` — a non-finite authored start-slot pose reaches the player's rigid body

```rust
spawn.position = slot.position;                                           // :609
spawn.yaw = slot.yaw_deg.map(f32::to_radians)
    .or_else(|| def.course_yaw(slot.position)).unwrap_or(spawn.yaw);       // :620-624
spawn.position.y += (0.25 - hull_min_y).max(0.35);                         // :1075
Transform::from_translation(spawn.position)
    .with_rotation(Quat::from_rotation_y(spawn.yaw))                       // :1096, alongside vehicle_bundle → RigidBody::Dynamic
```

* **Origin.** `StartPointsFile::parse` (`crates/mm2_formats/src/waypoints.rs:205`) reads `x,y,z,a` through `parse_f32` (`waypoints.rs:91-102`) — a bare `cell.parse()` that accepts `nan`, `inf`, `-inf` and overflowing literals like `1e999`. `authored_start_slots` (`crates/mm2_content/src/race_def.rs:311-328`) copies the position verbatim, and sets `yaw_deg: (p.angle_deg != 0.0).then_some(p.angle_deg)` — **`NaN != 0.0` is true**, so a NaN angle passes the "zero means unset" test and becomes `Some(NaN)`.
* **Guarded?** No, and the gate that exists does not cover it. `race_def.rs:162` calls `definition.validate()?` (a real gate — failure becomes `RaceBuildError`), but `RaceDefinition::validate` (`crates/mm2_game/src/race.rs:362-381`) checks only: non-empty checkpoints, `laps != 0` on Ordered, `time_limit_ticks != Some(0)`, and the **`radius`/`height` finiteness of checkpoint extents**. It never touches `start_slots[*].position`, `start_slots[*].yaw_deg`, or `checkpoints[*].center`. Two incidental NaN-swallowers exist but protect the wrong operand: `:1075`'s `.max(0.35)` drops a NaN *`hull_min_y`* but `NaN + 0.35` is still NaN; `course_yaw` is NaN-tolerant by accident (`NaN > 4.0` is false → `None` → fallback), so the yaw NaN arrives only via `Some(NaN)`.
* **Severity.** **Panic in debug/test; silent NaN simulation in release.** `avian3d-0.7.0/src/schedule/mod.rs:118-123` registers `assert_components_finite` in `PhysicsSystems::First` under `#[cfg(debug_assertions)]`, whose body is `debug_assert!($val.is_finite(), "NaN or infinity found in Avian component: type=Position …")` (`:296-309`); Avian syncs `Transform`→`Position` for the new body, so the first physics step after the spawn panics. Cargo's `debug-assertions` default is on for the workspace *and* dependencies here (only `opt-level` is overridden), so this fires in a normal `cargo run`/`cargo test`. In release the NaN pose spreads to the recovery anchor (`:1110-1114`), the chase camera (`:1020`), `spawn_mirror` (`:1121`), every opponent staged off the same pose (`:1405`), the ambient interest point (`:1333`) and the trailer reset offsets (`:413-427`).
* **Reachability.** Established: a mod `<event>_strtpnts` row of the form `nan,0,0,0,0,msg` or `1e999,0,0,0,0,msg`.
* **Doc contradiction (three claims).**
  * `crates/mm2_game/src/race.rs:361`: "Reject a definition that would behave oddly at runtime; the producer validates before inserting `RaceState`." The one authored value in the definition that reaches the physics engine is unchecked.
  * `crates/mm2_content/src/race_def.rs:340-344` (`RaceDefBuild::Failed`): "The producer rejected the event — … an **out-of-range authored parameter**, too few waypoint rows, or a definition that failed validation." A non-finite start point is exactly that, and is neither rejected nor reported.
  * `crates/mm2_app/src/session.rs:30-31`: "a failed load can never leave a live player simulation (AC02)." A NaN slot is not a failed load; it produces a live NaN-posed simulation.
* **Fix shape.** Add `start_slots[*].position`/`yaw_deg` and `checkpoints[*].center` finiteness to `RaceDefinition::validate` so the existing `RaceBuildError` path reports the event unusable. Do **not** zero or clamp the coordinate — a car silently teleported to the origin is a wrong-but-plausible race.
* **Status: fixed.** `RaceDefinition::validate` gained two variants — `RaceError::NonFiniteGate` (a checkpoint or finish `center`/`heading_deg` non-finite) and `RaceError::NonFiniteStart` (a start slot's `position` or authored `yaw_deg` non-finite) — and the producer's existing `definition.validate()?` (`race_def.rs:162`) turns either into a `RaceBuildError::Invalid`, so the event reports unusable instead of spawning a NaN-posed body. Test: `definition_validation_rejects_non_finite_authored_values` (`mm2_game/tests/race.rs`).

### 6. `crates/mm2_game/src/props.rs:796` — `+ 1` on a saturated `f32 → u64` cast of an authored prop offset

```rust
let want = if def.start <= curb_len {
    (((curb_len - def.start) / def.distance) as u64) + 1
} else { 0 }
.min(def.max_use as u64);
```

* **Origin.** `PropDef::start`/`distance` are `parse_f32(cells[1] / cells[2], ...)` from `propdefs.csv` (`crates/mm2_formats/src/proprules.rs:403-404`); `parse_f32` (`:329-340`) is a bare `cell.parse()` with no finiteness check, so `-inf`, `inf` and `nan` are accepted.
* **Guarded?** Partially, and not where it matters. The loop filters `def.distance <= 0.0 || def.files.is_empty() || def.max_use <= 0` (`props.rs:791`) — but **not `start`**. `PropDefs::validate` reports `PropRuleIssue::NegativeStart` (`proprules.rs:459-465`) yet is purely advisory: it never removes the row from `defs`. With `start = -inf` (or a finite `start` and a tiny `distance`) the quotient exceeds `2^64`, the Rust float→int cast **saturates to `u64::MAX`**, and `+ 1` overflows. (`nan` is safe — it fails `def.start <= curb_len`.)
* **Reachability.** Established: `walk_prop_rules` is driven by the city import from the authored `city/<name>.psdl` `prop_rule` byte plus `propdefs.csv`/`proprules.csv`, all VFS-resolved.
* **Severity.** Debug: integer-overflow panic during city load. Release: wraps to `0`, so `want = 0` and **every prop on that side of every room using that def silently disappears** — the wrong-but-plausible outcome the convention singles out as worse than a crash.
* **Doc contradiction (two claims, plus a correct sibling).**
  * The comment immediately above (`props.rs:791-794`): "Offsets `s = start + k·distance ≤ curb_len`, counted arithmetically **so a hostile def is measured against the budget instead of walked**."
  * `MAX_PROP_RULE_STAMPS`'s doc (`props.rs:96-99`): "a hostile table could request `maxUse` placements at near-zero `distance`; **overflow is counted, not silently dropped**." At near-zero `distance` the *count itself* overflows before the budget is consulted.
  * The correct pattern is in the same file: `path_stamp_sites` (`props.rs:1023`) does `let want = (f64::from(len) / f64::from(spacing)).ceil() as usize; let take = want.min(left);` — widened to `f64`, and **no `+ 1` after the saturating cast**. `crates/mm2_app/src/city.rs:3986` is a test named `a_hostile_segment_is_capped_instead_of_hanging` covering that sibling.
* **Fix shape.** Add `!def.start.is_finite() || def.start < 0.0` to the `props.rs:791` reject list and raise the existing `PropRuleIssue::NegativeStart` (plus a new non-finite issue) into `walk.stats.issues` so the row is reported unusable; then compute the count in `f64` like `path_stamp_sites` does.
* **Status: fixed.** Both halves landed. `walk_prop_rules` skips a def with non-finite `start`/`distance`/`lerp_min`/`lerp_max`, negative `start` or non-positive `distance`, and pushes a bounded `stats.issues` line naming the def and the values — before any arithmetic. The count is now computed in `f64`, bounded by `maxUse` *before* the truncating cast (`want.min(stamps_left)` unchanged), so a hostile span is measured against the budget instead of overflowing `+ 1` on a saturated cast. Upstream, `PropDefs::validate` gained `PropRuleIssue::NonFiniteField` naming `start`/`distance`/`minLerp`/`maxLerp` with the authored value verbatim; the `NegativeStart`/`NonPositiveDistance` comparisons stay finite-only so a NaN is not double-reported as both. Tests: `propdefs_non_finite_fields_are_named` (`mm2_formats::proprules`), `hostile_propdefs_skip_and_report_instead_of_stamping_nan` (`mm2_game::props`).

### 7. `crates/mm2_content/src/convert.rs:519` + `:530-531` — uncapped `AutoNumGears` sizes an allocation and a loop

```rust
// crates/mm2_formats/src/veh.rs:431
auto_num_gears: req_f32(trans_b, "vehCarSim.Trans", "AutoNumGears")?.max(1.0) as u32,
// crates/mm2_content/src/convert.rs:519, 530-531
let n_gears = sim.trans.auto_num_gears.max(1) as usize;
let mut gear_ratios = Vec::with_capacity(n_gears);
for i in 0..n_gears { … }
```

* **Origin.** `AutoNumGears` is read as an `f32` and saturating-cast to `u32`, so authored `AutoNumGears 1e12` or `inf` becomes `u32::MAX = 4294967295`. No plausibility cap anywhere — in pointed contrast to the `checked_count` / `MAX_ROWS` / `MAX_PATHS` / `MAX_ENTRIES` / `1 << 22` discipline that every binary parser in `mm2_formats` applies.
* **Guarded?** Only against zero (`.max(1)`), the harmless end.
* **Reachability.** Established — same `assemble` → `convert` path as finding 4, again *before* `config.validate()`.
* **Severity.** `Vec::with_capacity(4_294_967_295)` for `f32` requests ~17 GB; Rust allocation failure **aborts the process** (no unwind, no diagnostic). If it succeeds on a large machine, the loop runs 4.3 billion iterations of `powf`. Both profiles.
* **Fix shape.** Cap at a documented plausibility bound (retail authors ≤ 6) and `Err` out of `convert()` naming `AutoNumGears`. Do **not** silently `.min(6)`: a 6-speed config invented for a car whose file says 400 is a wrong-but-drivable car.
* **Status: fixed.** Bounded at decode instead of in `convert()`: `MAX_GEARS = 32` + `gear_count` reject `AutoNumGears`/`ManualNumGears` above the bound with the authored value in the error (a saturated `u32::MAX` would name nothing), and `req_finite_f32` rejects `nan`/`inf` before the cast. `convert()`'s `Vec::with_capacity(n_gears)` now sees a count ≤ 32. Test: `vehcarsim_bounds_gear_counts` (`mm2_formats/tests/vehicle_formats.rs`).

### 8. `crates/mm2_game/src/effects.rs:369` and `:823` — `end - start + 1` on raw authored `i64` flipbook bounds

```rust
// :367-371  SmokeEmitter::puff
let frame = if spec.tex_frame_end >= spec.tex_frame_start {
    spec.tex_frame_start
        + (e.rng.next_u64() % (spec.tex_frame_end - spec.tex_frame_start + 1) as u64) as i64
} else { spec.tex_frame_start };

// :822-823  PrecipDrop::frame
let span = self.frame_end - self.frame_start + 1;
```

* **Origin.** `tex_frame_start`/`tex_frame_end` are `req_i64(root, ctx, "TexFrameStart"/"TexFrameEnd")?` — `crates/mm2_formats/src/veh.rs:952-953` (the `vehCarDamage` smoke effect) and `crates/mm2_formats/src/banger.rs:342-343` (`.asbirthrule`). `req_i64` (`veh.rs:681-690`) accepts the **entire `i64` range** (`n.abs() <= i64::MAX as f64`). `ParticleSpec` carries both verbatim (`effects.rs:112-113`, `:168-169`), and `PrecipRig::drop` copies them into the drop unclamped (`effects.rs:742-743`).
* **Guarded?** The `end >= start` test rules out modulo-by-zero and a negative span, but **not the subtraction's own overflow**: `TexFrameStart -9223372036854775808` with `TexFrameEnd 0` satisfies `end >= start` and overflows `0 - i64::MIN`.
* **Reachability.** Established. Precip: `crates/mm2_app/src/precip.rs:153` parses `tune/rain.asbirthrule`/`snow.asbirthrule` from the VFS → `ParticleSpec::from(&parsed)` (`:167`) → `rig.drop(focus)` (`:263`) → `drop.frame()` (`:276`, `:356`). Smoke: `crates/mm2_app/src/damage_fx.rs:175` `rig.puff(idx, origin, entity)`. `precip.rs:206` clamps `tex_frame_end` when *sizing the atlas*, which is exactly why the raw value in the drop looks safe and is not.
* **Severity.** Debug: integer-overflow panic on the first particle. Release: wraps, and the drawn tile is a wrong-but-in-range value after `policy.tile()` clamps it.
* **Doc contradiction.** `crates/mm2_game/src/audio.rs:838-845` is the *already-repaired* instance of this exact shape, and its doc now promises: "`None` on a non-positive `end` or an `add`/`end` window that cannot fit `i64` — a modded table's `9223372036854775807`-scale fields are undrawable, **never an overflow panic**." The two `effects.rs` siblings were not brought along. (`effects.rs:1205`, `WheelPuff::frame`, *is* safe — but only incidentally, because its bounds went through `WheelPtxPolicy::tile`.)
* **Fix shape.** Mirror `draw_cue_suffix`: reject the window with `checked_sub`/`checked_add` and treat the spec as undrawable, surfacing it on the existing `PrecipReport`/smoke report. Clamping the *spec* would be a silent coercion; clamping is only correct at the final atlas lookup.
* **Status: fixed.** A shared `flipbook_span(start, end)` (`checked_sub` + `checked_add`) now backs all three sites. `VehicleSmoke::puff` and `Precipitation::drop` return `Option` — a window that cannot fit `i64` declines *before* drawing on the seeded RNG, so deterministic streams stay aligned; `PrecipDrop::frame` returns `Option<i64>` (`None` = undrawable). `WheelPuff::frame` was already safe-by-policy (`tile`-clamped bounds) and now shares the helper, pinning the start tile on a hand-built hostile pair. `SmokeFxReport`/`PrecipReport` gained `undrawable` counters surfaced in the headless record as `+Nu`; an inverted window still pins the start tile rather than declining. Tests: `unrepresentable_flipbook_windows_decline_without_drawing`, `precip_drop_frame_sweeps_the_authored_tiles` (extended), `precip_declines_an_unrepresentable_flipbook_window` (`mm2_game/tests/effects.rs`).

### 9. `crates/mm2_app/src/traffic.rs:807` (and `:742`) — the CG fallback is derived from an unchecked authored `Size`

```rust
let cg = tuning.cg
    .filter(|c| c.iter().all(|v| v.is_finite()))
    .unwrap_or([0.0, tuning.size[1] * 0.5, 0.0]);   // ← fallback unchecked
…
CenterOfMass(Vec3::from(cg)),                        // :825
Friction::new(tuning.friction),                      // :829  unchecked
Restitution::new(tuning.elasticity),                 // :830  unchecked
```

* **Origin.** `AiVehicleData::from_tune` reads these through `req_f32`/`req_vec3` over `TuneFile`, i.e. the same `tune.rs:425` `parse::<f64>()` path; `1e39 as f32` is also `inf`. `AiVehicleData` has **no `validate()` gate** anywhere on the path (`crates/mm2_content/src/traffic.rs:73` `load_tuning` → `AmbientSpec.tuning`). This is not hypothetical for this record family: `crates/mm2_formats/src/veh.rs:1152-1155` documents retail `va_garbagetruck` as already shipping **a NaN** in `MaxAng`.
* **Guarded?** Partially — `mass` (`:799`) and the *authored* `cg` are finiteness-filtered; the `Size`-derived fallback, `friction` and `elasticity` are not (`Friction::new`/`Restitution::new` carry no assert in avian3d 0.7 `physics_material.rs:172`). Same unchecked expression in `size_collider` (`:742`).
* **Reachability.** Established for the value reaching `CenterOfMass`/`Friction`. A record with an absent `CG` and a non-finite `Size` yields `CenterOfMass(NaN)`.
* **Severity.** Silent wrong value / solver poisoning. **Not** a panic — these are kinematic ambient bodies, and `assert_components_finite` (see finding 5) covers `Position`/`LinearVelocity`/`AngularVelocity`, not `CenterOfMass`.
* **Doc contradiction.** The comment immediately above (`traffic.rs:798`) claims "a degenerate/absent mass or CG falls back to **sane defaults**" — which the `Size`-derived branch and the two neighbouring material fields do not honour.
* **Fix shape.** Give `AiVehicleData` a `validate()` and make `load_tuning` a real gate that reports the record unusable — or, upstream, the `veh.rs` `req_f32` finiteness fix from finding 4, which covers this too. Note `banger.rs`'s `BangerDefinition::from_record` (`crates/mm2_game/src/banger.rs:155-187`) is the model: it launders every authored value at a single boundary.
* **Status: fixed** via the finding-4 fix — `AiVehicleData` now reads every required scalar through `req_finite_f32` and `Size` through `req_finite_vec3`, so a record carrying them fails decode in `load_tuning` and is reported rather than laundered into `CenterOfMass`; a non-finite `CG` warns and falls back like an absent one. `MaxAng` intentionally keeps the verbatim `opt_vec3`: retail `va_garbagetruck` ships a NaN there that is preserved by design (tested by `aivehicledata_decodes_msvc_non_finite_literals`).

### 10. `crates/mm2_formats/src/crashdata.rs:184-185` — authored integer columns round-tripped through `f32`, then saturating-cast

```rust
let mut num = |what: &str, cell: &str| match cell.parse::<f32>() { … };   // :140
let event = num("Event", cells[1]);                                       // :150
let checkpoints = num("Checkpoints", cells[2]);                           // :151
…
event: event as i64,                                                      // :184
checkpoints: checkpoints as i64,                                          // :185
```

* **Origin.** The `Event` and `Checkpoints` columns of `crash<N>data{,_p}.csv`.
* **Guarded?** No integrality test, no `is_finite()` test; the closure only diagnoses a cell that is not a float at all. So `2.7 → 2`, `1e30 → i64::MAX`, `16777217 → 16777216`, and **`nan → 0`**.
* **Reachability.** Established: `CrashDataRow.event` → `mm2_content::crashcourse::sub_event` (`crates/mm2_content/src/crashcourse.rs:463-464`) → `LessonObjective::from_code(row.event)`, where `0 => Self::Jump` (`crashcourse.rs:120`). An authored `nan` therefore decodes to a *valid-looking* lesson objective rather than `Unknown(_)`, and is printed as such by `tools/mm2_inspect/src/crashcourse.rs:104-112`.
* **Severity.** Silent truncation / silent misdecode. No panic in either profile; nothing allocates or indexes on it. Listed because it degrades the audit tool's own output — the one place meant to *report* the defect.
* **Doc/test contradiction.** `crashdata.rs:67-72`: "Malformed rows are skipped and recorded in `diagnostics`", and the test at `crashdata.rs:267` is named `malformed_rows_are_diagnostics_not_panics`. The *panic* half holds; the *diagnostic* half does not for these cells. The sibling table parser does it right: `crates/mm2_formats/src/racedata.rs:98-119` parses integer columns with `parse::<i64>()` and pushes a `TableDiagnostic` on failure.
* **Fix shape.** Parse `Event`/`Checkpoints` with `parse::<i64>()` like `racedata.rs` and diagnose + skip the row. (`TimeLimit`/`AmbDensity` on `:152-153` likewise accept `nan`/`inf`; every consumer traced is a `> 0.0` test or a `{:.2}` print, so they are not findings — but the same `is_finite()` diagnostic would make the doc claim true.)
* **Status: fixed.** `Event`/`Checkpoints` parse through a shared `int_cell` (`parse::<i64>()` → `TableDiagnostic` on failure) — a fractional, `nan` or out-of-range cell is now a diagnosed skip, and signed extremes decode verbatim. `TimeLimit`/`AmbDensity` parse through a shared `num_cell` that additionally diagnoses non-finite values, taking up the parenthetical suggestion. Tests: `integer_columns_reject_non_integral_cells`, `non_finite_decimal_cells_are_diagnostics` (`mm2_formats::crashdata`).

### 11. `crates/mm2_app/src/camera.rs:111`, `:735` and `crates/mm2_app/src/dash.rs:260` — authored `CameraFOV` reaches the projection with no range or finiteness check

```rust
fov_deg: spec.camera_fov.unwrap_or(70.0),                                   // camera.rs:111 → :143 .to_radians()
… pov.and_then(|p| p.camera_fov).unwrap_or(60.0).to_radians()               // camera.rs:735 (mirror camera)
fov: p.camera_fov.unwrap_or(60.0).to_radians(),                            // dash.rs:260 (cockpit)
```

* **Origin.** `TrackCamSpec::camera_fov` via `scalar()` (`crates/mm2_formats/src/camtrack.rs:106-110`) and `PovCamSpec::camera_fov` via `scalar()` (`crates/mm2_formats/src/dash.rs:129-133`) — both `TuneValue::number`, i.e. the same `tune.rs:425` `parse::<f64>()`. Neither `TrackCamSpec` nor `PovCamSpec`/`DashSpec` has a `validate()`, and the loaders only `.ok()` the parse (`camera.rs:162`, `spawn_dash`).
* **Guarded?** No — and the asymmetry is the point. The neighbouring fields on the *same struct literals* are guarded: `camera_near … .max(0.01)` / `.clamp(0.01, COCKPIT_NEAR_CAP)` and `camera_far … .max(1.0)`, and `COCKPIT_NEAR_CAP` carries a 13-line comment about defending against a bad authored `CameraNear`. `fov` got no such treatment.
* **Severity.** Silent wrong value / dead render, **not** a panic: `fov == 0` gives an `inf` clip matrix, `fov >= π` inverts the projection, `fov == NaN` gives a NaN matrix. No assert fires — glam's `glam_assert`/`debug-glam-assert` features are not enabled anywhere in the workspace, and bevy's only panic nearby (`bevy_camera-0.19.1/src/projection.rs:390`) is driven by viewport size and pre-guarded. Outcome: a cockpit or chase view that renders nothing on a car whose dash cluster otherwise binds fine. The same shape applies to `DashSpec`'s `wheel_fact`, `speed_rot`/`rpm_rot`/`damage_rot` (`dash.rs:568/573/580/585`) — also non-finite-capable, also NaN transforms, no panic.
* **Fix shape.** Reject a non-finite or out-of-range `CameraFOV` in a `validate()` on the spec and fall back to the *designed* lens (`ChaseLens::sized`) with a warning naming the file, rather than building a degenerate projection. A silent `.clamp(1.0, 179.0)` would hide a mod authoring the field in radians.
* **Status: fixed.** `camtrack::drawable_fov` (shared, `pub(crate)`) bounds `CameraFOV` to the open `(0, 180)` degree interval — finite required, so `nan`/`inf` are out. `TrackCamSpec` and `PovCamSpec` each gained `camera_fov_deg()` (reads an undrawable value as *unauthored*, so the designed `FOV`/`60°` default stands in) and `validate()`; `load_track_cams` and `load_pov_cam` warn each issue with the file path, matching the `for issue in spec.validate()` pattern used by the other loaders. The raw field stays verbatim — a mod with `CameraFOV` in radians is reported, not silently repaired. Tests: `undrawable_camera_fov_is_named_and_reads_unauthored` (`mm2_formats::camtrack`), `undrawable_authored_fov_falls_back_to_the_designed_lens` (`mm2_app` `camtrack`/`mirror`/`dash` test files).
* **Follow-up (iteration 004).** The same contract now covers every other consumed field in the family — the sibling-shape residual called out above (`DashSpec`'s `wheel_fact` and `*Rot*` sweeps) and the rest: `TrackCamSpec` gained `offset_vec`/`track_to_vec`/`collides`/`min_max_gated`/`min_dist_m`/`max_dist_m`/`min_speed_mps`/`max_speed_mps`/`camera_near_m`/`camera_far_m` and `validate()` names all sixteen fields; `PovCamSpec` gained `offset_vec`/`reverse_offset_vec`/`pitch_rad`/`camera_near_m`/`camera_far_m` (`nan` survives `f32::clamp`, and a `nan` `CameraFar` through `.max(1.0)` was a 1 m far plane); `DashSpec` gained its first `validate()` over the eleven placement vectors, `WheelFact` and the three needle sweeps. Consumers read accessors only — `ChaseLens::authored`, `spawn_mirror`, the cockpit camera and the `v3`/`rot`/`wheel_fact` filters in `spawn_dash` — and a `nan` flag (`CollideType`/`MinMaxOn`) reads *off*, not `!= 0.0` truthy. `spawn_dash` warns each `DashSpec` issue with the path. An astronomical-but-finite `Offset` whose derived length overflows also reads unauthored. Tests: `non_finite_fields_are_named_and_read_unauthored` (`camtrack`), `dash_spec_non_finite_fields_are_named`, `pov_cam_spec_non_finite_fields_read_unauthored` (`dash`), `non_finite_authored_fields_fall_back_to_the_designed_boom` (`mm2_app` `camtrack`), `non_finite_authored_pov_fields_fall_back` (`mm2_app` `dash`/`mirror`), `hostile_asnode_reads_unauthored` (`mm2_app` `dash` — synthetic `_dash.pkg` + hostile `_dash.asnode` through the real VFS). Retail: all 119 authored records (49 `camtrackcs`, 47 `campovcs`, 23 `_dash.asnode`) scanned — zero non-finite/f32-overflowing tokens in 4,550 fields, so the gates reject nothing authored. `PovCamSpec::track_to` is validated but still has no consumer (blend semantics unrecovered).
* **Follow-up (iteration 005, external-review residual).** The review flagged the asymmetric edge inside that contract: component finiteness does not stop *composed* results overflowing — a finite ~`3e38` `TrackTo` can still overflow `veh_rot * aim` into a non-finite look target (the quaternion product sums intermediate terms), a `3e38` `MaxDist` overflows `dir * dist`, `eye + v3(DashPos)` and the `pivot + offset + pivot_offset` chains sum to `inf`, and a ±3e38 needle sweep overflows `(max − min) * frac`. The gate is now a designed magnitude bound rather than bare finiteness: `camtrack::USABLE_BOUND = 1e6` with shared `usable_f32`/`usable3`/`usable1`/`usable_vec` helpers backs every accessor on all three specs (flags included — a beyond-bound `CollideType` reads off), and `validate` names the field with a distinct "exceeds the usable bound" issue. The bound sits orders above anything authored (retail max across the 4,550 tokens is `CameraFar 1330`) and orders below `f32::MAX`, so the composed transforms/projections/sweeps cannot reach `inf`. `spawn_dash`'s pkg/mtx-authored `part.origin` pivot shares the gate (same `pivot + offset` overflow shape, different record family). `ChaseLens::authored`'s hand-rolled offset-length check is subsumed and removed. Tests: `beyond_bound_fields_are_named_and_read_unauthored` (`mm2_formats` `camtrack`/`dash`), `overflowing_authored_fields_fall_back_to_the_designed_boom` (`mm2_app` `camtrack`), `overflowing_asnode_reads_unauthored` (`mm2_app` `dash`), `overflowing_authored_pov_fields_fall_back` (`mm2_app` `mirror`).

---

## Speculative

Listed only so they are not re-discovered as "new"; none is verified enough to act on.

* **S1. `crates/mm2_game/src/effects.rs:1116** — `let want = blast_due + spew_due;` where `blast_due` comes from authored `InitialBlast` (`req_i64`) and `spew_due` from authored `SpewRate` (`req_f32`). `InitialBlast 9223372036854775807` gives `blast_due ≈ 9.2e18`; a `SpewRate` large enough to saturate `spew_due` to `usize::MAX` overflows the sum. Needs *both* fields hostile at once, so not an accidental-corruption path. Everything downstream (`want.min(room)`, `room -= emit`) is correctly bounded.
* **S2. `crates/mm2_game/src/props.rs:597** — `reached[rid as usize]` is validated against `psdl.rooms.len()` but indexes `psdl.prop_rules`. In bounds today only because `Psdl::parse` fills `prop_rules` with `n_rooms` entries and `rooms` with `n_rooms - 1` (`crates/mm2_formats/src/psdl.rs:226-241`). I could not construct an authored file that breaks it, and the next line correctly uses `.get()`. A fragility note, not a defect.
* **S3. `crates/mm2_app/src/scripted.rs:689** — the *outer* `loop` of `reanchor_pose` (finding 1) with a **non-finite** closed route: `d`/`walked` go NaN, `walked >= REANCHOR_WALK` is permanently false, so the loop runs while `blocked(pose)` stays true. The scripted `blocked` includes `!supported(p)` (`scripted.rs:672-677`, a `cast_ray` from a NaN origin), which would plausibly report "no ground" → blocked → spin. **Avian's behaviour for a NaN ray origin was not established.** The opponents call site is fine (its `blocked` is false on a NaN pose).
* **S4. `crates/mm2_app/src/traffic.rs:742** (`size_collider`) — hands unchecked authored `Size` to `Collider::convex_hull` / `Collider::cuboid`. `parry3d-0.27.0` does carry live `assert!`s in its convex-hull code (`src/transformation/convex_hull3/convex_hull.rs:440`, `:526`), but I could not establish that avian's `Option`-returning wrapper reaches them with NaN input, so **no panic is claimed**.
* **S5. `crates/mm2_app/src/city.rs:919-937** (`choose_spawn`) — derives the cruise spawn from raw PSDL vertices/`bounds_center` with no finiteness check (it uses `total_cmp`, which orders NaN without panicking) and lands in the same `session.rs:518` → `:1096` slot as finding 5. The `.psdl` binary reader was not traced for whether a NaN vertex survives parse, so treat this as a likely second producer, unverified.

---

## Checked and rejected (verified guarded — do not re-raise)

**`crates/mm2_formats` is the best-hardened part of the tree.** `psdl.rs:221-222` rejects
`n_textures == 0` at `:213`, and all PSDL counts go through `checked_count`/explicit caps;
`cpvs.rs:77` rejects `0`/over-cap at `:69`; `aimap.rs:329/335` proved non-empty at `:311`;
`spchdata.rs:248` rejects `cells.len() < 3` at `:242`; `waypoints.rs:244/261` rejects
`cells.len() < 5` at `:229` so the range cannot invert; `tex.rs` rejects a zero dimension
at `:147` and enforces every mip's byte count via `r.bytes(byte_count)?` (no allocation
amplification, no short decode buffer on the sized formats); `bnd.rs:292-307` validates
every polygon vertex index; `pathset.rs:225-232`/`:302-310` and `lmap.rs:48-63` cap counts
before `with_capacity`; `sky.rs:39` requires exactly 4 tokens; `mtx.rs:48` checks
`BYTE_LEN`; `hudmap.rs:95-104` (`tail`) rejects `values.len() < n`; `tune.rs:409`'s
`line_end - 1` is only reached when `same_line` is non-empty; `ped.rs:1474` is an
`else if` after an explicit range test; `cardata.rs:549-557/632/783-784/1118-1131/
1203-1209/2170-2174`, `racedata.rs:193-194`, `materials.rs:95/104/119-138/556` and
`crashdata.rs:154` are each guarded by an arity check, `checked_sub`, `.get()` or an
`is_finite()` gate; `dave.rs:371` and `wav.rs:485` are inside `#[cfg(test)]`.

**`crates/mm2_game`.** `nav.rs`: `NavGraph::build` resolves every road end through
`bai.intersections.get(...).and_then(|int| int.roads.get(...))` with a back-reference
check (`:877-887`), so `int_idx` at `:1137` is in range; `push_lane` (`:2098-2110`)
*drops* non-finite lane points and lengths with `NavIssue::NonFiniteLane` — a real gate,
which is what makes `sample_polyline`'s `s.clamp(0.0, length)` (`:2233`) safe; arcs are
only pushed when `!vehicle_lanes.is_empty()`, so `transfer_lane`'s `lanes.len() - 1`
holds; `project_to_lane`, `legal_exits`, `advance_lane_cursor` (iteration-capped at
`traffic.rs:628`) and `leg_leaves_corridor` (a short-circuiting `.any()`) are all fine.
`ped.rs`: `PedRig::sample` rejects `clip.frames == 0` and non-finite frames
(`:222-235`), `tick` rejects `clip_frames == 0` (`:478`), `:366` uses
`saturating_sub(1).clamp(0, u32::MAX as i64)` — the F19-A.3 repairs hold. `props.rs`:
`match_strip` enforces `kerb.len() == outer.len() && >= 2` (`:501`), so `strip_at`'s
paired indexing is safe. `audio.rs`: `pick_step` (`:722`), `draw_speaker` (`:823`) and
`draw_cue_suffix` (`:845`) guard their divisors; `SirenStep::next_index` is range-checked
at `:774-778`. `traffic.rs`: `green_member` guards `n == 0` and `period == 0`;
`JunctionPolicy` ticks are designed constants; `AmbientRoster::new`/`select` guard;
`plan_ambient` clamps density so `with_capacity` is bounded by `policy.max_active`.
`effects.rs`: `SmokePolicy::atlas_tiles`/`PTX_ATLAS_TILES` are designed constants so
`tile`'s `(atlas_tiles * atlas_tiles) as i64 - 1` cannot invert; `SparkPolicy` is
designed (DSN-26). `banger.rs`: `BangerDefinition::from_record` (`:155-187`) is a real
laundering boundary (non-finite/non-positive mass → `DEFAULT_MASS`, size/cg non-finite →
0, `angular_kick` returns `ZERO` on a non-finite result), and `claim_slot`'s
`*occupied -= 1` (`:364`) cannot underflow. `texel.rs:78` rejects non-finite splat
points/radii. `race.rs`: `RaceLine::bind` guards `n < 2` and `!loop_len.is_finite()`;
`measure`/`cycle_target` guard; `time_remaining` is fully saturating.

**`crates/mm2_content`.** `race_def.rs:247-248` and `availability.rs:80` are guarded by
explicit arity/`>= 3`/`>= 4` tests; `model.rs:450` by `w.index >= 4`; `convert.rs`'s
other clamps all use constant bounds (a NaN *value* is legal for `clamp`, only a NaN
*bound* is not). `catalog.rs`, `events.rs`, `garage.rs`, `rewards.rs`, `damage.rs`,
`surface.rs` and `crashcourse.rs` carry no arithmetic on a parsed numeric field.

**`crates/mm2_vehicle`.** `config.validate()` is a real gate (`crates/mm2_game/src/session.rs:239`,
`crates/mm2_content/src/assemble.rs:383`) and checks `finite(s.travel) && s.travel > 0.0`,
non-empty `gear_ratios`, etc., so `systems.rs`' `clamp(0.0, suspension.travel)` and
`sim.rs`' `gear_ratios[gear]` are covered.

**`crates/mm2_app`.** `city.rs`: room attributes resolve vertex refs through
`vertex(i, verts)` → `AttrError::BadVertexRef`; `fan`/`fan_facing`/the divided-road
emitters guard `len < 3`/`n < 2`; `AnimatedTexture` is only pushed when
`frames.len() > 1`; `RoomCollider` push is gated on `!group.tris.is_empty()` and
`PropCollision::into_collider` on non-empty positions/tris; `decode_tex_with` caps mip
levels against `max_levels`. `navarrow.rs`: `rasterize_tri`'s caller range-checks
file-supplied indices (`:314-320`), and `RgbaTex::sample`'s `% self.width` cannot see a
zero width because `TexFile::parse` rejects zero dimensions. `water.rs` is the model the
others should follow — `(r >= 1).then(|| psdl.rooms.get(r as usize - 1))`,
`psdl.vertices.get(...)`, a `top.is_finite()` check, and `WaterIssue::NonFiniteLevel` is
an actual gate at `city.rs:3722-3726`, matching its own doc claim at `water.rs:51-53`.
`decals.rs`: `spacing_metres` is `u8 / 4.0` with a `<= 0.0` fallback, `build_ribbon`
checks `a.is_finite() && b.is_finite()` per section, `indices[..take*6]` is bounded by
`take <= ribbon.quads`. `environment.rs`: `slot % jobs` uses `paint_jobs.max(1)`, the
shader pick uses `.get()`, `SKY_DOME_RADIUS / extent` is gated on
`extent.is_finite() && extent > 0.0`, and the sky scalars are finiteness-gated at
`:481-488` (a real gate). `camera.rs:596-597`'s `clamp(dist_min.min(rest),
dist_max.max(dist_min))` is safe by construction — `min ≤ max` algebraically, and
`f32::max`/`f32::min` return the non-NaN operand so neither bound can be NaN.
`hudmap.rs`: `saturating_mul` + `.get()` + `.max(1)` throughout, `paint_jobs ≤ 127`
(`pkg.rs:482-491`), `.get(DOT_*)` + `unwrap_or_default()`, and bevy's
`Viewport::clamp_to_size` re-clamps an out-of-window authored viewport every frame.
`dash.rs`: `.filter(|m| *m > 0.0)` rejects NaN and zero, `.max(1.0)`, `.max(1e-3)`,
`want.min(len.saturating_sub(1))` + `.get(want)`. `menu.rs`: `cycle_choice`, `step4`
(`rem_euclid(4)` into `Weather::new`/`TimeOfDay::new`, both range-checked at
`config.rs:321-332`) and `authored_seed` (`:1523-1554`, a real gate: `u8::try_from` +
`Weather::new().ok()` + `TimeOfDay::new().ok()` + `densities.validate()` +
`num_laps >= 1`) are all sound. `racestat.rs`/`racetime.rs` cap every digit source
(99 / `999*6000+5999`) and index spawn-fixed arrays. `session.rs:598`'s
`apply_race_picks` clamps the opponent pick to `roster.entries.len()`, so every
`0..opponent_count` loop walks a real vec length. `precip.rs`/`wheel_fx.rs`/`damage_fx.rs`
always hold ≥ 1 atlas quad, so `quads.len() - 1` is safe. `pvs.rs` indexes
`0..cells.len()` and uses `cpvs.decompress(...)`. `race.rs:650`'s `LOW_TIME_TICKS - t` is
inside `t <= LOW_TIME_TICKS`. `contracts.rs:230-243` filters the striker mass to
finite-and-positive. `breakaway.rs`, `texel_fx.rs`, `oppind.rs`, `scripted.rs` (other
than finding 1) verified guarded.

**`tools/mm2_inspect`.** `g.intersections()[exit.intersection as usize]` (`main.rs:2377`,
`:2417`, `:2506`) is safe — `NavGraph.intersections` is a 1:1 map of `bai.intersections`
(`nav.rs:1207-1216`) and `exit.intersection` was validated during build;
`main.rs:2233`'s `with_capacity(n_arcs)` uses an internal arc counter; `main.rs:985`,
`:2453-2454` are guarded by `peak.max(1)` / `v.is_empty()`; `placement.rs:150/162/
477-481/618`, `bind.rs:494`, `crashcourse.rs:227`, `event.rs:553`, `audio.rs:216-217`
(`duration_secs` guards `byte_rate == 0`) and `inventory.rs` verified guarded.

## Not in this defect class, noted in passing

`tools/mm2_inspect/src/placement.rs:789` and `bind.rs:302`/`:509` use `vfs.read(&res)?`,
so a read error aborts the whole multi-city audit instead of being recorded as a
per-source failure the way the parse errors immediately around them are.

## Summary of doc/test claims contradicted by the code

| # | Claim | Where | Reality |
|---|---|---|---|
| 1 | walk is "bounded", "disclosed, not silently unbounded" | `opponents.rs:184-187`, `:193-196`, `:597-599` | infinite loop on a degenerate closed route |
| 2 | "the graph itself is always produced"; "never hidden" | `nav.rs:862-866`; `mm2_content/src/nav.rs:42-44` | index-OOB panic; `BaiIssue::DanglingIntersectionRoad` exists but is not consulted |
| 3 | a hostile pkg "must fail the spawn, not panic inside it" | `navarrow.rs:314-320` (+ test `navarrow.rs:533`) | honoured in the rasteriser, not in the production builder `city.rs:2573` |
| 5 | "Reject a definition that would behave oddly at runtime"; "out-of-range authored parameter" rejected; "a failed load can never leave a live player simulation (AC02)" | `race.rs:361`; `race_def.rs:340-344`; `session.rs:30-31` | ~~non-finite start slot passes `validate()`~~ **resolved** — `NonFiniteGate`/`NonFiniteStart` reject it |
| 6 | "a hostile def is measured against the budget instead of walked"; "overflow is counted, not silently dropped" | `props.rs:791-794`, `:96-99` | ~~the count itself overflows before the budget applies~~ **resolved** — f64 count bounded by `maxUse` before the cast |
| 8 | "a modded table's `9223372036854775807`-scale fields are undrawable, never an overflow panic" | `audio.rs:838-845` (the repaired sibling) | ~~the two `effects.rs` instances of the same shape were not repaired~~ **resolved** — shared `flipbook_span` |
| 9 | "a degenerate/absent mass or CG falls back to sane defaults" | `traffic.rs:798` | ~~the `Size`-derived CG fallback, `friction` and `elasticity` do not~~ **resolved** — `req_finite_f32`/`req_finite_vec3` gate the record at decode |
| 10 | "Malformed rows are skipped and recorded in `diagnostics`" + test `malformed_rows_are_diagnostics_not_panics` | `crashdata.rs:67-72`, `:267` | ~~`nan`/`inf`/fractional `Event`/`Checkpoints` are silently coerced~~ **resolved** — `int_cell`/`num_cell` diagnose and skip |
