# Last iteration — F25-B measurement slice II: the AC03 impairment
# matrix runs as an eight-cell recipe grid over real loopback with
# measured counters recorded in `docs/research/net.md`, which also
# gains the req-6 payload/update-rate/bounds budget (new-run
# iteration 2)

Implementation iteration on `ralph/night` (baseline `1e1f820` — the
latest-wins/`LinkStats` candidate; external verify + review pass with
gaps only). The named open items were the F25-AC03 matrix itself and
the written bandwidth/update-rate budget — this iteration lands both,
consuming the `bytes_*`/`delayed`/`snap<x>` counters the last slice
added.

## What landed

- `net_app::the_impairment_matrix_records_each_recipe_cell`: eight
  named recipe cells (clean, latency 100 ms + 20 ms jitter, jitter
  10 ms + 60 ms, loss 20%, loss-heavy 60%, duplicate 50%, reorder 50%,
  and the combined recipe the two-process `net_drive` leg runs), each
  a fresh in-process host + joined client over real loopback through
  a fresh seeded `ImpairProxy`. The lobby crosses clean, both
  directions arm at `Playing`, ~150 paired updates move real `Input`
  frames up and `Snap` frames down, and a bounded settle tail drains
  every owed hold (delay + jitter + `HOLD_CAP` ≤ 220 ms) before the
  counters are read. Per-cell floors assert frames moved both ways,
  snapshots applied, inputs drove the remote seat, no lane overflow,
  each armed knob visible in its `LinkStats` counter, and
  duplicate/reorder cells landing counted `snaps_staled`; the clean
  cell asserts all recipe counters zero so its stale count calibrates
  the publish-cadence dedup floor. A `matrix cell=` record line per
  recipe carries the measured `LinkStats` + `NetDriveReport` numbers.
- `lobby_app` (the shared `net_app` fixture) now registers
  `advance_session_tick` in `FixedUpdate`, matching production and
  `run_headless` — without it every published `Snap` shares tick 0
  and the stale floor would measure a test artifact, not the wire.
- `docs/research/net.md`: new "Measured impairment matrix (F25-AC03)"
  section records the run (251 snaps published, 103 applied / 148
  staled clean — the ~60% same-tick republish floor at the fixture's
  64 Hz tick vs ~250 Hz update cadence; duplicate adds ~1 stale per
  copy; reorder moves the floor rather than raising it; 60% loss
  still applies 69 snaps and never starves the input mailbox), plus a
  "Data-plane budget and bounds" section: the v10 payload-size table
  (`Input` 21 B, `Snap` = 20 + 62·seats + 57·trailers + 46·impacts,
  worst case 3,916 B ≪ `MAX_FRAME`), the update-rate model (both
  directions send once per `Update` — vsync-bounded windowed,
  unbounded headless; ~31 KB/s downstream per client at 60 Hz/8
  seats), and the receiver bounds (`RemoteSnaps`, the 250 ms input
  staleness, impact pending/dedup caps, `CORRECTION_SNAP_DIST`,
  `RESET_REQUEST_COOLDOWN`).

## Tests

`net_app` 37→38 (the matrix leg; the `advance_session_tick`
registration is exercised by every existing leg — all 38 pass).
Measured numbers in the doc are a transcribed `--nocapture` run.

## Gates

`cargo fmt --all -- --check` clean; `cargo clippy --locked
--workspace --all-targets --all-features -- -D warnings` clean;
`cargo test --locked --workspace` green — all binaries, 0 failures
(`net_app` 38/38 incl. the matrix cell).

## Classification / remaining open items

- Implementation choice + measured evidence throughout — the matrix
  recipes, assertion floors and budget numbers are ours; no
  original-behavior claim (the retail wire protocol is unrecovered).
- Evidence: in-process apps over real loopback sockets only — the
  measured grid is *not* process-level (the `net_drive` two-process
  harness still runs its single combined recipe), still no LAN or
  Internet leg, nothing rendered or driven by hand, no retail
  content. AC03 advances but stays open until a process-level grid
  exists; AC01–AC02/AC04–AC06 stay open as before.
- Documented interpretation: `snap<x>` is only meaningful against the
  clean-cell floor (same-tick republish dedup); `inputs_staled`
  measures staleness at apply, not arrivals — arrival volume is the
  proxy's `bytes_in`/`frames_in`.
- F25-B remaining scope: replicated result/race state, remote-copy
  breakaway fragments, the process-level impairment grid. The
  `RemoteSnaps`-persists-across-hosts wedge the last review named is
  unchanged — a fresh host restarting generation 1 stale-drops until
  its ticks pass a stale watermark.

---

# Last iteration — F25-B measurement slice: `RemoteSnaps::push`
# gains a `(generation, tick)` watermark so a reordered/duplicated
# frame can no longer displace a newer staged pose, `LinkStats`
# measures payload bytes and delay-holds, and the `net=` record
# reports client-side stale drops (`snap<x>`) (new-run iteration 1)

Resume-and-finish iteration on `ralph/night` (baseline `b2999a0` —
the trailer-leg candidate; external verify + review pass with gaps
only). The previous run's iteration 2 was interrupted mid-slice with
this work uncommitted in the tree (the push gate + `stale` counter and
the `LinkStats` counters were already written; the report fold, the
`net=` cell, and all test coverage were not). This iteration completed
that slice rather than opening unrelated work — same disposition as
the previous run's iteration 1 gave *its* interrupted predecessor.

## What landed

- `RemoteSnaps::push` is now latest-wins on the frame's own
  `(generation, tick)` instead of arrival order: an incoming frame at
  or behind the staged-or-applied watermark drops counted into a new
  `stale` counter — under a reorder/duplicate recipe an old frame can
  no longer clobber a newer pending pose for a frame. Its impact rows
  still queue: events outlive the frame that carried them.
- `NetDriveReport::snaps_staled` folds the push-time count
  (`apply_snapshots` folds every run — a stale drop can land when
  nothing is staged) plus the now-unreachable-in-practice apply-side
  stale drop, kept as a counted guard.
- `net=`'s snap cell gained a third counter — `snap<s>s/<a>a/<x>x`,
  sent/applied/staled — matching `in`'s existing `<x>` convention.
- `LinkStats` gained `bytes_in`/`bytes_out` (payload bytes, each
  duplicated copy re-paying) and `delayed` (frames scheduled with a
  positive delay/jitter hold; a reorder-held frame counts once for
  the hold and once for its delayed release) — the measured half of
  F25-B req 6's bandwidth/delay budget evidence.
- `net_drive`'s parser reads the new cell; the impaired two-process
  leg now asserts `bytes_*`/`delayed` on both proxy directions and
  `snaps_staled > 0` across the client records — the AC03
  stale-state measure landing on real processes.

## Tests

`netdrive` 14→16: `a_stale_snap_drops_at_push_but_keeps_its_events`
(straggler/dup at or behind the watermark drops counted and cannot
displace a newer staged pose, its impact rows still queue and dedup,
latest-wins still moves forward, the applied watermark gates with
nothing staged, a new generation is never stale) and
`apply_snapshots_reports_the_stale_drops` (the fold runs even when
nothing applies). `mm2_net` impair legs extended: byte counts on the
transparent/clean/reorder/duplicate/delay legs and `delayed` on the
delay/reorder legs (reorder alone pays no delay-hold — `reordered`
counts it). `net_drive::an_impaired_two_process_session_still_
converges` gained the `bytes_*`/`delayed`/stale-cell asserts — and
produces them over real loopback.

## Gates

`cargo fmt --all -- --check` clean; `cargo clippy --locked
--workspace --all-targets --all-features -- -D warnings` clean;
`cargo test --locked --workspace` green — 91 binaries, 0 failures
(`netdrive` unit 16/16, `mm2_net` 80/80, `net_drive` 2/2 incl. the
impaired leg).

## Classification / remaining open items

- Implementation choice throughout — the watermark ordering, the
  counter shapes and the record cell are ours; the retail wire
  protocol is unrecovered, so no original-behavior claim.
- Evidence: in-process unit + integration legs plus the two-process
  impaired leg over real loopback — still no LAN, no rendered/manual
  observation, no retail content.
- F25-B remaining scope: replicated result/race state, remote-copy
  breakaway fragments, the full documented AC03 impairment *matrix*
  (this landed the measurement plumbing and one measured recipe, not
  a grid), the written bandwidth/update-rate budget doc the new
  `bytes_*` counters now feed. AC01–AC06 stay open.

---

# Last iteration — F25-B repair: the trailer leg's three copy-side
# review findings — grounded bit lands on the copy's wheel state, the
# hitch joint despawns with its trailer, the predicted copy carries
# `DamageSignals` (new-run iteration 1)

Repair iteration on `ralph/night` (baseline `a844eb3` — the
encode-floor candidate of the previous run's interrupted iteration 4;
its uncommitted ordering-repair half was still in the tree and is now
committed as `af2cc5c`, gates re-verified green before committing).
The trailer leg's external review (run 20261002T095452, iter 1, pass
with gaps) left three non-blocking copy-side findings; this iteration
repairs all three — repair-before-feature per the selection policy.
Three commits, one per defect class:

- `ef315d4` — `car_visual::spawn_trailer` now children the
  `SphericalJoint` entity to the trailer (`ChildOf`), so a mid-session
  leave/re-pick despawn sweeps it instead of orphaning it on dead
  bodies until `SessionEntity` teardown.
- `cf231a9` — the predicted half of `spawn_remote`'s trailer rig is a
  named helper, `spawn_trailer_copy`, which lands the copy with
  `DamageSignals` like the authority's real body and the local trailer
  — a client's impact-signal consumers (`collect_impacts` →
  telemetry/events) now see remote-trailer contacts.
- `dad1c59` — the v9 `SnapTrailer` grounded bit reaches the kinematic
  copy's `VehicleState` (`apply_trailer_present` in the trailer apply
  loop): the row carries no compression, so grounded settles every
  wheel at its authored rest sag (`HandlingMetrics::of` — the number
  the local sim settles to) and a clear bit hangs full droop. The own
  rig's real trailer keeps its sim-owned wheel state.

## Tests

- `netdrive::tests::a_predicted_trailer_copy_carries_damage_signals`
  drives `spawn_trailer_copy` through a real `CommandQueue` and locks
  the copy's contract (DamageSignals, kinematic replica rig, wire-keyed
  marker, hitched rest pose).
- `trailer.rs::despawning_the_trailer_takes_the_hitch_joint_with_it`
  spawns the real jointed rig through `spawn_trailer`, despawns the
  trailer, asserts the `SphericalJoint` entity is gone and the car
  untouched.
- `net_app::a_trailer_rows_grounded_bit_drives_the_copys_suspension`
  runs the real loopback wire: host broadcasts `Snap.trailers` rows
  into a `bridge_app` client — grounded row lands `state.grounded`
  plus per-wheel rest sag matching `HandlingMetrics`, a clear row
  droops every wheel.

## Gates

`cargo fmt --all -- --check` clean; `cargo clippy --locked
--workspace --all-targets --all-features -- -D warnings` clean;
`cargo test --locked --workspace` green — 91 binaries, 0 failures
(`net_app` 37, `trailer` 2, `netdrive` unit 14).

## Classification / remaining open items

- Implementation choice throughout — copy-side presentation/lifecycle,
  no original-behavior claim.
- Evidence: in-process unit + integration legs over real loopback
  sockets. Trailered picks remain retail-only (`vpsemi`/`vpcentury`),
  so no process-level session carried a trailer — the rigs are
  declared by hand the way the spawn paths leave them. Nothing
  rendered or driven by hand; no LAN.
- The `apply_trailer_present` rest-sag presentation is designed: the
  wire grounded bit cannot express per-wheel droop, so a grounded copy
  shows the settled pose rather than live deflection.
- F25-B remaining scope unchanged: replicated result/race state,
  remote-copy breakaway fragments, the measured AC03 impairment
  matrix, bandwidth budgets. AC01–AC06 stay open. Impact-side review
  nits still open: `RemoteImpact` carries no struck-side identity,
  surface or generation; `OversizeImpacts`'s u8 cast is cosmetic.

---

# Last iteration — F25-B repair: replicated repair ordering — a
# per-seat repair ledger makes a pre-repair `SnapImpact` row drop
# instead of splatting after the wipe, and `encode_damage` stops
# rounding a positive total onto the repair byte (iteration 4, run
# 20261002T095452-41346)

Repair iteration on `ralph/night` (baseline `9e93990` — the texel
candidate; external verify + review pass with gaps only). Two of its
verification gaps were real defects in the landed ordering, so this
iteration repairs them instead of opening new feature work.

## What landed

Two commits, one per defect class:

- `a844eb3` — `encode_damage` floors any positive total at byte 1:
  `round(fraction*255)` mapped a total in (0, ~0.2% of `MaxDamage`)
  to 0, which a predicted client reads as the `>0→0` repair signal —
  a wipe the authority never ordered. Unreachable through `apply`
  today (an accepted severity always clears ~0.4% of max); reachable
  the day the authored `regenerate_rate` channel runs mid-session.
  Byte 0 now strictly means "total is 0".
- ordering — `RemoteSnaps` gains `repaired`, a per-seat ledger
  recording `(generation, snap tick)` of each `>0→0` transition the
  apply pass performs. `apply_snapshots` split into
  `apply_snap_frame` (state) + `drain_pending_impacts` (events); the
  drain now runs *after* the state pass every run, so a repair byte
  landing this frame is recorded before the queued rows are judged.
  A pending row whose `tick` sits at or below the seat's recorded
  repair tick drops (`SnapImpact::tick` is the emit tick, never after
  its snap's publish tick, so it is provably pre-repair — the
  authority ordered splat-then-wipe). This covers both co-arrival
  (the row outlived its superseded snap and drains beside the repair
  byte) and late arrival (a reordered pre-repair frame's rows
  draining after the repair applied).

## Tests

`texel_fx` 10→13: `a_pre_repair_impact_row_never_splats_after_the_
wipe` (co-arrival — damaged byte applied, then a queued pre-repair
row plus the repair snap in one update: skin stays clean, row counts
as a drop, the wipe still runs), `a_delayed_pre_repair_row_drops_
against_the_repair_ledger` (the row arrives a snap *after* the wipe
applied), `a_post_repair_impact_still_splats` (emit tick past the
repair's snap tick lands like any other hit). `netdrive` unit test
gained the sub-byte encode leg. New helpers: `push_snap_with_
impacts`, `impact_row`, `snap_entry_at` (asserts a pose so the copy
stays where the impact points were authored — entries at the wire
zero pose teleport it to the origin before `apply_remote_texels`
reads `GlobalTransform`).

## Gates

`cargo fmt --all -- --check` clean; `cargo clippy --locked
--workspace --all-targets --all-features -- -D warnings` clean;
`cargo test --locked --workspace` green — 91 binaries, 0 failures
(`texel_fx` 13/13).

## Classification / remaining open items

- Implementation choice throughout — the ledger shape, the `<=`
  boundary and the encode floor are ours; the retail wire protocol is
  unrecovered, so no original-behavior claim.
- Evidence: in-process integration legs driving the production
  systems — no process-level session produced a nonzero `imp` cell
  (the dev-world cruise never collides), nothing rendered or driven
  by hand, no LAN, no retail content.
- Residual edges, documented in `docs/research/net.md`: a hit emitted
  in the *same host tick* the repair resolved reads as pre-repair
  (may drop a legitimate post-repair splat); a pre-repair hit whose
  damaged intermediate byte was superseded before it ever applied is
  unobservable — no transition was seen, so the row is
  indistinguishable from a fresh hit on an intact car. Both are
  presentation-only; a per-seat repair epoch on the wire row would
  close them if they ever matter.
- F25-B remaining scope unchanged: replicated result/race state,
  remote-copy breakaway fragments, the measured AC03 impairment
  matrix, bandwidth budgets. AC01–AC06 stay open.

---

# Last iteration — F25-B eighth slice: replicated texel damage —
# remote copies bind a `TexelDamageRig`, splat off the `RemoteImpact`
# stream, and clear on the replicated damage byte's repair transition
# (iteration 3, run 20261002T095452-41346)

Implementation iteration on `ralph/night` (baseline `06a027c` — the
v10 impact-event candidate; external verify + review pass with gaps
only). The named next slice was the F25-B remainder; this takes the
texel leg its review named as the open "a remote copy binds no texel
rig" gap — protocol v10 already carries per-impact positions, so the
skin can now splat where the hit landed.

## Task selection

No failing gate or blocking finding — the v10 review passed with gaps
only. Of the named F25-B remainder (result/race state, texel splats,
breakaway fragments, the AC03 measured matrix, bandwidth budgets),
texel replication is the highest-value ready slice: every piece it
needs already exists (`RemoteImpact` rows, `spawn_vehicle_model`'s
optional rig, `TexelRepair`), the consumers (`spark_fx`, `audio`) just
proved the exactly-once-per-process pattern, and it closes req 5's
damage-replication leg presentation-side. Breakaway fragments are a
bigger slice (authority-spawned state, not presentation).

## What landed

- `netdrive::spawn_remote`: the model build now passes
  `def.damage.as_ref().map(|d| (d, generation << 32 | wire))` to
  `car_visual::spawn_vehicle_model` — a remote copy binds
  `TexelDamageRig` on the same authored `vehcardamage` record +
  `_dmg`-paired-shader gate a local pick does, seeded off the wire id
  like the smoke/spark rigs. Nothing new loads; the rig builder is
  the existing one.
- `texel_fx::apply_texel_damage`: the authority gate widened to the
  session — outside `Playing` it drains-and-drops like before; inside
  it applies local `ImpactEvent`s to every participant on the
  authority (a `Remote` seat is locally simulated there and splats
  like an AI car), while on a predicted client the `Remote` skip
  keeps remote copies off the local stream so a hit never
  double-stamps. A client's own predicted seat splats off its local
  stream — `apply_snapshots` never echoes an own-seat row.
- `texel_fx::apply_remote_texels` (new, Update): reads `RemoteImpact`,
  resolves the copy's `GlobalTransform` + `TexelDamageRig`, converts
  the replicated world point to car space, and runs the same
  `rig.apply` the local stream feeds. Copies without a rig (no
  authored damage record / no `_dmg` pair) skip silently; the reader
  drains while not `Playing`. Scheduled `.after(apply_snapshots)` in
  `main.rs` and `smoke.rs` — a replicated hit splats in the frame it
  landed (the `smoke.rs` Update tuple hit Bevy's system-config arity
  cap, so the system registers on its own `add_systems` line).
- `netdrive::apply_damage` (the v8 damage-byte write): now detects the
  `>0 → 0` transition — the authority's `resolve_disabled`
  `damage.reset()` + `texel.reset()` pair arriving as replicated
  state — and runs `TexelRepair::reset` on it. Splats stamped while
  the byte read intact stay put (the retail rig splats every
  `ImpactsTable` entry regardless of the accumulator), and repeated
  clean bytes are not repairs — only a real transition re-blits. The
  transition check sits inside the `Option<VehicleDamage>` guard, so
  undamageable picks keep skipping.
- `apply_snapshots` threads a `TexelRepair` SystemParam (the bundle
  `resolve_disabled` already uses) so the wipe runs inside the apply
  pass.
- `docs/research/net.md`: the v10 paragraph now records the texel leg
  and keeps the open gaps honest — no struck-side identity, no
  `surface`, breakaway fragments still authority-only.

## Tests

`texel_fx` 5→10: `texel_app` gained a `SessionAuthority` parameter
(the fixture app can now run a predicted session) and the fixture
spawn moved into `bind_fixture_model`, shared with a new
`spawn_rigged_remote` that reproduces `spawn_remote`'s component
shape (`SessionEntity`/`NetPlayer`/`ResetEpoch`/`PlayerControl::Remote`
+ the authored-spec `VehicleDamage`) through the production model
path. New legs: `a_remote_seat_splats_off_the_local_stream_on_the_
authority` (Remote participant splats like an AI car where it is
simulated), `a_predicted_clients_own_seat_splats_off_the_local_stream`,
`a_remote_copy_ignores_the_local_stream_on_a_predicted_client` (the
once-per-process contract), `a_replicated_impact_splats_the_remote_
copy` (`RemoteImpact` → `rig.apply` at the replicated point),
`a_replicated_repair_restores_the_splats` (byte 200 lands, splat,
byte 0 re-blits clean, `resets == 1`), `a_replicated_repair_restores_
the_own_seats_skin` (the predicted seat's own splats wipe on the same
signal — its `VehicleDamage` is only written by the byte), and
`a_clean_byte_never_erases_a_splat` (a splat stamped at byte 0
survives repeated `damage: 0` snaps — `resets == 0`).

## Gates

`cargo fmt --all -- --check` clean; `cargo clippy --workspace
--all-targets --all-features -- -D warnings` clean; `cargo test
--workspace` green — all binaries, 0 failures (`texel_fx` 10/10).

## Classification / remaining open items

- Implementation choice throughout — rig gating/seed, the `Remote`
  skip split, the `>0→0`-as-repair read of the v8 byte are all ours;
  the retail wire protocol is unrecovered, so no original-behavior
  claim.
- Evidence: in-process integration legs only — the fixture apps drive
  the production systems but no process-level session ever produced a
  nonzero `imp` cell (the dev-world cruise never collides), nothing
  rendered or driven by hand, no LAN, no retail content exercised.
- Named gaps stay open: `RemoteImpact` carries no struck-side
  identity (replicated audio still picks the id-0 catch-all) and no
  `surface`; a remote copy's splats are presentation-only — its body
  panels still never detach (breakaway is authority-spawned state);
  the `RemoteImpact`→rig path keys off the copy's displayed transform
  a snap behind the authority's.
- F25-B remaining scope: replicated *result/race* state, remote-copy
  breakaway fragments, the measured AC03 impairment matrix,
  bandwidth/update-rate budgets. AC01–AC06 stay open as before; this
  slice advances req 5's damage leg only.

---

# Last iteration — F25-B seventh slice: replicated impact events —
# protocol v10 gives `Snap` an `impacts` list so a remote car's
# per-hit sparks and impact audio render on every process
(iteration 2, run 20261002T095452-41346)

Implementation iteration on `ralph/night` (baseline `4ebd968` — the
v9 trailer candidate; external verify + review pass with gaps only).
The named next slice was the F25-B remainder; this takes its damage
*event* leg — the review's own noted gap (per-impact positions the v8
damage byte cannot carry).

## Task selection

No failing gate or blocking finding — the v9 review passed with gaps
only. Of the named F25-B remainder (result/race state, damage events,
the AC03 measured matrix, bandwidth budgets), replicated damage
events is the highest-value ready slice: it lands on the just-proven
snapshot path, unblocks two presentation consumers at once
(`emit_sparks`, `impact_voices`), and carries the F05 req-6
presentation story forward.

## What landed

- `mm2_net` protocol v10 (`proto.rs`): `Snap` gains `impacts`, a
  `MAX_SNAP_IMPACTS` (64)-bounded list of `SnapImpact` rows — seat
  wire id, `ImpactId`, host tick, world-space point, outward normal
  (mirrored per side), severity. Strict fixed-width decode with an
  `OversizeImpacts` guard on both encode and decode; exact-version
  admit gate; round-trip and oversize fixtures updated.
- `mm2_app::netdrive`: `publish_snapshots` drains the `ImpactEvent`
  stream every run (incl. gated-out phases, so stale hits never
  replay), filters to the current generation, maps `ObjectId` →
  `NetPlayer` wire id, emits one row per seat-named participant side,
  sanitizes finite point/normal/severity, sorts strongest-first and
  truncates at the cap (overflow counts `impacts_dropped`).
  `RemoteSnaps` queues impact rows on their own bounded pending queue
  (256) — events, not state, so a superseded pose frame keeps its
  effects — behind a bounded `(generation, seat, id)` dedup window
  (512, FIFO retire) for duplicated/reordered frames.
  `apply_snapshots` drains the queue every run: foreign-generation,
  unspawned-seat and non-finite rows drop counted; the receiver's own
  seat skips silently (predicted physics already rendered it); the
  rest emit `RemoteImpact` messages resolved to the live copy, with
  the normal re-normalized (`Vec3::Y` fallback on degenerate wire
  values).
- `spawn_remote` binds the authored `VehicleSparks` rig on
  damage-record picks beside `VehicleSmoke` (both roles — on the
  authority the copy is a locally simulated participant and sparks
  off the local stream; on a client off `RemoteImpact`).
- `spark_fx::emit_sparks` / `audio::impact_voices` consume both
  streams through shared burst/voice tails; the authority-side
  `PlayerControl::Remote` skip narrows to predicted sessions only
  (the host renders its remote seats' hits like AI cars'), so each
  impact presents exactly once per process. Replicated audio rows
  read the id-0 catch-all — the wire carries no struck-side identity.
- `emit_sparks`/`impact_voices` order `.after(apply_snapshots)` in
  `main.rs`/`smoke.rs` — a replicated hit presents in the frame it
  landed, keeping `--frames` captures deterministic.
- `NetDriveReport` gains `impacts_sent`/`impacts_applied`/
  `impacts_dropped` → `imp<s>s/<a>a/<d>d` on the smoke `net=` field
  (`imp0s/0a/0d` is the honest dev-world value — the clean cruise
  never collides); `net_drive`'s parser reads the cells.
- `docs/research/net.md` v10 paragraph, gap list and `net=` field
  list updated; the v8 paragraph's "F26 scope" claim corrected.

## Tests

`net_app` +2: `a_snap_carries_the_sessions_impact_rows` (host leg —
a real lobby socket, an `ImpactEvent` naming the remote seat rides
the next `Snap` with mirrored outward normal; world-only and
foreign-generation events emit nothing; `impacts_sent` counts) and
`a_snapshot_feeds_the_remote_impact_stream` (client leg — remote-seat
row resolves to the spawned copy and lands one `RemoteImpact`;
own-seat row skipped by design; departed-seat and NaN rows count as
drops; a duplicated frame's rows never double-fire; a
foreign-generation row drops at the apply gate). `spark_fx` +2,
renamed 1 (authority sparks a remote seat's local-stream hit;
predicted client stays silent; a `RemoteImpact` bursts the copy's
rig at the replicated point/normal/severity). `audio` +2, renamed 1
(same split: authority voices the remote seat spatially; predicted
client silent on local stream; a `RemoteImpact` voices through the
id-0 catch-all). `mm2_net` fixtures: v10 round-trip rows, decode
`OversizeImpacts`, encode `OversizeImpacts`.

## Gates

`cargo fmt --all -- --check` clean; `cargo clippy --locked
--workspace --all-targets --all-features -- -D warnings` clean;
`cargo test --locked --workspace` green — 0 failures across all
binaries (mm2_net 80/80, `net_app`, `net_drive` 2/2, `spark_fx`,
`audio` incl. the new legs).

## Classification / remaining open items

- Implementation choice throughout — the row shape, dedup key,
  per-side mirroring and the presentation-only boundary are ours;
  no original-behavior claim (the retail wire protocol is
  unrecovered).
- Evidence is in-process integration over real loopback plus the
  proto fixtures — no process-level session ever produced a nonzero
  `imp` cell (`imp0s/0a/0d` on the dev-world cruise is honest: it
  never collides), nothing rendered or driven by hand, no LAN.
- Named gaps stay open: a remote copy binds no texel rig (its skin
  never splats — the rig clones textures at spawn), breakaway
  fragments are authority-spawned state, the wire carries no
  struck-side identity (replicated audio picks the id-0 catch-all)
  and no `surface` (no consumer reads it).
- F25-B remaining scope: replicated *result/race* state, remote-copy
  texel/breakaway, the measured AC03 impairment matrix,
  bandwidth/update-rate budgets. AC01–AC06 stay open.

---

# Last iteration — F25-B sixth slice: replicated trailers — protocol
# v9 gives `Snap` a `trailers` list so a trailered pick's rig rides
# the wire, plus the `ResetVehicle`-stream follower that reseats any
# tractor's trailer (iteration 1, run 20261002T095452-41346; resumed
# an interrupted iteration of run 20261002T081030-24791)

Implementation iteration on `ralph/night` (baseline `acabb40` — the
v8 damage candidate; external verify + review pass with gaps only).
The previous run's iteration was interrupted (agent exit 1) with the
v9 production diff landed but no net-side test legs; this iteration
finished that slice rather than starting a new one.

## Task selection

No failing gate or blocking finding — the prior review passed with
gaps only. The uncommitted tree already implemented replicated
trailers (`vpsemi`/`vpcentury` rigs) end-to-end in code; the missing
half was its wire-side evidence and two latent bugs the new legs
exposed.

## What landed

- `mm2_net` protocol v9 (`proto.rs`): `Snap` gains `trailers`, a
  `MAX_PLAYERS`-bounded list of `SnapTrailer` rows — owner wire id,
  pose, velocities, mean grounded-wheel spin (the `SnapEntry`
  encoding) and a grounded flag. Trailers key off the *towing seat*
  and ride its reset epoch; they carry none of their own. Strict
  fixed-width decode, `OversizeTrailers` guard, exact-version admit
  gate, round-trip/oversize fixtures updated.
- `mm2_app::netdrive`: `RemoteTrailer { owner }` marker on both
  roles — `spawn_remote` builds the authority's real jointed body
  via `car_visual::spawn_trailer` or the client's kinematic
  `RemoteLerp`/`RemoteDrive`/`RemoteReplica` copy; `publish_snapshots`
  emits a row per trailer towing a `NetPlayer` seat (host rig
  included); `apply_snapshots` blends a remote copy like its seat,
  snaps it on the owner's epoch advance, and drops the own rig's
  epoch-equal rows outright; `reconcile_remote_players` despawns a
  trailer with its departed/re-picked owner or a stale `towing`.
- `session::reseat_towed_trailers` — a `MessageMutator` follower on
  the `ResetVehicle` stream, scheduled in the binary and headless
  app after every writer and before `vehicle_reset`: every tractor
  reset (the `R` bundle, `--reset-at`, recovery/stuck/disabled
  resolves, scripted/opponent re-anchors, self-right, wire
  `ResetRequest`s) now reseats that tractor's trailers at their
  authored `rest_offset`s in the same update. This replaces the four
  per-caller `SpawnPoint.trailers` loops (`spawn_resets`,
  `resolve_stuck`, `resolve_recovery`, `resolve_disabled`), which
  only ever covered the *local* player — a remote participant's rig
  reseats on the authority identically.
- `trailer_input` reads the tractor's `VehicleInput` by the `Trailer`
  relation instead of `PlayerVehicle`, so a remote rig's trailer
  copy takes its brake state off the snap-applied input.
- `NetDriveReport.trailers_synced` → `tsyn<n>` on the smoke `net=`
  field (client side; `tsyn0` is the honest dev-world value — the
  dev car tows nothing).
- `docs/research/net.md` v9 paragraph and `net=` field list updated.

## Bugs the new legs caught

- `spawn_remote`'s predicted trailer branch spawned
  `vehicle_bundle(..)` **and** `RigidBody::Kinematic`/velocities in
  one tuple — a duplicate-component panic on the first remote
  trailered pick (never exercised: the dev car tows nothing). Now
  the kinematic override inserts over the bundle like the seat does.
- `apply_snapshots` hard-snapped the own rig's trailer — a real
  local body — to the authority's lagged pose *every* snapshot,
  fighting the hitch joint and contradicting the own-seat contract.
  Epoch-equal rows are now dropped; the row lands only on the
  owner's epoch advance or a real divergence.

## Tests

`net_app` 33→34: `a_snap_carries_the_remote_cars_drive_state`
extended (host publishes a trailer row keyed by the seat's wire id,
pose/vel/angvel/spin/grounded asserted off the wire);
`a_snapshot_drives_a_remote_rigs_trailer` new (remote copy blends +
velocities + spin rate land, own rig's epoch-equal row provably
ignored, owner-epoch advance snaps it with `Teleported`,
`trailers_synced` counts the landed rows);
`an_impaired_link_still_converges_the_data_plane` now parks a
`RemoteTrailer`+`RemotePick` trailer on the remote rig and asserts
the reconcile despawn cascades on `Leave`. `session.rs` test:
`spawn_resets_is_the_player_row` (the bundle shrank to the player
row) + `reseat_towed_trailers_follows_any_tractor_reset` (stream
follower lands the trailer the same update, through `vehicle_reset`).
`stuck`/`recovery` trailer legs updated to declare the real `Trailer`
relation. `mm2_net` fixtures updated for v9.

## Gates

`cargo fmt --all -- --check` clean; `cargo clippy --locked
--workspace --all-targets --all-features -- -D warnings` clean;
`cargo test --locked --workspace` green — 91 test binaries, 0
failures (`net_app` 34/34, `net_drive` 2/2, mm2_net 80/80 incl. the
v9 fixtures).

## Classification / remaining open items

- Implementation choice throughout (wire row shape, marker/follower
  design); no original-behavior claim — the original's networked
  trailer behavior is unrecovered.
- Evidence: in-process integration legs over real loopback plus the
  proto fixtures — but no *process-level* session ever carried a
  trailer (`tsyn0` by design on the dev car; nonzero evidence needs
  a retail trailered pick, and `vpsemi`'s rig on a live two-process
  session is unexercised). No LAN, nothing rendered or driven by
  hand.
- `SpawnPoint.trailers` remains populated — the chase-cam occluder
  (`camera.rs`) still reads it; it is no longer a reseat source.
- F25-B remaining scope: replicated *result/race* state, damage
  *event* replication (sparks/texel/breakaway — per-impact
  positions), deduplicated audio/effects, the measured AC03
  impairment matrix, bandwidth/update-rate budgets. AC01–AC06 stay
  open; this slice advances req 3's trailer leg only.

---

# Last iteration — F25-B fifth slice: replicated damage state —
# protocol v8 gives `SnapEntry` a damage byte so remote copies carry
# the authority's `VehicleDamage` total and emit authored smoke off it
# (iteration 16, run 20261001T195454-62282 continued)

Implementation iteration on `ralph/night` (baseline `f018915` — the
v7 presentation-tail candidate; external verify + review pass with
gaps only). One coherent slice: the damage leg of spec req 3's
"damage/reset state" — remote kinematic copies carried an inert
`VehicleDamage` nobody wrote, so a remote car could never show damage
and a client's own seat never learned its authoritative total.

## Task selection

No failing gate or blocking finding — the previous review passed with
gaps only, and its remaining-scope list names "replicated
damage/result state" first. The damage *state* leg is the coherent
small piece: damage *events* (sparks/texel/breakaway need per-impact
positions) stay F26 scope, and result/race-state replication is a
separate slice.

## What landed

- `mm2_net` protocol v8 (`proto.rs`): `SnapEntry` gains `damage`
  (u8) — the authority's `VehicleDamage` total as a fraction of the
  seat's authored `MaxDamage`, ×255. `0` for intact and for a seat
  with no authored `vehcardamage` record (undamageable reads as
  undamaged — no fabricated spec); `255` at/over the bound. Strict
  fixed-width decode, exact-version admit gate unchanged; every
  fixture updated.
- `mm2_game::VehicleDamage::set_replicated` — the wire's write path:
  reconstitutes the total through the entity's own spec, clamps into
  `0.0..=1.0` (an authority repair legitimately lowers it — `apply`
  never could), ignores non-finite input, and never touches the
  `ImpactId` watermark so a late duplicate impact still cannot land.
- `mm2_app::netdrive`: `encode_damage` on publish (`None`/degenerate
  spec → 0, fraction saturates at 255); `apply_snapshots` writes the
  byte through `set_replicated` onto every named seat — remote copies
  *and* the client's own predicted seat, the only own-seat snap field
  (nothing local accumulates damage under prediction, so the
  replicated total is the meter's truth). Writes count into
  `NetDriveReport.damage_synced` → `dsyn<n>` on the smoke `net=`
  field.
- `spawn_remote` binds the authored `VehicleSmoke` rig next to
  `VehicleDamage`, seeded off `generation|wire` so every process
  replays the same emission stream for a seat; `drive_smoke` drops
  its `Remote` skip — on the host a remote copy's rig emits off the
  authority's live accumulator, on a client off the replicated total.
  Texel/sparks/breakaway stay unrigged on remotes: the byte carries
  no per-impact positions — that is F26 event replication.
- `damage::sync_impairment` drops its authority gate (still
  `Playing`-gated; `RemoteReplica` excluded — dead weight on
  unsimulated copies) so a client's own predicted seat weakens the
  way the authority's copy of it does. `apply_impact_damage` and
  `resolve_disabled` stay authority-gated, so a replicated `Disabled`
  never resolves a local wreck — the authority's epoch-declared
  teleport plus a repaired total is the answer that comes back down.
- Tests: mm2_game +1 (`set_replicated` — fraction reconstitution,
  clamps, repair-down, NaN/inf ignored, watermark survives a stale
  duplicate); netdrive +1 (`encode_damage` — none/intact/half=128/
  saturated/degenerate); `net_app` — the client leg's snap carries
  `damage: 128` on the host copy and `200` on the own seat (both
  reconstitute, own-seat drive fields still ignored,
  `damage_synced >= 2`) and the later `damage: 0` snaps assert
  replicated repair on both; the host leg accumulates half of
  `MaxDamage` through the real `apply` path and asserts the decoded
  wire byte is 128; `damage_fx` — `a_remote_participant_never_smokes_
  locally` inverted to `a_remote_participant_smokes_from_its_carried_
  state` (rigged remote emits, puffs attribute to it, an unrigged
  damaged entity still emits nothing); `net_drive` parses `dsyn` (not
  asserted — the dev car binds no damage record, so `dsyn0` is the
  honest dev-world value; nonzero evidence needs retail content).

## Gates

`cargo fmt --all -- --check` clean; `cargo clippy --locked --workspace
--all-targets --all-features -- -D warnings` clean; `cargo test
--locked --workspace` green — 91 test binaries, 0 failures
(`net_app` 33/33, `net_drive` 2/2, `damage_fx` 10/10, `damage` 15/15,
mm2_net 80/80 incl. updated v8 fixtures).

## Classification / remaining open items

- Implementation choice throughout (wire byte, `set_replicated`
  semantics, smoke-rig seeding); the replicated total replays
  authority state, no original-behavior claim.
- Evidence: unit + in-process integration legs plus the two real-
  process loopback sessions — but `dsyn` is `0` there by design (dev
  car has no authored damage record), so the byte's end-to-end path is
  proven in-process only; no LAN, no retail install, nothing rendered
  or driven by hand.
- Replicated `Disabled` presentation on a client is now internally
  consistent (meter/smoke/impairment read the total; the authority's
  epoch teleport carries the wreck's reset) but unexercised
  end-to-end — no retail car has taken a replicated wreck on a live
  session.
- F25-B remaining scope: replicated *result/race* state, damage
  *event* replication (sparks/texel/breakaway — needs per-impact
  positions on the wire), deduplicated audio/effects, the measured
  AC03 impairment matrix, bandwidth/update-rate budgets. AC01–AC06
  stay open as before; this slice advances req 3's damage leg only.

---

# Last iteration — F25-B fourth slice: replicated drive-presentation
# state — protocol v7 gives `SnapEntry` a wheel/engine tail so remote
# copies steer, spin, settle their suspension and light their pedals
# (iteration 15, run 20261001T195454-62282 continued)

Implementation iteration on `ralph/night` (baseline `6816fc3` — the
two-process driving candidate; external verify + review pass with gaps
only). One coherent slice: the wheel/engine leg of spec req 3 — remote
kinematic copies already received pose/velocity snapshots but their
`VehicleState`/`VehicleInput` were dead fields nobody wrote, so the
copy's wheels sat at full droop, never steered or spun, and the
brake/reverse glows never lit.

## Task selection

No failing gate or blocking finding — the previous review passed with
gaps only. Of F25-B's remaining scope (replicated damage/result/
wheel-engine presentation, the AC03 measured matrix, dedup'd effects,
bandwidth budgets), the presentation tail is the remaining *production*
gap the review's "replicated remote damage/result/wheel-engine
presentation" names — the matrix is queued as the next test-side slice.

## What landed

- `mm2_net` protocol v7 (`proto.rs`): `SnapEntry` gains `steer` (i16
  milliradians, saturating), `spin` (i16, 0.1 rad/s, mean grounded-wheel
  rate), `compression` (u8, mean droop fraction ×255) and `flags`
  (brake/reverse/grounded). Strict encode/decode; every fixture
  updated.
- `mm2_vehicle`: `RemoteReplica` marker — a client-side remote copy is
  kinematic and carries `VehicleState`/`VehicleInput` the local
  `vehicle_simulation` would otherwise stomp with dead-input results
  between snapshots; the query now excludes `RemoteReplica`. The
  host's authoritative seats never get the marker and still simulate.
- `mm2_app::netdrive`: `encode_present` on publish (authoritative
  steer angle — assists/rate limits included — mean grounded-wheel
  `vel_long/radius`, per-wheel compression normalised by travel,
  brake/direction/grounded flags); `apply_present` on receipt writes
  the copy's `VehicleState`/`VehicleInput` with wire values clamped to
  the pick's steer/travel bounds — informational only, pose still
  belongs to the lerp; `RemoteDrive::spin_rate` integrates the
  replicated rate into `WheelState::spin` inside `drive_remote_lerp`
  so wheels keep turning between ~20 Hz snapshots; the epoch-equal
  own-seat entry keeps ignoring the tail (local prediction owns it).
  `NetDriveReport.remote_spin` counts integrated radians; the smoke
  `net=` field gains `rspn<n>` — on the real driving legs the clients
  report nonzero (the host's own copy of truth is simulation, so its
  `rspn` stays 0 and `net_drive` asserts it on clients only).
- Tests: netdrive units +3 (tail encode incl. saturation and the
  airborne-freezes-spin leg, apply incl. hostile-value clamping);
  `net_app` extended + new — the client leg's tick-7 snap carries a
  live tail and asserts steer/compression/grounded/brake land on the
  copy's state, the spin rate integrates over updates, and a junk tail
  on the epoch-equal own-seat entry stays ignored; new host leg
  `a_snap_carries_the_remote_cars_drive_state` drives a real wire
  `DriveInput` + hand-written authority truth through the real
  `publish_snapshots` encode and asserts the decoded tail; `net_drive`
  parses `rspn` and requires `remote_spin > 0` in both process-level
  client records.

## Gates

`cargo fmt --all -- --check` clean; `cargo clippy --locked --workspace
--all-targets --all-features -- -D warnings` clean; `cargo test
--locked --workspace` green — 89 test binaries, 0 failures (`net_app`
33/33 incl. the new host-side tail leg, `net_drive` 2/2 in ~4 s).

## Classification / remaining open items

- Implementation choice throughout (protocol and component shape);
  the presentation fields replay simulation output, no original-behavior
  claim.
- Evidence: in-process integration + unit legs plus two real-process
  loopback sessions asserting `rspn` — synthetic dev world only, no
  LAN, no retail install, nothing rendered or driven by hand.
- F25-B remaining scope: replicated damage/result state, deduplicated
  audio/effects on remote cars, the *measured* AC03 impairment matrix
  (harness exists; one recipe exercised), bandwidth/update-rate
  budgets. AC01–AC06 stay open as before; this slice advances req 3's
  wheel/engine leg only.

---

# Last iteration — F25-B third slice: the first two-process driving
# evidence — `mm2 --host`/`mm2 --join` as separate OS processes, clean
# and through a live `ImpairProxy` (iteration 14, run
# 20261001T195454-62282 continued)

Implementation iteration on `ralph/night` (baseline `083ed24` — the
F25-B second-slice candidate; external verify + review pass with gaps
only). One coherent slice: the multi-process side of F25-AC01/AC02 —
until now every driving leg ran the host and clients in one test
process, so "separate host/client processes" had no coverage at all.

## Task selection

No failing gate or blocking finding. The F25-B review's sharpest gap
was that all evidence stayed in-process loopback — no two-process
session, impaired or clean. That gap is now cheap to close honestly:
`mm2 --host --headless` and `mm2 --join --headless` already exist with
the full data plane wired (inputs up, snaps down, remote reconcile,
reset requests), and each prints a `smoke=` record carrying the `net=`
counters — so a process-level test can assert what the wire moved
rather than what an in-process mailbox held.

## What landed

- `crates/mm2_app/tests/net_drive.rs` — two legs over real OS
  processes and real loopback sockets:
  - `two_mm2_processes_drive_one_session_over_loopback` — one
    `--host --headless` (9000-frame budget so the parked lobby
    outlives the clients) plus two `--join --headless --ready`
    clients on a synthetic dev-world install. The host's record stream
    gates the start (`ready=true` ×2 → `start` → `event=started
    generation=1`). Each client's mid-session record proves the full
    data plane: inputs streamed (`in<N>s`), snapshots applied
    (`snap<N>a`), remote copies reconciled, own predicted seat driven
    (`moved=` 197 m on the scripted driver). Both exits land on the
    host as `cause=quit` — a deliberate `Leave`, not a lost socket —
    and the host's `quit` record shows the authority side
    (`in<N>a`/`snap<N>s` — e.g. `in0s/1095a/1x,snap19345s/0a` on the
    manual rehearsal).
  - `an_impaired_two_process_session_still_converges` — the same
    session with every client connection relayed through one
    `ImpairProxy` (seed `0xC0FFEE`). The lobby crosses clean — the
    `Up` recipe arms only after both `ready` echoes prove the verbs
    delivered, `Down` arms after `event=started` plus a settle, since
    a one-shot verb has no retransmit — then the whole driving phase
    rides delay 40 ms + jitter 30 ms + loss 5% + duplicate 10% +
    reorder 10% in *both* directions. Both clients still converge and
    `LinkStats` asserts the recipe fired on the wire (drops/dups/
    reorders nonzero in each direction). This is the first end-to-end
    leg where `loss` is armed — the earlier net_app leg left it out by
    design (a dropped `ResetRequest` is an unanswered press); the
    input/snap stream tolerates it.
- `crates/mm2_app/src/smoke.rs` — the parked-lobby verdict now
  carries `net=` too. A hosted session that ran and then quit (the
  `quit` → `Cancel` → teardown → parked `Menu` path) used to report
  only `lobby closed`; now the record shows what its data plane
  moved, which is what `quit_and_assert_host_drove` reads.

Two things the first run taught the test:

- `mm2`'s tracing logs share stdout with the record stream — records
  are scanned for (`until("listening=")`), never assumed first.
- `rem<N>` on a cap record is *final-state*: a peer that caps and
  leaves first is correctly roster-pruned and its remote despawns
  before the later client's record prints. Client frame caps are
  therefore staggered (bob 900/1100 < alice 1400/1600): the earlier
  record pins `rem>=2` while both peers are connected — covering
  AC01's "visibly distinct cars" count deterministically — and the
  later record pins `rem>=1`. The despawn-on-leave itself is the
  designed behavior F25-AC05's disconnect leg wants, observed here on
  the wire for the first time.

## Gates

`cargo fmt --all -- --check` clean; `cargo clippy --locked --workspace
--all-targets --all-features -- -D warnings` clean; `cargo test
--locked --workspace` green — 91 suites, 0 failures (`net_drive` 2/2
in ~7.5 s).

## Classification / remaining open items

- Implementation choice throughout — test scaffolding on the
  already-shipped data plane; no original-behavior claim.
- Evidence level is *separate OS processes over loopback* on a
  synthetic dev world: the first non-in-process driving legs, and the
  first with loss armed end-to-end. Still no LAN leg, no rendered
  observation, no retail install, no human-driven input.
- F25-AC03 stays open — one recipe over a live session is not the
  recorded delay/jitter/loss matrix with measured corrections and
  stale-state behavior.
- F25-AC01/AC02 are *advanced* but not claimed: three real processes
  drove one authoritative session with two visibly distinct clients,
  but AC01 wants city/retail-rendered proof and AC02's collision+reset
  convergence is untested at process level. AC04/AC05/AC06 remain
  open; B's remaining scope is unchanged (replicated damage/result/
  wheel-engine presentation, deduplicated audio/effects, bandwidth/
  update-rate budgets).
- Known harness caveats stand: `LinkStats` direction aggregation
  assumes up-then-down lane spawn order; reset dedup is
  cooldown-based; a blocked lane reader relies on socket shutdown.

---

# Last iteration — F25-B second slice: the deterministic impairment
# harness, plus the mailbox ordering rules it makes load-bearing
# (iteration 032, run 20261001T195454-62282 continued)

Implementation iteration on `ralph/night` (baseline `2b5dc1f` — the
start-gate repair candidate; external verify + review pass with gaps
only). One coherent slice of F25-B: the seeded impairment harness the
spec's req 6 names and F25-AC03 needs, plus the two mailbox guards an
impaired wire exercises for real, plus the review's two naming/doc
nits.

## Task selection

No failing gate or blocking finding — the F25-B review listed
verification gaps (no impairment matrix, no seq freshness check) and
two nits (`#[allow]` justifications, `RemoteInputs::len`/`is_empty`
counting only input slots). The impairment harness was the highest-value
ready item: it is a spec-named requirement, it is what turns AC03's
"documented latency/jitter/loss matrix" from hand-waving into a
measurable run, and it surfaced two real ordering holes — `seq` existed
on `DriveInput` but nothing enforced it, and the reset mailbox took
arrival order rather than highest generation. All three share one
abstraction boundary, so they landed as one slice.

## What landed

- `crates/mm2_net/src/impair.rs` — `ImpairProxy`, a framed TCP relay:
  peer dials the proxy's loopback address, the proxy dials the target,
  and each direction of each connection is a *lane* (a blocking reader
  thread feeding a bounded `sync_channel`, and a writer thread that
  classifies, schedules and emits). `Impair` is the recipe: `delay` +
  uniform `jitter` set a per-frame release instant (jitter past a
  neighbour's release is a genuine reorder), `loss` drops whole frames,
  `duplicate` emits an adjacent copy, `reorder` holds a frame to swap
  with its successor (bounded by `HOLD_CAP` when the successor never
  comes — a held stream cannot stall). Draws come from a per-lane
  SplitMix64 seeded at construction — `lane_seed(seed, conn, dir)` —
  so one seed replays an identical pattern over identical traffic;
  `set()` retunes a live direction so legs handshake clean and impair
  only the session phase; `LinkStats` per lane slot (summed per
  direction on read) records in/out/dropped/duplicated/reordered/
  overflowed so a leg proves the impairment happened. `MAX_QUEUED`
  bounds pending frames; `Drop` shuts down every relayed socket (the
  only thing that wakes a `read_frame`-blocked reader) then joins
  every thread.
- `crates/mm2_net/src/lobby.rs` — two ordering hardenings the harness
  makes load-bearing (ordered TCP produces neither defect in practice;
  the impaired legs and any future unordered transport do):
  - `RemoteInputs::store` refuses a sample whose `seq` is not strictly
    ahead of the stored one — duplicates and arrival-order regressions
    can no longer regress the slot. The staleness clock stays the
    stored sample's arrival `Instant`.
  - `RemoteInputs::request_reset` keeps the *highest* generation per
    slot — a stale ask reordered behind a fresher one never masks it.
  - `len`/`is_empty` → `input_len`/`is_idle` (review nit): pending
    asks are mail too, so the honest "empty" question is both maps.
- `crates/mm2_net/src/proto.rs` — `DriveInput::seq`'s doc now states
  the enforced contract (monotonic per client, freshness tag the
  mailbox enforces) instead of "telemetry only".
- `crates/mm2_app/src/netdrive.rs` — the review's missing `#[allow]`
  justification comments (`reconcile_remote_players`, `spawn_remote`,
  `apply_reset_requests`).
- `docs/research/net.md` — an "Impairment harness (F25-B)" section and
  the evidence paragraph extended.
- `crates/mm2_app/tests/net_app.rs` +`an_impaired_link_still_converges_
  the_data_plane` — the full session through a live proxy: lobby
  crosses clean, then `set` arms Up with delay+jitter+dup+reorder and
  Down with delay+dup; 12 inputs ride the storm and the hosted car
  still drives on `seq == 12`; `Snap`s cross impaired; one
  `ResetRequest` — duplicated and reordered on the wire — is granted
  exactly once (`requests_granted == 1`, one epoch bump, seat back on
  its grid slot) even after the duplicate copies drain. `LinkStats`
  asserts the recipe really fired.

A real bug the first run caught: the accept loop polls its listener
nonblocking, and on macOS an accepted socket inherits the flag — the
lane readers died on `WouldBlock`, and `finish()` then flushed delayed
frames early (the delay leg's "arrived in 2 ms" was the tell). `relay`
now restores blocking I/O before the lanes spawn.

## Gates

`cargo fmt --all -- --check` clean; `cargo clippy --locked --workspace
--all-targets --all-features -- -D warnings` clean; `cargo test
--locked --workspace` green — 90 suites, 0 failures (mm2_net 80/80
incl. the impair and mailbox legs, net_app 32/32 incl. the impaired
session leg in 8.37 s).

## Classification / remaining open items

- The harness is implementation choice end to end — test scaffolding,
  no original-behavior claim; MM2's DirectPlay netcode is out of scope.
- F25-AC03 still open: the harness exists and is exercised, but the
  *recorded* impairment matrix (a delay/jitter/loss grid over a driving
  session with measured corrections and stale-state behavior) has not
  been run. Loss was deliberately left out of the net_app leg — a
  dropped `ResetRequest` is a press nothing answered by design, so a
  loss-matrix leg belongs with the measured matrix, not an assertion
  about timing.
- Evidence stays in-process loopback: no two-process impaired session,
  no LAN leg, no rendered observation. F25-AC01/AC04/AC05/AC06 remain
  open; B's remaining scope is replicated damage/result/wheel-engine
  presentation, deduplicated audio/effects, and the bandwidth/
  update-rate budget doc.

---

Repair iteration on `ralph/night` (baseline `b73f875` — the F25-B
first-slice candidate; external verify **failed**: `cargo test
--locked --workspace` exit 101 —
`r_under_a_remote_session_asks_the_authority` panicked `no host event:
Timeout` at `net_app.rs:188`, i.e. `until_started` never saw
`Started`).

## Root cause

Not the feature — a test-side race shared by every `link.ctl().set_*`
→ `host.start` leg. `set_vehicle`/`set_ready` write to the socket; the
host's per-peer reader thread forwards `PeerMessage` into the loop's
control channel, while `host.start` sends `LoopMsg::Start` from the
test thread — two producers racing on one channel. The bare
`app.update()` between them only pumps the *client* app; when `Start`
wins, the loop's start gate sees a not-ready player and emits
`StartRefused` ("alice is not ready"), which `until_started` discarded
until `recv_timeout` fired — a 15 s stall instead of the real reason.
The older legs carried the same latent race; the new client-leg test
lost it under verify load. `ready_peer`, the `net_host` record legs
and the mm2_net lobby tests were already synchronized — they wait for
the roster broadcast, the `event=ready` record line, or per-event
`recv`s. No production code is implicated: an operator's start is
human-timescale after readying, and the dedicated host reads its own
event confirmations.

## What landed

- `net_app.rs` +`until_ready` — the `LobbyLink` half of `ready_peer`'s
  discipline: `app.update()`s until `LobbyState.roster` echoes our own
  slot `ready` with a pick. The loop broadcasts the roster only
  *after* applying each `SetVehicle`/`SetReady`, so the echo is the
  happens-before `host.start` needs. Applied at all six racy sites
  (`a_start_begins_the_wired_session`,
  `a_cancel_returns_the_session_to_the_lobby`,
  `a_start_mid_session_parks_until_teardown_lands`,
  `losing_the_host_tears_down_and_exits`,
  `a_remote_drivers_reset_request_resets_its_seat`,
  `r_under_a_remote_session_asks_the_authority`), replacing the bare
  `app.update()`.
- `until_started` now panics on `StartRefused` with the host's reason
  — a refused start is the answer, not a 15 s wait for `Started`.

## Gates

`cargo fmt --all -- --check` clean; `cargo clippy --locked --workspace
--all-targets --all-features -- -D warnings` clean; `cargo test
--locked --workspace` green — 90 suites, 0 failures; `net_app` 31/31
in 8.18 s (the failed run burned 15 s on the timeout alone).

## Classification / remaining open items

- Test-only change — no production behavior moved, no gate muted, no
  assertion narrowed; the refusal path itself is unchanged (the
  mm2_net `StartRefused` legs already cover it). The repair makes the
  ordering deterministic rather than retrying the racy command.
- F25-B's open scope is unchanged: replicated damage/result/
  wheel-engine presentation for remote drivers, deduplicated
  audio/effects on remote cars, the impairment harness
  (delay/jitter/loss/duplication/reorder — spec req 13), bandwidth/
  update-rate budgets. Evidence stays synthetic/loopback — no
  two-process driving session, no retail-install leg, no rendered
  observation. F25-AC01..AC06 remain open.

---

# Last iteration — F25-B first slice: the wire-carried driver reset
# request — `R` under a predicted session asks the authority to reset
# its seat (iteration 030, run 20261001T195454-62282 continued)

Implementation iteration on `ralph/night` (baseline `7bd13ac` — the
iteration-029 repair candidate; external verify + review pass with
gaps only). One coherent slice: the reset-request verb the A-ledger
repeatedly named as F25-B's opening move.

## Task selection

F25-A.5's authority gate made `R` provably inert under `Remote`
authority — correct (a local teleport is a self-teleport the host can
never declare) but dead-feeling: the driver presses the key and
nothing happens until a detector fires. The F25-A row's remaining
scope carried exactly this fix: *a wire-carried driver reset request —
the R-gate's UX successor*. Everything downstream already exists —
the authority's `ResetVehicle` → `vehicle_reset` →
`track_reset_epochs` → `Snap` path lands a declared teleport on the
owning client since A.5/A.6 — so the missing piece was only the
client→host verb and a bounded host-side grant path. TASKS.json
carries F25-B as the next queued feature and this is its smallest
coherent first slice.

## What landed

- `mm2_net` — protocol v6: `Message::ResetRequest { generation }`,
  a fixed 8-byte client→host message. It carries no target field on
  purpose: the sender's roster slot names the seat, so a request can
  only ever ask for the sender's own car.
- `mm2_net::lobby` — `RemoteInputs` gains a reset mailbox beside the
  input mailbox: per-slot latest-wins, bounded by `MAX_PLAYERS`,
  absorbed by the reader threads (the lobby event loop never wakes
  for one), pruned when a player departs; `drain_resets` hands the
  batch to the sim. `ClientCtl::request_reset(generation)` sends it.
- `mm2_app::netdrive::send_reset_request` (client) — the same
  key/pad edge `reset_input` reads (`R` / `pad::RESET`), gated to
  predicted authority + `Playing` + a live link; sends the running
  generation and counts `requests_sent`. Fire-and-forget by design —
  the answer is the seat's epoch-declared `Snap`, which A.5's
  own-seat reconcile already applies like any authority reset.
- `mm2_app::netdrive::apply_reset_requests` (host) — drains the
  mailbox once per update after `drive_host`. A request drops (never
  queues) on: a generation that is not the running session's, a
  not-`Playing` phase, a sender with no spawned participant, or an
  ask inside `RESET_REQUEST_COOLDOWN` (1 s per seat — designed; wire
  asks arrive at socket rate, not key-edge rate, so an unbounded
  grant would let one client teleport-lock its seat every update).
  A fresh grant computes the *requesting* seat's shared grid slot
  (`seat_ids`/`seat_pose`, the map every process resolves
  identically) with the reconcile's hull-clearance lift and emits a
  targeted `ResetVehicle` — scheduled `.before(vehicle_reset)` so
  A.6's ordering contract carries it: the teleported pose and the
  bumped epoch leave on the same `Snap`.
- Wiring — `send_reset_request` joins the client lobby systems in
  `main.rs`/`smoke.rs`; `apply_reset_requests` joins the host side
  ordered after `drive_host` and before `vehicle_reset` in both.
  `NetDriveReport` gains `requests_sent`/`requests_granted`/
  `requests_dropped`, surfaced on the smoke record's `net=` field
  as `req<n>s/<n>g/<n>d`.
- `docs/research/net.md` — the protocol section moves to v6 with
  the v3→v4/v4→v5/v5→v6 history named (it had stalled at v3 through
  the F25-A run), and the lobby diagram lists the absorbed
  `Input`/`ResetRequest` arrows.

## Tests

- `mm2_net` +4 — wire round-trip/strict decode, latest-wins collapse
  + cap, absorb-into-mailbox over a real socket pair, departed peer
  prune.
- `net_app` +2 — `a_remote_drivers_reset_request_resets_its_seat`
  (host leg through the production schedule: an ask drained pre-
  `Playing` drops; the granted ask teleports the remote seat back to
  its grid slot with `Teleported` + `ResetEpoch` 1, and the same
  `Snap` carries pose *and* epoch; a cooldown repeat and a foreign
  generation drop) and `r_under_a_remote_session_asks_the_authority`
  (client leg: `Menu`-phase `R` sends nothing, `Playing` `R` lands
  exactly one ask in the real host's mailbox keyed to our roster
  slot, a held key resends nothing).

## Gates

`cargo fmt --all -- --check` clean; `cargo clippy --workspace
--all-targets --all-features -- -D warnings` clean; `cargo test
--workspace` green — all suites pass (mm2_net 61/61, net_app 31/31,
session 29/29, drive 17/17).

## Classification / remaining open items

- The request verb, the mailbox, the cooldown and the drop policy
  are implementation choice / designed policy — the original's
  networked driver-reset rule is unrecovered; spec req 4 wants the
  authority to own teleports and this keeps it so. No reply message
  exists by design: a granted ask is answered by the epoch-declared
  `Snap` itself; a dropped ask is a press nothing answered (a lobby
  notice is future UX work).
- F25-B remains open: replicated damage/result/wheel-engine
  presentation for remote drivers, deduplicated audio/effects on
  remote cars, the impairment harness (delay/jitter/loss/duplication/
  reorder — F25 spec req 13), and documented bandwidth/update-rate
  budgets. `mm2-host` the dedicated binary absorbs requests into its
  mailbox but never drains them — it owns no simulation, so resets
  there are out of scope by design.
- Evidence stays synthetic/loopback — the new legs run a real
  socket pair and the real reader/mailbox/schedule path in one
  process; no two-process driving session, no impairment matrix, no
  retail-install leg (no content paths changed), no rendered
  observation. F25-AC01..AC06 remain open.

---

# Last iteration — external-review repair: `dev_reset_at` joins the
# authority gate — `--join --reset-at` can no longer self-teleport a
# predicted seat (iteration 029, run 20261001T195454-62282 continued)

Repair iteration on `ralph/night` (baseline `f6782a1` — the F25-A.7
candidate; external verify green, review **fail** on one blocking
finding inside the candidate's own closing claim).

## Task selection

The A.7 review refuted the iteration's closing claim that every local
`ResetVehicle` writer is "authority-gated or wire-impossible":
`session::dev_reset_at` carried no authority check and *is* reachable
under `Remote` — `--reset-at` is absent from `--join`'s conflicts
list, `LobbyLink::join` hands `cli.dev` to the link, and `net::start`
deliberately stamps `config.dev = link.dev.clone()` onto the accepted
config so evidence flags still apply locally. `mm2 --join <addr>
--reset-at N` therefore fired the shared `spawn_resets` bundle under
predicted authority — a local self-teleport of the predicted own seat
that the host never declares, while the client's `ResetEpoch` never
moves (`track_reset_epochs` is host-only) and epoch-equal own-seat
`Snap` entries stay ignored: exactly the permanent divergence the A.5
`R` gate closed. The asymmetry with `reset_input` — same bundle,
gated since A.5 — made it an oversight, not a designed dev exception.
Repair ahead of any new feature work; no unrelated work taken.

## What landed

- `session::dev_reset_at` early-returns under
  `!session.authority_role().is_authority()` — the same predicate in
  the same position as `reset_input`'s gate. The flag keeps its
  evidence role on authoritative sessions (`Local`; a `Host`
  advertisement could never carry it — `net::advertise` refuses dev
  overrides — but the gate sits on the authority boundary like every
  other writer) and is inert under `Remote`.
- The system's doc now records the reachability — `--reset-at` does
  not conflict with `--join`, and `net::start` stamps the client's
  `dev` flags onto the accepted config — so the gate reads as
  deliberate rather than belt-and-braces.
- Ledger correction in place: the A.7 entry/row's "or wire-impossible
  (dev overrides refused by `net::advertise`)" was false for the
  client path — that refusal only covers *hosting*.

## Tests

- `session` +1 — `reset_at_is_inert_under_remote_authority`: a
  `Remote` session carrying `dev.reset_at` drives past the tick with
  zero `ResetVehicle` messages, no `Teleported`, the predicted car
  never returning to spawn; the `Host` leg still fires through the
  production path (`Teleported` + back on the spawn point), proving
  the gate is the authority boundary rather than dev flags dying
  under networking.

## Gates

`cargo fmt --all -- --check` clean; `cargo clippy --locked --workspace
--all-targets --all-features -- -D warnings` clean; `cargo test
--locked --workspace` green — 90 suites, 0 failures (session 29/29).

## Classification / remaining open items

- Implementation choice encoding the same designed policy as the
  other reset gates (a predicted session never self-teleports; spec
  req 4). `dev_finish`'s direct `Position` write remains the one
  disclosed dev exception — it was never a `ResetVehicle` writer, so
  it sits outside the refuted claim's scope, and its Local-only
  reasoning stands as documented in A.6/A.7.
- All other A.7 disclosures stand: FixedLast-writer same-`Snap`
  coherence is reasoned from schedule order, gyro-ledger preservation
  across resets is designed with its original-rule status unverified,
  and the open F25-A scope is unchanged (drift reconciliation, input
  replay, replicated damage/result presentation, rate limiting,
  interpolation tuning, the F25-B wire reset request).
- Evidence stays synthetic/loopback — no two-process driving session,
  no impairment matrix, no retail-install leg (no content paths
  changed), no rendered observation. F25-AC01..AC06 remain open.

---

# Last iteration — F25-A.7: the self-right assist joins the authority
# gate — a predicted session never self-teleports (iteration 028, run
# 20261001T195454-62282 continued)

Implementation iteration on `ralph/night` (baseline `96cfc93` — the
F25-A.6 candidate; external verify + review pass with gaps only). One
coherent slice: the A.6 review's named divergence gap.

## Task selection

A.6 established "an authority-side teleport is a declared reset" and
gated the `R` bundle; the review then observed the other in-crate
writer was still live under a predicted session: `vehicle_self_right`
emitted a local-only `ResetVehicle` on remote kinematic copies and the
predicted own seat under `Remote` authority. On the own seat that is
exactly the self-teleport the A.5 gate forbade — the host's copy never
learns it, and the epoch-equal `Snap` entries that would correct it
stay ignored, so the divergence was permanent; on a remote kinematic
copy the local flop fights the next `Snap`'s blend. `mm2_vehicle`
cannot see `Session` (the crate boundary), so the authority reaches it
as a resource the app stamps — the `TireConditions` pattern.

## What landed

- `mm2_vehicle::vehicle::ResetAuthority(pub bool)` — whether this
  process may originate a `ResetVehicle` itself. `VehiclePlugin` inits
  it `true`: a standalone world and any authoritative session
  (`Local`, `Host`) resolve their own teleports, and every harness
  that never loads a networked session keeps its assists. `Remote`
  stamps `false`.
- `vehicle_self_right` early-returns under the gate — no
  `ResetVehicle`, no `upended_for` accumulation. The remote driver's
  flip is resolved by the host's own detectors and carried back as
  the declared epoch, not flopped locally.
- `mm2_app::session::load_session_world` stamps
  `ResetAuthority(config.authority.is_authoritative())` beside the
  `TireConditions` write — re-stamped on every load so a networked
  session's gate cannot leak into the next local one. Every local
  `ResetVehicle` writer is now authority-gated (`reset_input`, the
  scripted/opponent re-anchors, self-right — and `dev_reset_at`,
  gated in the iteration-029 repair above: the original claim's
  "or wire-impossible — dev overrides are refused by
  `net::advertise`" was false for the client path, since
  `net::start` stamps `link.dev` onto the accepted `Remote` config;
  `dev_finish`'s direct `Position` write stays the disclosed
  exception).

## Tests

- `drive` +1 — `an_upended_car_stays_down_without_reset_authority`:
  `ResetAuthority(false)` + a roofed car waits out the authored delay
  with no flop, no `Teleported`, the detector never arming.
- `session` +1 — `self_right_is_inert_under_remote_authority`: the
  production `load_session_world` stamps the gate — `Remote` →
  `ResetAuthority(false)`, no `ResetVehicle` written, no `Teleported`,
  the predicted car still upended; `Host` → `true`, the flop still
  lands through `vehicle_reset` (the hosted remote drivers' recovery
  path stays live).

## Gates

`cargo fmt --all -- --check` clean; `cargo clippy --locked --workspace
--all-targets --all-features -- -D warnings` clean; `cargo test
--locked --workspace` green — 90 suites, 0 failures (drive 17/17,
session 28/28).

## Classification / remaining open items

- The gate is an implementation choice encoding the same designed
  policy as the R-gate (a predicted session never self-teleports);
  self-right staying live on the authority for remote drivers' cars is
  the designed recovery path from A.4.
- Still open F25-A scope: continuous drift reconciliation between
  epochs, full input-replay prediction, replicated damage/result
  presentation, input rate-limiting, interpolation tuning, and the
  wire-carried driver reset request (F25-B). The A.6 review's other
  disclosures stand: `dev.finish`'s direct `Position` write is the
  deliberate Local-only dev-flag exception (its swept segment is the
  point — a `Teleported` hop would bank nothing); gyro-ledger
  preservation across resets is designed, its original-rule status
  unverified; FixedLast-writer same-`Snap` coherence is reasoned from
  schedule order, not yet test-asserted.
- All evidence remains synthetic/loopback — no two-process driving
  session, no impairment matrix, no retail-install leg (no content
  paths changed), no rendered observation. F25-AC01..AC06 stay open.

---

# Last iteration — F25-A.6: every authority teleport is a declared
# reset — self-right joins the `ResetVehicle` lifecycle and the epoch
# tracker can no longer trail the pose (iteration 027, run
# 20261001T195454-62282 continued)

Implementation iteration on `ralph/night` (baseline `aa4af45` — the
F25-A.5 candidate; external verify + review pass with gaps only). One
coherent slice: the three gaps A.5's review named, all facets of one
invariant — *an authority-side teleport must be a declared reset*.

## Task selection

The A.5 review passed the change but found the epoch lifecycle had
holes: (1) `vehicle_self_right` teleported an upended car in place —
no `ResetVehicle`, no `Teleported`, no epoch bump — so a client's copy
blended the flop instead of receiving a declared snap; (2) the
Update-scheduled reset writers (`reset_input`, `dev_reset_at`,
`scripted_drive`, `opponent_drive`) were unordered vs
`track_reset_epochs`, so a reset's epoch could ride the `Snap` *after*
the one carrying the teleported pose; (3) the new `reset_input`
authority gate had no negative test. One repair covers all three:
route self-right through the message like every other teleport, then
make the writer → apply → track → publish order explicit.

## What landed

- `mm2_vehicle::vehicle_self_right` — no longer writes
  `Position`/`Rotation`/velocities/`Transform` itself. It emits a
  targeted `ResetVehicle` on the shared `upright_recovery_pose`
  landing and the plugin chains it before `vehicle_reset`
  (`(vehicle_self_right, vehicle_reset).chain()`), so the flop applies
  the same frame through the single apply point — `Teleported`-marked
  for swept-segment consumers and visible to the wire epoch tracker.
- `mm2_vehicle::vehicle_reset` — preserves `gyro_spins` /
  `gyro_completed` across the `VehicleState` rebuild: now that
  self-right routes through it, wiping the ledger would erase a flip's
  own maneuver history. The counters are run evidence, not sim state.
- Schedule ordering — identical in `main.rs` and `smoke.rs`: every
  Update-scheduled `ResetVehicle` writer is `.before(vehicle_reset)`
  (`reset_input`, the `dev_reset_at` chain, `scripted_drive`,
  `opponent_drive`; `vehicle_self_right` is chained inside the
  plugin), and `track_reset_epochs` is `.after(vehicle_reset)` +
  `.before(publish_snapshots)` with the publish also `.after` the
  apply. The epoch bump and the teleported pose now provably leave on
  the same `Snap`. FixedLast writers (`resolve_disabled`,
  `resolve_stuck`, `resolve_recovery`) needed no edge: fixed schedules
  run ahead of `Update`, so their messages were already readable in
  the frame that applies and tracks them.
- Test-harness wiring mirrors the production edges — `host_app` in
  `net_app.rs` schedules the real `reset_input` +
  `vehicle_reset` with the same constraints, so the legs observe real
  same-frame coherence rather than a test-only stream; `session.rs`'s
  `test_app` carries the same `.before(vehicle_reset)` edges.

## Tests

- `drive` (16, extended in place) — `an_upended_car_flops_back_onto_
  its_wheels` now asserts `Teleported` on the recovered car (proof the
  flop went through `vehicle_reset`, not an in-place write);
  `reset_teleports_and_clears_motion` seeds `gyro_spins`/`gyro_
  completed` and asserts both survive the reset.
- `session` (+1) — `r_is_inert_under_remote_authority`: a `Remote`
  (predicted) session drives off its spawn, then `R` writes **no**
  `ResetVehicle` message, stamps no `Teleported`, and the predicted
  car never moves back — the A.5 gate's missing negative leg.
- `net_app` (host leg extended) — the hand-written remote-seat reset
  now must arrive on the wire with pose **and** `epoch: 1` in the same
  `Snap`; and a host-side `R` keypress — the real `reset_input`
  writer — teleports the remote seat and the same `Snap` carries its
  new pose with `epoch: 2`. `NetDriveReport.resets` counts both.

## Gates

`cargo fmt --all -- --check` clean; `cargo clippy --workspace
--all-targets --all-features -- -D warnings` clean; `cargo test
--workspace` green — recorded below/verify log.

## Classification / remaining open items

- The shared-`ResetVehicle` discipline and the ordering edges are
  implementation choice; self-right riding the declared-reset path is
  designed policy consistent with spec req 4 (authority teleports are
  the wire's job to declare).
- Still open F25-A scope: continuous drift reconciliation between
  epochs, full input-replay prediction, replicated damage/result
  presentation, input rate-limiting, interpolation tuning, and a
  wire-carried driver reset request (the `R`-gate's UX successor —
  under `Remote` the key is now provably inert, which is honest but
  dead-feeling; the request message is F25-B).
- All evidence remains synthetic/loopback — no two-process driving
  session, no impairment matrix, no retail-install leg, no rendered
  observation. F25-AC01..AC06 stay open.

---

# Last iteration — F25-A.5: reset epochs on the wire — the authority's
# teleports reconcile the owning seat and snap remote copies
# deterministically (iteration 026, run 20261001T195454-62282 continued)

Implementation iteration on `ralph/night` (baseline `a31dc72` — the
F25-A.4 candidate; external verify + review pass with gaps only). One
coherent slice: the gap the review named "open and now observable" —
the host resolves a remote driver's reset, but the owning client's
predicted car never learned, so a wrecked remote driver stayed wrecked
in their own view forever.

## Task selection

F25-A.4's own remaining-items list carried "own-seat
prediction/reconciliation — the owning client does not yet learn it was
reset". A pose-difference heuristic cannot detect it: the commonest
authority reset is *in place* (the stuck/wreck resolve lands the car on
the same spot, uprighted) — `CORRECTION_SNAP_DIST` only sees teleports
with distance. So the wire now declares resets: `SnapEntry.epoch` is
the authority's per-seat reset counter.

## What landed

- `mm2_net` — protocol v5: `SnapEntry` gains `epoch: u8`, the seat's
  reset counter (wraps at 256 — only *difference* is read, so a wrap
  is a false snap at worst, never a missed reset).
- `mm2_app::netdrive::ResetEpoch` — one component on every participant
  (remote spawns and the `NetPlayer` stamping of the local car),
  meaning "the reset epoch this entity's pose reflects". On the host,
  `track_reset_epochs` reads the same `ResetVehicle` stream
  `vehicle_reset` applies (independent cursors — it can never steal a
  reset) and bumps the target's counter; `entity: None` bumps all.
  `publish_snapshots` stamps it, ordered after the tracker so a bump
  and its teleported pose leave on the same `Snap`.
- `apply_snapshots` — the epoch is a second snap trigger beside the
  20 m distance bound (which stays as the catch-all for teleports the
  epoch cannot describe). Remote copies snap on either; the **own
  seat** — the `PlayerControl::Local` car — takes the asserted pose and
  velocities outright when the epoch advances, marked `Teleported` so
  swept-segment consumers re-anchor. Epoch-equal own-seat entries stay
  ignored: between authority resets the local sim owns the pose, and
  blending toward a host copy that lags by the round-trip would
  rubber-band the driver.
- `input::reset_input` — the `R`/pad reset is now authority-gated.
  Under a predicted session a local teleport was exactly the
  self-teleport spec req 4 forbids: the host copy could never learn it
  and the two truths would diverge permanently. A remote driver's
  recovery is the authority's detectors (impact-armed stuck,
  water/out-of-bounds recovery, wreck resolve) answered by an
  epoch-declared reset. A driver-*requested* reset over the wire is
  named F25-B scope — the gate trades a working-looking but divergent
  key for an honest one.
- `NetDriveReport.resets` counts authority resets observed (host:
  tracked bumps; client: applied epoch teleports).

## Tests

- netdrive units +1 — `reset_vehicle_events_bump_the_seat_epoch`:
  targeted bump, non-participant target ignored, reset-all bumps all.
- `net_app` (29 tests, extended in place): host leg — a hand-written
  `ResetVehicle` on the remote car bumps `ResetEpoch` and the next
  wire `Snap` carries `epoch: 1` for that seat. Client legs — an
  epoch-1 own-seat entry snaps the local car's pose + velocities and
  stamps `Teleported`; an epoch-equal own-seat entry never moves it;
  an epoch bump snaps a remote copy at 5 m — under the blend bound —
  proving the declared reset beats distance.

## Gates

`cargo fmt --all -- --check` clean; `cargo clippy --locked --workspace
--all-targets --all-features -- -D warnings` clean; `cargo test
--locked --workspace` green (mm2_net 58/58, mm2_app lib 71, net_app
29/29, session 26/26 — recorded below/verify log).

## Classification / remaining open items

- The epoch mechanism is an implementation choice; the *policy* it
  encodes — authority resets reconcile the owning seat, epoch-equal
  divergence stays predicted-local — is designed, documented per spec
  req 2/4's leave to choose the prediction policy.
- Still open F25-A scope: continuous drift reconciliation between
  epochs (sub-epoch divergence is accepted — physics agreement bounds
  it in practice; a worst case is the 20 m snap bound firing), full
  input-replay prediction, replicated damage/result presentation for
  remote drivers, input rate-limiting, interpolation tuning, and a
  wire-carried driver reset request (the R-gate's UX successor).
- All evidence is synthetic/loopback; no retail-install leg (no
  content paths changed), no two-process or impairment-matrix run —
  F25-AC01..AC06 stay open.

---

# Last iteration — F25-A.4: authority-owned remote-driver outcomes —
# the host's damage/stuck/recovery pipeline resolves remote drivers
# (iteration 025, run 20261001T195454-62282 continued)

Implementation iteration on `ralph/night` (baseline `bbc7527` — the
F25-A.3 candidate; external verify + review pass with gaps only). One
coherent slice: `PlayerControl::Remote` conflated input ownership with
process authority — on a hosted session the host already simulates a
remote driver's car, so a remote who wrecks, wedges or sinks was never
resolved by anyone. The host's rule pipeline now owns those outcomes.

## Task selection

The F25-A row's remaining scope named it ("damage/stuck/recovery/result
replication for remote drivers") and the defect was structural: the
host ran every remote car's physics but skipped it in every outcome
system, so a remote wreck just sat there forever while its driver
waited on an authority that already was this process. The correct
split is `SessionAuthority`/`AuthorityRole` decides *whether* this
process resolves; `PlayerControl` only says whose hands the input came
from.

## What landed

- `mm2_app::damage` — `sync_impairment` drops its remote skip: the
  host weakens the engine of the remote car it simulates.
  `resolve_disabled` merges `Remote` into the AI arm — in-place
  reset + repair under every mode, never the local driver's event
  restart or a shared clock tax; the stuck-disarm broadens to every
  identified participant.
- `mm2_app::stuck` / `mm2_app::recovery` — the arm/observe/resolve
  legs drop their remote skips; a remote car arms off the real impact
  stream, fires on the authored window, recovers through the shared
  `ResetVehicle` path (production `Teleported` marking included).
  Predicted sessions still drain everything inertly — the session-level
  `authority_role().is_authority()` gate is the real boundary.
- `mm2_app::netdrive::spawn_remote` — remote cars now carry the
  authored `VehicleDamage`/`VehicleStuck` (recordless picks stay
  undamageable/unstuckable — no fabricated spec) and the designed
  `VehicleRecovery` detector, on both wire sides: inert under a
  predicted session, one spawn shape, and damage-state replication
  gets a landing place.
- `mm2_app::netdrive::apply_snapshots` — `CORRECTION_SNAP_DIST`
  (20 m, designed bound for spec req 4's bounded corrections): a
  teleport-scale correction snaps the copy to the asserted pose
  instead of blending a slide through the world. The authority's new
  resets produce exactly those corrections; sub-bound corrections
  still blend.
- Presentation systems keep their remote skips (smoke/sparks/texel/
  breakaway/impact-audio): remote-car effects are replicated
  presentation, F25-B/F26 scope, and remote spawns carry none of those
  rigs — the skips are belt-and-braces. Stale "their own authority
  renders it" comments updated to say so.
- Tests — `damage`/`stuck`/`recovery` fixtures take a caller
  `SessionConfig`; each suite gains a `Host` leg (remote car
  arms/impairs/resolves/recovers through the production path, no
  session restart) and a `Remote` leg (predicted session drains
  everything inertly — even a hand-written `RecoveryEvent` never
  resolves). `net_app` legs extended in place: the real
  `reconcile_remote_players` spawn asserts it carries
  `VehicleRecovery` while the recordless dev-car pick stays
  undamageable/unstuckable, and the snapshot leg asserts a
  teleport-scale correction snaps where a 4 m correction still
  blends.

## Gates

`cargo fmt --all -- --check` clean; `cargo clippy --workspace
--all-targets --all-features -- -D warnings` clean; `cargo test
--workspace` — all suites green (damage 15/15, stuck 12/12,
recovery 16/16, net_app 29/29 — unchanged count, two legs extended
in place).

## Classification / remaining open items

- The outcome split is designed policy: authority decides, not input
  ownership. A remote driver's wreck resetting in place + repairing
  under every mode (rather than ending their event) is designed —
  the original's MP wreck handling is unverified (UNK-13 territory);
  the alternative, one remote wreck restarting everyone's event, was
  rejected in the row above.
- Still open F25-A scope: own-seat prediction/reconciliation — the
  owning client does not yet learn it was reset (its copies of
  *other* cars snap correctly); replicated damage/result presentation;
  input rate limiting beyond latest-wins; interpolation tuning.
- All evidence is synthetic/loopback: the `Host`/`Remote` legs stamp
  authority on the session config; `net_app`'s snap legs ride a real
  loopback socket but a hand-broadcast `Snap`. No two-process driving
  evidence, no impairment matrix, no soak, no rendered observation —
  F25-AC02..AC06 stay open.

---

# Last iteration — F25-A.3: MP-4's ambient-traffic leg — networked
# sessions field no lane followers on either wire side (iteration 024,
# run 20261001T195454-62282 continued)

Implementation iteration on `ralph/night` (baseline `8ce7a18` — the
F25-A.2 candidate; external verify + review pass with gaps only). One
coherent slice: the follow-up A.2 disclosed — ambient traffic still
loaded under networked authority.

## Task selection

F25-A.2's own remaining-items list named it ("Ambient traffic still
loads under networked authority — MP-4 also removes it"), and the
external review confirmed the same gap. It is also a real coherence
defect, not just a missing rule: `load_ambient_traffic` gated on
`is_authority()`, which is true on the host — so a hosted session
simulated lane followers that no client replicates, and the
host-simulated remote cars could collide with traffic invisible to
their drivers. MP-4 is documented ("No ambient traffic, cops or AI
opponents in MP races; humans replace AI opponents" — help:
Multiplayer Games); cops have no runtime (F20) and pedestrians none
(F19), so this gate completes MP-4 for everything implemented today.

## What landed

- `mm2_app::traffic::load_ambient_traffic` — the authority gate
  tightens from `!session.authority_role().is_authority()` to
  `config.authority != SessionAuthority::Local`. Both wire sides now
  field the identical empty ambient world; `spawn_traffic_signals`
  rides inside the same early return. `Local` sessions are unchanged.
- `mm2_app::session` — the `load_session_world` call-site comment
  records that networked sessions get `None` under MP-4. Also a
  comment-only repair of the A.2 review's noted imprecision: a seat
  past the authored grid fans off the last row's right vector, not
  the roam `origin` (the no-grid case alone fans off the base).
- `tests/traffic.rs` — `a_networked_session_spawns_no_ambient_traffic`:
  the real `load_session_world` over the synthetic city install under
  `Host` and `Remote` authority — no `AmbientTraffic` resource, zero
  `AmbientCar`/`TrafficSignal` entities, the local car still loads.
  The networked legs strip dev overrides (`net::advertise` refuses
  them — a wire-legal config is the honest fixture), and a `Local`
  control leg on the same install still fills the seeded plan.

## Gates

`cargo fmt --all -- --check` clean; `cargo clippy --workspace
--all-targets --all-features -- -D warnings` clean; `cargo test
--workspace` — all 90 suites green (traffic 39/39, net_app 29/29,
mm2_net 58/58).

## Classification / remaining open items

- The no-ambient-traffic gate is documented original behavior (MP-4)
  — an original requirement, not designed policy. MP-4's "cops" clause
  has no consumer yet (F20); pedestrians have no runtime (F19).
- If F26 ever replicates ambient world actors, the gate is the single
  point to revisit (`load_ambient_traffic`'s early return); the
  runtime systems' `Option<Res<AmbientTraffic>>` reads stay inert
  without the resource.
- F25-A stays active: own-seat prediction/reconciliation, outcome
  replication (damage/stuck/recovery/results — the
  `PlayerControl::Remote` skips stay deliberate), input rate limiting
  beyond latest-wins, interpolation tuning.
- All evidence is loopback/synthetic; no retail-install leg this
  slice (the gate is authority-only — content paths unchanged) and no
  rendered observation.

---

# Last iteration — F25-A.2: shared seat/grid assignment — the lobby's
# humans take the authored start slots in wire-id order (iteration 023,
# run 20261001T195454-62282 continued)

Implementation iteration on `ralph/night` (baseline `8a081ca` — the
F25-A.1 candidate; external verify + review pass with gaps only). One
coherent slice: remote participants stop staging at a designed lateral
offset — every networked human resolves a deterministic seat on the
session's authored start grid, and MP-4 keeps AI opponents out of
networked races entirely.

## Task selection

F25-A.1's review left spawn-grid assignment as the named A-scope
remainder that was ready (own-seat prediction, outcome replication and
impairment testing are deliberately later). During design the rules
ledger settled one open question: MP-4 is *documented* — "No ambient
traffic, cops or AI opponents in MP races; humans replace AI
opponents." The AI-shift design was dropped in favour of a `Local`-
authority gate, which is also the only consistent behavior while
opponents are unreplicated (each process would otherwise simulate its
own divergent AI set).

## What landed

- `mm2_app::netdrive` — the shared seat map. `seat_ids` ranks the
  lobby's wire ids ascending: roster entries + our own id + wire id 0
  while a host seat exists (`HostLink` on a hosted app, the
  `Start`-carried `host_pick` on a joined client; a dedicated
  `mm2-host` seats nobody, so its first roster member takes seat 0).
  `seat_pose` resolves a seat identically on every process: authored
  `start_slots[seat]` verbatim — authored `yaw_deg`, or the
  `course_yaw` facing on the `a = 0` no-heading sentinel (WPT-4) —
  and past the grid a designed `SEAT_STAGE_GAP` fan-out continues off
  the last row's right vector; a race-less session fans off the roam
  base. `apply_seat` moves a `SpawnPoint` to a seat; `NetSeats` is the
  `SystemParam` the session load reads its own seat through.
  `remote_spawn_pose` and `REMOTE_SPAWN_GAP` are gone — the reconcile
  now feeds each remote `NetPlayer` through the same `seat_pose`.
- `mm2_app::session` — `SpawnPoint` gains `origin`/`origin_yaw`, the
  pre-seat roam base the fan-out anchors on (the authored slot grid is
  the anchor when a race ships one — a per-process `--spawn` override
  must never enter the shared map). `load_session_world` captures the
  base, seats the local car via `seats.self_seat()` (solo seats 0, the
  slot it took before), then still applies `--spawn` last as the dev
  override it always was. MP-4: `spawn_opponents` is gated on
  `SessionAuthority::Local` — `Host` and `Remote` event sessions field
  the lobby's humans only.
- `mm2_app::opponents` — `spawn_opponents` docs record the MP-4
  boundary; the `index + 1` slot rule is single-player-only now.

## Evidence

Synthetic tests:

- `netdrive` units +5 — seat-id ranking (joined-client, hosted-app,
  dedicated-host and self-dedup legs), authored-row order + `a = 0`
  course-facing fallback + grid-exhaustion fan-out + no-race fan
  (`seat_pose` legs), `apply_seat` preserves the roam-base origin.
- `event` +1 — `a_joined_client_seats_the_local_car_on_its_grid_row`:
  a real loopback `Host` + joined `LobbyLink` mint wire id 1; the
  `Remote`-authority `load_session_world` puts the local car on the
  authored grid's row 1 (position + verbatim 90° facing), not row 0.
- `opponents` +1 — `a_networked_event_spawns_no_ai_opponents`: the
  authored two-driver roster that spawns under `Local` spawns zero
  `OpponentDriver`s under both `Host` and `Remote` authority while
  the local car still loads.
- `net_app` — both data-plane legs now assert the seat pose: the
  host's remote car lands one `SEAT_STAGE_GAP` right of the roam base
  (wire id 1 → seat 1) and the client's host copy lands on the base
  itself (wire id 0 → seat 0) — the same mapping resolved on both
  sides of the wire.
- All `SpawnPoint` fixture literals across 20+ test files converted to
  `SpawnPoint::new` (origin = position for pre-load placeholders);
  the trailer fixture uses struct-update syntax.

## Gates

`cargo fmt --all -- --check` clean; `cargo clippy --workspace
--all-targets --all-features -- -D warnings` clean; `cargo test
--workspace` — recorded in the commit message / verify log.

## Classification

- Grid row consumption and the `a`/`yaw` conventions ride the WPT-3 /
  WPT-4 evidence; the original's row→participant mapping stays
  UNK-17 — wire-id rank order is designed.
- MP-4's no-AI gate is documented original behavior (help:
  Multiplayer Games); applying it is original requirement, not
  designed policy.
- The fan-out spacing and anchor are designed (`SEAT_STAGE_GAP`); no
  original equivalent is claimed.

## Remaining open items

- F25-A stays active: own-seat prediction/reconciliation, outcome
  replication (damage/stuck/recovery/results — the
  `PlayerControl::Remote` skips stay deliberate), input rate limiting
  beyond latest-wins, interpolation tuning.
- All evidence is loopback/synthetic; no retail-install leg this
  slice and no rendered observation — grid placement was verified at
  the ECS level.
- Ambient traffic still loads under networked authority (MP-4 also
  removes it — the ambient traffic path is a separate gate, left for
  a follow-up slice since divergent parked/lane cars are at least
  non-interactive today).

---

# Last iteration — F25-A.1: host-authoritative remote driving transport
# — inputs up, host-side remote sim, snapshots down (iteration 022,
# run 20261001T195454-62282 continued)

Implementation iteration on `ralph/night` (baseline `92a46f3` — the
F24-B.8 candidate; external verify green, review pass with gaps only).
One coherent slice: the first F25-A data-plane leg — remote lobby
players stop being roster/display state and become networked driving
participants over the socket the lobby already owns.

## Task selection

F24-B closed with one named gap: "remote players remain
roster/display state only — no spawn, interpolation, input transport
or replication exists to observe (explicit F25/F26 scope)". F25-A is
the next frontier and unblocked (F24-B landed). Scope is the minimal
authoritative data plane, deliberately not full multiplayer: no
client-side prediction or own-seat reconciliation, no damage/stuck/
result replication (the rule systems' `PlayerControl::Remote` skips
are preserved — resolving a remote driver's *outcome* needs wire
coordination a later slice adds), no LAN/Internet reachability
(F24-C), no DirectPlay compatibility (out of product scope).

## What landed

- `mm2_net::proto` — protocol v4. `DriveInput{generation, seq,
  throttle:u8, brake:u8, steer:i8, handbrake:u8}` (quantized integer
  controls) and `SnapEntry{player, pos, rot, vel, angvel}` (f32 pose)
  ride new `Message::Input`/`Message::Snap{generation, tick, entries}`
  frames — bounded encode/decode like every sibling; `Snap` entries
  are roster-capped (`MAX_PLAYERS`). `Message::Start` carries
  `host_pick: Option<VehiclePick>` — the host seat is never a wire
  roster entry (ids mint from 1; 0 is reserved), so its vehicle
  travels on the start frame.
- `mm2_net::conn` — `TCP_NODELAY` on lobby sockets: the data plane is
  latency-sensitive traffic, not a batch channel.
- `mm2_net::lobby` — `RemoteInputs`, the host's per-player input
  mailbox (`Arc<Mutex<BTreeMap<u16, StampedInput>>>`, latest-wins per
  roster slot, bounded by `MAX_PLAYERS`, arrival-time staleness clock).
  Per-peer reader threads absorb `Input` frames straight into the
  mailbox — they never queue behind lobby events — so a fast sender
  cannot pile up a backlog and a slow sender just leaves a stale
  sample. Departed slots prune on roster removal; teardown clears the
  map. `HostCtl::broadcast` queues an arbitrary host→all message for
  the event loop (snapshot publication path); `ClientCtl::send_input`
  is the client send. A failed broadcast write reaps the peer under
  the existing disconnect discipline and rebroadcasts the corrected
  roster.
- `mm2_app::netdrive` (new) — the app-side data plane:
  - `encode_input`/`decode_input` — `VehicleInput` ↔ `DriveInput`
    quantization (0–255 / ±127), `forced_gear` stays local.
  - `NetPlayer(u16)` stamps each participant's wire identity — the
    local car included, so the host's snapshot carries seat 0 and a
    client recognizes its own entry.
  - `reconcile_remote_players` — keeps the world equal to the lobby
    state: every picked remote roster slot (plus the `host_pick` seat
    on clients) spawns a session-owned participant (`PlayerControl::
    Remote`, minted ids, `DamageSignals`, the shared vehicle/spawn
    paths); a departure or changed pick despawns it. The authority
    role splits the spawn — on the host it is a dynamic simulated
    car, on a client a kinematic copy with a `RemoteLerp` blend.
  - `send_drive_input` (client) — the settled local `VehicleInput`
    becomes a generation-stamped `Input` frame per update while
    `Playing`.
  - `apply_remote_inputs` (host) — each remote car reads its mailbox
    slot; samples older than `INPUT_STALE` (250 ms) or from another
    generation zero the input — a stalled driver coasts, it never
    keeps its last throttle.
  - `publish_snapshots` (host) — every participant's `Position`/
    `Rotation`/velocities broadcast once per update, ticked by the
    session clock.
  - `RemoteSnaps`/`apply_snapshots`/`drive_remote_lerp` (client) —
    the latest staged snapshot retargets each copy's lerp (interval =
    the observed arrival gap); stale ticks and foreign generations
    drop, own-seat entries are received but never applied.
- `mm2_app::net` — `LobbyState.host_pick`; `drive_lobby` stages `Snap`
  into `RemoteSnaps`; `HostLink::open` advertises the host seat's pick
  via `Start`; `HostLink::remote_inputs` exposes the mailbox.
- `car_visual::spawn_dev_car` — the synthetic dev car's visuals
  extracted so remote dev-car picks build the same rig the local one
  does.
- `main.rs`/`smoke.rs` — both link arms wire the systems with the
  drain-before-consume ordering; the headless record gains
  `net=in<s>a/x,snap<s>a,rem<n>` (absent for non-lobby records —
  bit-identical otherwise).

## Evidence

- `mm2_net` 58 tests green: `Input`/`Snap` wire round-trips and
  truncation/oversize rejections, `Start` host-pick legs (both
  `Some`/`None`), mailbox latest-wins under a real socket pump, the
  `MAX_PLAYERS` bound, prune-on-leave, teardown clear, `Snap`
  broadcast reaching every peer, dead-write removal under the shared
  discipline.
- `mm2_app` netdrive unit legs: encode/decode round-trip incl.
  `forced_gear` staying local, out-of-range/NaN clamping, wire
  quaternion sanitization.
- `tests/net_app.rs` 27→29: `a_remote_players_inputs_drive_the_hosted_car`
  — a real loopback peer's `Input` frames spawn a
  `Remote`+`Authority` dynamic car whose `VehicleInput` follows the
  mailbox, staleness zeroes it, `Snap` reaches the peer, departure
  despawns; `a_client_streams_inputs_and_applies_the_host_snapshot` —
  the host seat spawns as a `Remote`+`Predicted` kinematic copy, the
  local input stream lands in the host's mailbox under the session
  generation, a `Snap` retargets the lerp and the `Position` follows,
  stale-tick and foreign-generation frames drop, the own-seat entry
  is skipped.
- `net_host.rs` asserts a dedicated (`mm2-host`) session advertises
  `host_pick: None` — a seat-less host spawns no seat-0 car.

## Gates

- `cargo fmt --all -- --check` — clean.
- `cargo clippy --workspace --all-targets --all-features --
  -D warnings` — clean.
- `cargo test --workspace` — all suites green incl. `net_app` 29/29
  and `mm2_net` 58/58.

## Classification / remaining open items

- Implementation choices: host seat = wire id 0 (never a roster
  entry); remote spawn offsets are designed staging (grid assignment
  is a follow-up); `INPUT_STALE` 250 ms is a designed bound;
  `TCP_NODELAY` on the lobby socket; snapshot pacing rides Update.
- Still open F25-A scope: own-seat prediction/reconciliation,
  damage/stuck/recovery/result replication for remote drivers (their
  `PlayerControl::Remote` skips stay intentional), spawn-grid
  assignment, HUD/roster naming of remote drivers, input
  rate-limiting beyond latest-wins, interpolation tuning, the
  F25-B/C rows (opponent/traffic/object replication).
- Verification gaps carried forward: everything is loopback — AC03's
  impairment matrix and LAN/Internet legs remain F24-C; no
  retail-install leg this iteration (synthetic mounts only); the
  windowed surface is still a status line.

---

# Last iteration — F24-B.8: the in-app host surface — `mm2 --host`
# runs the lobby inside the real application (iteration 021, run
# 20261001T195454-62282)

Implementation iteration on `ralph/night` (baseline `12b42f1` — a
same-run repair commit gating `lobby_input` on `not(capturing)` and
fixing `drive_lobby`'s `pending_exit.take()` ordering, both
non-blocking review observations from the F24-B.7 range; iteration
020's range verified green, review pass with gaps only). One coherent
slice: the host mirror of B.7's client bridge — `mm2 --host` hosts a
lobby in the real application, windowed or `--headless`, and the
operator's `start` mints a generation that begins the session through
the shared `Session` lifecycle.

## Task selection

The F24-B remainder was the named next slice; with B.7's client
bridge landed, the in-app *host* surface was its named open piece —
F24 spec req 2 wants direct host *and* join paths in the application,
and the app could only join. Scope kept explicit: remote players are
roster/display state only — no remote spawning, interpolation, input
transport or world replication (F25/F26); the lobby surface is a
status line, not a menu.

## What landed

- `mm2_app::net` — `HostLink` (resource) owns `mm2_net::Host`:
  `try_recv` is nonblocking so no pump thread; `Host` is `!Sync`
  (its receiver), so it lives behind a `Mutex`. Operator intents ride
  a `HostCommand` channel (`Start`/`Cancel`/`Quit`) — the headless
  stdin loop parses `start`/`cancel`/`quit` (the same contract
  `mm2-host` documents), the windowed `host_input` maps `Enter` →
  start and `Esc` → stop hosting, gated on `not(capturing)` and
  parked-at-`Menu`. `drive_host` drains commands + `HostEvent`s once
  per update into the shared `LobbyState`: remote `Joined`/`Left`/
  `VehicleChanged`/`ReadyChanged` mirror into `roster` (the host seat
  is the local player — never a wire entry), `Started` runs
  `Session::begin_generation` under the host-minted generation with
  `SessionAuthority::Host`, `StartRefused` becomes the gate notice.
  Lifecycle edges: a locally ended hosted session auto-sends `Cancel`;
  `Quit` cancels a running session, drains the leave and owns
  `AppExit` (0 clean, 1 dead host loop); a `Start` queued once
  `leaving` is ignored and a `Started` drained while leaving begins
  nothing — a session nobody is left to cancel must never mint.
  `LobbyState::exit_sent` preserves the consumed exit code so the
  smoke record reports a drained clean shutdown as a pass.
  `net::describe_host_event` is the shared `event=` formatter —
  `mm2-host` and the in-app headless host print identical records.
- `session.rs` — `MenuExit` covers `HostLink`: `drive_session`'s
  `Menu` quit no longer writes `AppExit` while hosting.
- `main.rs` — `mm2 --host` (conflicts `--join`/`--menu`), `--bind`
  (default `127.0.0.1:0`), `--seed`. The session-shaping flags
  configure the *advertised* session, validated at startup through
  the VFS — city psdl resolves, `event_race_setup` builds, the
  `--car`/`--paint` pick passes the scanned catalog, `net::advertise`
  encodes (dev overrides are not network-legal — refused). The app
  prints `listening=<addr> fingerprint=… seed=… session="…"`; the
  windowed surface is a `HostText` status line (bind addr, session
  offer, remote readiness, own pick, gate notice) that clears while a
  session runs.
- `smoke.rs` — `RunSource::Host` + `headless_host` share
  `run_headless`: parked records carry `mp=host(<n>p)`, a running
  session `mp=gen<N>`; verdict — a clean `quit` (including a
  never-started lobby) is a `lobby closed` pass via `pending_exit`/
  `exit_sent`, a dead host loop a named fail, a `Cancel` return to
  the lobby `returned to the lobby` pass. Non-lobby records
  bit-identical.

## Evidence

- `tests/net_app.rs` 15→27 legs, all green (~8 s):
  - In-process hosted bridge vs real loopback `mm2_net` clients:
    remote join/pick/ready/leave mirroring `LobbyState.roster` (host
    seat absent — remote players only); `Started` → `Loading` under
    the minted generation with `Host` authority; `StartRefused`
    naming the unready blocker as the notice; `Cancel` →
    `Unloading → Menu`; a mid-session `Start` parking until teardown
    lands; a locally ended session auto-cancelling on the wire;
    `Quit` cancelling + leaving + `AppExit` 0; a dead host loop →
    `AppExit` 1.
  - `host_input_keys_ride_the_command_channel` — `Enter`/`Esc` ride
    the command channel through the real `ButtonInput` resource,
    `Esc` mid-session ignored, `Esc` at `Menu` exits 0.
  - `a_headless_host_runs_the_advertised_session` — a real
    `headless_host` app with a remote peer: `start` → dev world loads
    through `load_session_world`, records `world=dev-world mp=gen1
    status=pass`.
  - Process legs — `mm2 --host --headless` as a separate OS process
    vs `mm2-join`/`mm2 --join` clients: `start` → `mp=gen1` pass on
    the host and `mp=gen1` on the joiner; unready-peer refusal +
    clean `quit` → `lobby closed` exit 0; `--host` flag gates
    (`--bind`/`--seed` required-forms, `--join`/`--menu` conflicts)
    → exit 2.

## Gates

- `cargo fmt --all -- --check` — clean.
- `cargo clippy --workspace --all-targets --all-features --
  -D warnings` — clean.
- `cargo test --workspace` — all suites green incl. `net_app` 27/27.

## Classification / remaining open items

- Implementation choices: the app-layer host seat is never a wire
  entry (remote players only in `roster`); `Enter`/`Esc` and
  `start`/`cancel`/`quit` are the designed operator surface (MP-8
  documents host control; the original's refusal conditions are
  unrecovered — the start gate's designed policy stands).
- Still open F24-B scope: a real lobby menu (both sides are status
  lines), disconnect-UX polish beyond the notice, and `Start` →
  spawn-roster consumption — the wired session spawns only the local
  player; remote roster entries are not spawned (F25/F26).
- Verification gaps carried forward: everything is loopback — AC03's
  impairment matrix and AC06's LAN/Internet legs remain F24-C; the
  hosted session runs the host seat only — no position/score/damage
  replication exists to observe; no retail-install leg was run for
  `--host` (B.6 recorded the retail `mm2-host`/`mm2-join` leg — the
  VFS/validation paths are shared).

---

# Last iteration — F24-B.7 repair: generation-ceiling saturation +
# the networked restart gate (iteration 020, run 20261001T195454)

Repair iteration on `ralph/night` (baseline `6298749` — the F24-B.7
candidate; external verify green, review **fail** on two blocking
findings, both inside the candidate's own contracts).

## Task selection

Iteration 019's review named two blockers; the selection policy puts
repair ahead of any new feature work. No unrelated work was taken.

## What landed

- `mm2_game::Session::{begin, begin_generation}` — both generation
  bumps are `saturating_add(1)`. `Message::Start.generation` decodes
  as an unbounded `u64`: a `Start{generation: u64::MAX}` was adopted
  verbatim, after which the next bump computed `u64::MAX + 1` — a
  remote-triggerable panic under dev `overflow-checks`, and in release
  a wrap to 0 that `max(wire, 0)` silently regressed, breaking the
  never-regress invariant `ObjectId`/`ResultId` staleness detection
  keys on. The host-side mint in `mm2_net::lobby` (`generation += 1`)
  saturates too — same contract.
- `mm2_app::session::drive_session` — the `Menu` restart arm gates
  `Session::begin` on `config.authority == SessionAuthority::Local`,
  the predicate the `F4` binding already applies at intent time. The
  F4 gate left the intent's other producers unhandled: the results
  screen's Restart row (`results_input` — reachable under `Remote`),
  the Blitz/Checkpoint `RestartEvent` disabled outcome (inert under
  `Remote` — `resolve_disabled` is authority-gated — but live under
  `Host`), and `--restart`. Each could mint a local generation for a
  session the lobby owns. A non-`Local` restart intent is now consumed
  at `Menu` without a `begin` — teardown has already returned the
  client to the lobby's waiting state, where the wire's next
  `Cancel`/`Start` (or a parked `pending_start`) owns what happens
  next. Pause-menu Restart was already unreachable under non-`Local`
  (MP-6 forbids the pause).

## Evidence

- `mm2_game` `tests/session.rs` —
  `a_u64_max_generation_never_overflows_or_regresses`: wire `u64::MAX`
  adopted, the next local `begin` saturates at the ceiling, and a
  repeated `begin_generation(u64::MAX)` stays pinned — no panic, no
  wrap, no regression.
- `mm2_app` `tests/session.rs` —
  `a_networked_restart_returns_to_menu_without_a_local_begin`: under
  both `Remote` and `Host`, the real producer path (`Results` → focus
  "Restart race" → `Enter` through `results_input`) queues the intent,
  the session tears down to `Menu` through the production
  `Unloading` arm, and no `begin` follows — generation unchanged,
  authority retained, intent consumed, no `AppExit`.

## Gates

- `cargo fmt --all -- --check` — clean (after one `cargo fmt` pass).
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — clean.
- `cargo test --locked --workspace` — all suites green: mm2_game
  session 14/14, mm2_app session 26/26 (incl. the new leg), net_app
  15/15, net_host 4/4, net_join 4/4, mm2_net 53/53.

## Classification / remaining open items

- Implementation choice: a networked-authority restart intent still
  tears the predicted session down to `Menu` (the lobby's waiting
  state) — it is the "leave the session" leg, not a hidden begin. A
  hostile-generation host is not otherwise defended: saturation pins
  the counter at the ceiling rather than rejecting the peer.
- All of iteration 019's open items stand unchanged: the lobby menu
  surface, disconnect-UX polish, the in-app host surface, `Start` →
  spawn-roster consumption (F25/F26), the AC03 impairment matrix and
  AC06 LAN/Internet legs (F24-C), and replication generally.

---

# Last iteration — F24-B.7: the Bevy-side lobby client — `mm2 --join`
# consumes `Start` into the shared session lifecycle (iteration 019,
# run 20260929T174954)

Implementation iteration on `ralph/night` (baseline `42f1196` — the
F24-B.6 candidate; external verify green, review **pass** with
verification gaps only, no blocking findings). One coherent slice: the
in-app client bridge — `mm2 --join <addr>` joins a lobby in the real
application (windowed or `--headless`) and a host `Start` begins a
session through the production `Session` lifecycle and
`load_session_world`. This is the AC05 leg B.6's `ClientCtl` was
built for.

## Task selection

The F24-B remainder was the named next slice, and the bridge was its
load-bearing piece: `mm2-join` proved the protocol client-side but
nothing fed `Start` into the app — the joined process could not run a
session. Scope kept explicit: remote spawning, roster-pick gameplay
consumption and any replication stay F25/F26; the lobby surface is a
status line, not a menu; an in-app *host* surface remains future work.

## What landed

- `mm2_game::Session::begin_generation(config, generation)` — a
  `Menu → Loading` begin under the host-minted lobby generation, so
  `ObjectId`/`ResultId` generation fields agree across peers. Clamped
  `max(wire, local + 1)` — generation-keyed staleness detection
  assumes the counter never regresses.
- `mm2_app::net` — `LobbyLink` (resource): `Client::recv` blocks, so
  it lives on a pump thread forwarding each frame as a `LobbyEvent`
  (terminal `Closed`); outbound intents ride `ClientCtl`; `Drop` sends
  `Leave`. `drive_lobby` drains the channel once per update into
  `LobbyState` (roster / advertised session / running generation /
  notice / `pending_start` / `pending_exit`) — display and decision
  state only. `Start` runs the same gate `mm2-join` runs (`accept` +
  `check_session`), stamps `Remote` authority + the roster-echoed
  pick + this process's local `mods_active`/`dev` (none of which ride
  the wire), and calls `begin_generation`; a `Start` mid-session parks
  to `pending_start` until teardown returns `Menu`; a matching
  `Cancel` quits through `Unloading → Menu`. The lobby owns exit while
  a link exists — `drive_session`'s `Menu` quit no longer writes
  `AppExit`; `drive_lobby` writes it (0 after our acknowledged leave —
  a 5 s watchdog covers a host that never closes; 1 on lost host or
  refused session).
- `session.rs` — `MenuExit` system-param (menu shell / lobby link
  ownership of the `Menu` quit), and the stock `F4` restart binding
  gated to `Local` authority — a remote session's restarts are the
  host's `Cancel`/`Start`.
- `progression.rs` — `Ineligible::Networked`: a networked-authority
  session is record-ineligible (designed conservative policy — a
  local prediction must not mint single-player unlocks; the original's
  MP→SP progression relation is unverified).
- `smoke.rs` — `headless_lobby(link, …)` shares `run_headless` with
  `headless_smoke` via `RunSource::{Session, Lobby}`: a lobby run
  parks at `Menu`, paces parked updates in wall-clock (4 ms) so the
  frame budget waits on the wire, ends the wait on a dead/finished
  link, and reports `mp=gen<N>` / `mp=lobby(<n>p)` plus a lobby-aware
  verdict (`returned to the lobby` pass on `Cancel`, `lost the host` /
  `host never started` fails). Non-lobby records are bit-identical.
- `main.rs` — `mm2 --join <addr>` (conflicts every session-shaping
  flag — the wire owns world/mode/difficulty/conditions/seed),
  `--driver` (default: bound profile name, else `player`,
  `MAX_STRING`-bounded), `--ready`; the resolved `--car`/`--paint`
  pick is offered to the lobby at join. The windowed surface is a
  `LobbyText` status line (session offer, roster readiness, own pick,
  latest notice) with `Enter` = ready toggle and `Esc` = leave.

## Evidence

- `tests/net_app.rs` — 15 legs, all green (~9 s):
  - In-process `drive_lobby`/`drive_session` vs a real loopback
    `Host`: join surfacing ad+roster at `Menu`; `Start` →
    `Loading` under the minted generation with `Remote` authority and
    the wired world/mode; `Cancel` → `Unloading → Menu` with no
    `AppExit`; a `Start` mid-session parking until teardown lands;
    a dead host tearing down to `AppExit` 1 with a `lost the host`
    notice; `Leave` → host `Quit` → exit 0; a `Menu` quit staying in
    the lobby; an unrunnable ad refused with a clean leave + exit 1;
    `F4` restart gated to `Local`.
  - `a_lobby_start_loads_the_wired_world_headless` — a real
    `headless_lobby` app joins an in-process host; `start` → dev
    world loads through `load_session_world`, drives, records
    `world=dev-world mp=gen1 status=pass`.
  - Process legs — `mm2 --join --headless` as a separate OS process
    against a separate `mm2-host`: `start` → `mp=gen1 status=pass`;
    `cancel` → `mp=lobby(1p) phase=menu`; host quit → `lost the
    host`, exit 3; refused connect → exit 1; `--join` ×
    session-shaping flags → exit 2.
- `mm2_game` legs: `begin_generation` adoption + no-regression clamp;
  `record_eligibility` refuses `Remote`/`Host` authority.

## Gates

- `cargo fmt --all -- --check` — see below.
- `cargo clippy --workspace --all-targets --all-features --
  -D warnings` — see below.
- `cargo test --workspace` — see below. `net_app` 15/15 green.

## Classification / remaining open items

- Still open F24-B scope: a real lobby menu (the surface is a status
  line), disconnect-UX polish beyond the notice, the in-app host
  surface, and `Start` → spawn-roster consumption (the wired session
  spawns only the local player — remote roster entries are not
  spawned; F25/F26).
- Verification gaps carried forward: everything is loopback —
  AC03's impairment matrix and AC06's LAN/Internet legs remain F24-C;
  the remote session is a *local prediction* — no position/score/
  damage replication exists to observe.

---

# Last iteration — F24-B.6: `mm2-join`, the headless lobby client +
# client-side session-content gating (iteration 018, run
# 20260929T174954)

Implementation iteration on `ralph/night` (baseline `a682db9` — the
F24-B.5 candidate; external verify green, review **pass** with
verification gaps only, no blocking findings). One coherent slice: the
client side of the headless lobby pair — a real `mm2-join` process as
the counterpart of `mm2-host`, consuming the deferred client-side
session-content gate.

## Task selection

No failing gate or review finding to repair — iteration 017's review
passed with gaps only. Its recorded gap was the load-bearing one: "a
hostile host blob's Event params are bounded in size but not
content-validated by the joiner," and clients had *no* shipped process
at all — `mm2_net::Client` existed only inside tests, so AC01's
"separate client processes" leg had no consumer. The gate needed a
real client to live in; `mm2-join` is the smallest concrete one and
the shape the Bevy bridge's control thread will reuse.

## What landed

- `mm2_net::lobby` — `Client::ctl()` hands out a cloneable
  `ClientCtl` (`set_ready`/`set_vehicle`/`leave`), mirroring `HostCtl`:
  a thread blocked in `Client::recv` can still send. Senders serialize
  on a shared `Mutex<Writer>` — a frame is two `write_all` calls, so
  unsynchronized clones would interleave them. `MAX_STRING` is
  re-exported from the crate root (the validator bound was always
  public policy).
- `mm2_app::net::check_session(vfs, &SessionConfig)` +
  `SessionContentError::{World, Event, Laps, Opponents}` — the join-side
  content gate: the advertised `City` psdl must resolve on *this*
  mount; an `Event` must survive `race::event_race_setup` (the same
  resolve+build the host's flag-time gate runs); a `race`
  customization is bounded where it applies — `laps ≤
  CUSTOMIZE_LAP_MAX` on `Ordered` rules only, `opponents ≤` the authored
  aimap roster. Deliberately skipped: anything `accept` + `validate`
  already bounds, and picks the runtime ignores.
- `mm2-join` — new `mm2_app` binary. `--mm2-path` (read-only mount,
  same policy as `mm2-host`), `--mods`, `--connect`, `--driver`,
  `--vehicle <id>[:<paint>]`, `--ready`. Prints `connected=`/`session=`/
  `event=` records (`roster`/`pick_refused`/`started`/`cancelled`/
  `join_failed`/`session_refused`/`closed`); stdin drives
  `vehicle`/`ready`/`unready`/`quit`. Every advertised session — the
  lobby ad and the running one inside `Start` — is checked by
  `net::accept` + `check_session`; a refusal is `session_refused`, a
  clean `Leave`, exit 1. Exit codes mirror `mm2-host`: 0 `quit`, 1
  refused/lost, 2 usage. A started session is reported, never spawned
  — F25/F26 scope.
- `tests/support/mod.rs` — the spawned-proc line driver and the
  `testcity` fixture hoisted out of `net_host.rs` (fixture gained a
  buildable `circuit:1` row for the customization legs).

## Evidence

- `mm2_net` 51→53: `ClientCtl` drives a cross-thread pick/ready/leave
  against a real lobby.
- `tests/net_join.rs` (8 legs, ~2 s):
  - `separate_client_processes_pick_ready_and_start` — two `mm2-join`
    OS processes + one `mm2-host` process: pick, ready, roster
    reaching 2/2, `started generation=1`, `quit` exit 0 each and
    `cause=quit` on the host record. First separate-*client*-process
    AC01 evidence — loopback still.
  - `a_client_process_reports_a_refused_join` — mismatched mounts fail
    the fingerprint handshake: `join_failed` exit 1 client-side, the
    refusal logged host-side (AC02/AC04 surface).
  - `a_client_process_refuses_a_session_it_cannot_run` — an in-process
    `Host` on identical (empty) mounts advertises a dev-world event
    and an unresolvable city; the client reports `session_refused`,
    exits 1, and the host sees a `Quit` leave, not a dropped socket.
  - `a_client_process_accepts_a_runnable_event` — real `mm2-host
    --event race:0` process + `mm2-join`: `session="testcity, race:0,
    amateur"`, `started generation=1`, clean quit.
  - Direct `check_session` matrix: runnable dev/cruise/event sessions
    pass; missing world / out-of-table / absent-table /
    resolve-but-unbuildable events refuse `World`/`Event`; `laps` 11
    and `opponents` 1 refuse on `circuit:1` (`Ordered`, empty aimap
    roster) while the same picks pass where the runtime ignores them
    (Checkpoint `laps`, Cruise `race`).
  - `check_accepts_a_retail_session` — env-gated (`MM2_RETAIL`) leg
    over the real install.
- Retail leg (original-content evidence): `mm2-host --mm2-path
  /Users/linus/coding/rust-mm2/retail --city sf --event race:0` +
  `mm2-join --vehicle vpbug --ready` — stock pick validated by the
  host's catalog gate, roster 1/1, `started generation=1
  session="sf, race:0, amateur"` observed client-side, `quit` →
  `cause=quit`, both processes exit 0.

## Gates

- `cargo fmt --all -- --check` — clean (after one `cargo fmt` pass).
- `cargo clippy --workspace --all-targets --all-features --
  -D warnings` — clean.
- `cargo test --workspace` — all suites green incl. `net_host` 4/4 and
  `net_join` 8/8 (exact totals in the run log).

## Classification / remaining open items

- Implementation choice recorded in `docs/research/net.md`: the
  client-side gate duplicates the *check*, not trust — the fingerprint
  handshake already means an honest host shares the content; the gate
  is defense-in-depth for a blob that disagrees with it.
- Still open F24-B scope: the Bevy-side bridge (AC05's in-app leg —
  `ClientCtl` is the piece it was waiting on), `Start` → spawn-roster
  consumption (F25/F26), AC04's richer disconnect UX.
- Verification gaps carried forward: everything is loopback — the
  AC06 LAN/Internet matrix and AC03's impairment legs remain F24-C; a
  started session is a record line, not a simulation — no position/
  score/damage replication exists to observe.

---

# Last iteration — F24-B.5: event-mode hosting, the `LateJoin::Closed`
# consumer (iteration 017, run 20260929T174954)

Implementation iteration on `ralph/night` (baseline `29cc266` — the
F24-B.4 candidate; external verify green, review **pass** with
verification gaps only, no blocking findings). One coherent slice: the
plan's named "event-mode hosting + a `LateJoin::Closed` consumer" leg —
`mm2-host` can now host an authored event lobby, which makes MP-5's
race rule real end-to-end instead of wire-only.

## Task selection

No failing gate or review finding to repair — iteration 016's review
passed with gaps only. The plan's F24-B remainder named three legs:
client-side session-content join gating (blocked on the in-app bridge
having somewhere to report), the Bevy-side bridge itself (too large
for one slice on its own), and event-mode hosting. I took event-mode
hosting: the smallest remaining piece with a real shipped consumer,
and the only consumer `LateJoin::Closed` has.

## What landed

- `mm2_game::config` — `EventRef::parse(arg, city)` +
  `EventTableKind::parse_token`: the `<table>:<row>` selector grammar
  (`checkpoint`/`race`, `blitz`, `circuit`, `crash`/`crashcourse`),
  hoisted out of `main.rs`'s `parse_event_ref`; `mm2 --event`,
  `mm2-host --event` and `mm2-inspect`'s `parse_table_filter` now share
  the one implementation (three near-copies collapsed; mm2-inspect
  keeps its own error wording).
- `mm2-host` — `--event <table>:<row>` hosts an authored event instead
  of cruise. `--city` names both the world and the event's city
  (default `london`), mirroring `mm2`; `--dev-world` + `--event` stays
  legal as a dev rig. The gate is the same path a session load takes:
  `race::event_race_setup` (catalog scan → dependency-checked
  `EventCatalog::resolve` → `race_definition` build, plus the authored
  roster/reward surface) — an unknown row, missing records or an
  unbuildable definition exit 2 at flag time rather than failing every
  client at start. Crash Course rows refuse through
  `RaceBuildError::CrashCourseUnsupported`, same as `mm2` (F21).
  `start`'s late-join policy is the session mode's (MP-5,
  documented): `Event` → `LateJoin::Closed`, `Cruise` → `Open` — the
  first shipped `Closed` consumer.
- No wire change — `PROTOCOL_VERSION` stays 3; `Start`/`Cancel`/
  `SessionStarted` already carry everything this leg needs.

## Evidence

- `mm2_game` +2: `EventRef::parse` grammar round-trip (aliases,
  lowercase normalization) and malformed-selector rejections
  (`circuit:-1`, `circuit:0:extra`, index overflow, …).
- `tests/net_host.rs` +2 against the separate `mm2-host` process:
  - `an_event_host_closes_joins_at_start` — a synthetic `testcity`
    install (checkpoint row + `race0.aimap`/`race0waypoints.csv` +
    `city/testcity.psdl` stub) hosts `--event race:0`; the advertised
    `Session` decodes back to `SessionMode::Event{testcity, Checkpoint,
    0}` on `city/testcity.psdl`; `start` mints generation 1, then a
    late join is refused `Rejected{SessionStarted}` with
    `event=join_failed … "the session has already started"` on the
    record; `cancel` re-opens the lobby (a post-cancel join gets
    Session+Roster and a quiet socket — no trailing `Start`), `quit`
    exits 0.
  - `an_unrunnable_event_is_refused_at_flag_time` — `--event bogus`
    (grammar), `--event race:9` (row beyond the table — resolve fails)
    and `--event circuit:0` (resolve-Ready but `NumLaps 0` cannot build
    an Ordered definition) each exit 2 with a named stderr error; the
    last leg proves the gate runs the real build, not just resolve.
- Retail leg (original-content evidence): `mm2-host --mm2-path
  /Users/linus/coding/rust-mm2/retail --city sf --event race:0`
  resolves+builds and serves `session="sf, race:0, amateur"`;
  `--event crash:0` exits 2 on `CrashCourseUnsupported` and
  `--event race:99` exits 2 on the missing row.

## Gates

- `cargo fmt --all -- --check` — clean.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — clean.
- `cargo test --locked -p mm2_game` — all suites green (lib 89/89).
- `cargo test --locked -p mm2_app --lib` — 62/62 (`net::` 10/10).
- `cargo test --locked -p mm2_inspect` — 42/42 (incl. the delegated
  `parses_event_specs` legs).
- `cargo test --locked -p mm2_app --test net_host` — 4/4 in ~2 s with
  `mm2-host` as a separate OS process. Note: the intermittent
  `_dyld_start` security-evaluation stall on this machine made several
  earlier runs fail with the child never exec'd inside the 15 s wait —
  environment flakiness first recorded in iter 012, identical to iter
  014's note; the suite passes cleanly once a fresh binary has been
  exec'd once. Full-workspace `cargo test --locked --workspace` is left
  to the external `verify.sh` pass — mm2_net is untouched by this diff.

## Classification / remaining open items

- Original requirement honoured: MP-5's race-side late-join rule now
  has a real consumer (event lobbies close at start); cruise stays
  open. `mm2 --event`'s flag-time refusal semantics are matched
  (resolve-or-fail, never advertise the unrunnable).
- Designed and recorded in `docs/research/net.md`: advertising is
  gated by the shared loader build (`event_race_setup`) — a lobby-level
  check cannot prove each client's render path; the mode→`LateJoin`
  mapping (any authored event = race rule, incl. Crash Course rows,
  which refuse earlier at the build anyway).
- Still open F24-B scope: client-side session-content join gating (a
  peer validating an advertised session against its *own* content —
  `EventRef.index`, customized `laps`/`opponents` range binding — lands
  with the in-app bridge that has somewhere to report it), the
  Bevy-side bridge (AC05's in-app leg), `Start` → spawn-roster
  consumption (picks are still unused at session build — F25/F26
  scope), AC04's consumer-facing error surface. The advertised-event
  wire path now exists, but no client builds the session from `Start`
  yet — `Start` remains a signal only.
- Verification gaps carried forward: clients still share the test
  process (full AC01 topology + AC06 LAN/Internet matrix are F24-C);
  the wire-level event legs run over a synthetic `testcity` fixture —
  the retail leg above proves the flag-time gate on real content but
  no client ever joined a retail-hosted event lobby; `SetReady`/
  `SetVehicle` rate-limiting and unbounded channels remain
  F24-C/AC03 scope as previously disclosed.

---

# Last iteration — F24-B.4: session start/cancel, generation, and the
# MP-5 late-join policy (iteration 016, run 20260929T174954)

Implementation iteration on `ralph/night` (baseline `ae01795` — the
iteration-015 repair; external verify green, review **pass** with
verification gaps only, no blocking findings). One coherent slice: the
plan's named fourth leg of F24-B — start/cancel — which also lands the
spec's session-generation requirement and MP-5's documented late-join
split.

## Task selection

No failing gate or review finding to repair — iteration 015's review
passed with gaps only. The plan's named next slice was F24-B's fourth
leg, start/cancel, which the plan itself flagged as owning "un-picked
players and start gating". Reading `docs/original-rules.md` first
mattered: MP-5 (documented, `help:Multiplayer Games`) splits the
original's late-join behavior by mode — races close at start, Cruise
and C&R stay open — so a blanket "no joins once started" wire policy
would have contradicted a documented original rule. The policy is
instead the consumer's per-start choice (`LateJoin::Closed`/`Open`),
which is also exactly what a future event-mode host needs.

## What landed

- `mm2_net::proto` — `PROTOCOL_VERSION` 2 → 3. `Message::Start {
  generation: u64, session: SessionAdvertisement }` (self-contained —
  the *running* session, snapshotted at start, so a mid-session
  `set_session` re-advertisement can't rewrite what is running) and
  `Message::Cancel { generation }`; `RejectCode::SessionStarted`.
  The session codec is shared between `Session` and `Start`.
- `mm2_net::lobby` — a `Phase` (`Lobby` / `InSession { generation,
  session, late_join }`) in the host loop. `Host::start(late_join)`
  requests a start; the gate (designed — MP-8 documents the host's
  Start control, not its refusal conditions) requires lobby phase, an
  advertised session, and every connected player ready *and* picked —
  the first blocker names the refusal reason, and an empty wire roster
  passes (the app-layer host player is not on it). `Started` reports
  the roster that survived the `Start` send. `Host::cancel()` returns
  to lobby: `Cancel` broadcast, readiness reset (picks kept), joins
  re-open — a no-op outside a session. `Host::ctl()` hands out
  `HostCtl`, the cloneable cross-thread driver (`Host` is `!Sync`).
  Late joins follow MP-5: `Closed` rejects `SessionStarted` before
  `Accept`; `Open` admits and then unicasts the running `Start`.
  The roster stays live mid-session — departures and pick changes
  still apply and rebroadcast (MP-5's leaver rule). Client-sent
  `Start`/`Cancel` drop `Malformed` like every host-only message.
- `mm2-host` — stdin is the operator control surface: `start` (always
  `LateJoin::Open`, since it advertises cruise only), `cancel`,
  `quit` (clean exit 0); a closed stdin is normal unattended
  operation. The record contract gains `event=started generation=`,
  `event=start_refused reason=`, `event=cancelled generation=`.

## Evidence

- `mm2_net` 40 → 51, real loopback sockets: `Start` reaches every peer
  with generation 1 and the self-contained session; the gate names
  each blocker (unready, unpicked, no session, already running) and a
  refused start changes nothing; an empty roster may start; a `Closed`
  join is refused `SessionStarted` with a `JoinFailed` record; an
  `Open` joiner gets Session → roster → the *running* `Start` even
  after a mid-session re-advertisement; `Cancel` broadcasts, resets
  readiness, keeps picks, re-opens joins, and the next start mints
  generation 2; a mid-session pick and a quit still update the roster;
  client-sent `Start`/`Cancel` drop the peer `Malformed`; `HostCtl`
  drives a start from another thread.
- `tests/net_host.rs` +1 leg against the separate `mm2-host` process:
  stdin `start` refused before ready (`event=start_refused`), then
  `event=started generation=1` with the client decoding the session
  via `net::accept`, a mid-session joiner receiving the running
  `Start`, `cancel` → reset roster → generation-2 restart, and `quit`
  exiting the process cleanly.

## Gates

- `cargo fmt --all -- --check` — clean.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — clean.
- `cargo test -p mm2_net` — 51/51. `cargo test -p mm2_app --test
  net_host` — 2/2 in ~2 s with `mm2-host` as a separate OS process.
  The full workspace run is left to the external `verify.sh` pass;
  mm2_net's only consumer is mm2_app, covered here.

## Classification / remaining open items

- Original requirement honoured: MP-5's mode-dependent late-join split
  (Open/Closed) and the leaver-visible roster; MP-8's host-owned start
  control. Designed and recorded in `docs/research/net.md`: the start
  gate's exact refusal conditions, readiness-reset-on-cancel,
  generation semantics (namespaces `mm2_game`'s `ObjectId`), the empty
  roster pass, `Start`'s self-contained session payload, and
  mm2-host's stdin surface.
- Still open F24-B scope: session-content join gating (a peer admitted
  to a session it cannot run — e.g. missing event content — beyond the
  fingerprint gate), the Bevy-side in-app host/client bridge (AC05's
  other leg), event-mode hosting (mm2-host is cruise-only, so the
  `Closed` policy has only wire-level legs — no real consumer yet),
  AC04's consumer-facing error surface. Picks are still not consumed
  by any session-build path — `Start` signals; the spawn roster is a
  later leg — and the latent decode range gaps (`EventRef.index`,
  `RaceCustomization.opponents`, laps) stay deferred to it.
- Verification gaps carried forward: clients still share the test
  process (full AC01 topology + AC06 LAN/Internet matrix are F24-C);
  no retail-install `mm2-host` run (real catalog picks) is on record;
  `SetReady`/`SetVehicle` rate-limiting and unbounded channels remain
  F24-C/AC03 scope as previously disclosed.

---

# Last iteration — external-review repair: an over-long `VehicleRefused`
# reason could drop the live picker (iteration 015, run
# 20260929T174954)

Repair iteration on `ralph/night` (baseline `0aa82ac` — the F24-B.3
candidate; external verify green, review **fail** on one blocking
finding).

## Root cause

An implementation defect in the F24-B.3 refusal path. The
`PickValidator` contract is `Result<(), String>` — the reason is
consumer text — but it rode `Message::VehicleRefused.reason`, a
`MAX_STRING` (256-byte) wire field, with no bound. The shipped
validator (`mm2_app::net::vehicle_validator`) echoes the
wire-controlled id: `SetVehicle` with a `vehicle` string of ~236+
bytes — wire-legal, reachable through `Client::set_vehicle` — produced
a 257+ byte `unknown vehicle id {vehicle:?}` reason. `Writer::send`
encodes before any I/O, so the send failed `OversizeString`, and the
lobby's `.is_err() && remove_player(...)` arm could not tell an encode
failure from a dead socket — the healthy peer was reaped
`LeaveCause::Lost` even though `HostEvent::VehicleRefused` had already
reported a delivered refusal. That inverted the leg's invariant: a bad
pick became a drop, not a refused request. The validator's pre-computed
`Incomplete` reasons (`vehicle {id} is incomplete: missing {…}`) were
the same overflow class with no wire input at all — a long mod-catalog
id or missing-deps list over the bound.

## Repair

`mm2_net::lobby` now bounds the reason before it reaches the wire:
`bound_reason` shortens an over-long `Err` string on a char boundary to
`MAX_STRING` (marking the cut with `...`) ahead of both the
`HostEvent::VehicleRefused` emission and the `VehicleRefused` send, so
the encode cannot fail on field length and a failed send once again
means a transport error — the dead-write removal arm is then
unambiguous. Bounding lives in `mm2_net` because `MAX_STRING` is the
wire's bound and the validator is consumer-supplied: any consumer (not
just `mm2_app`'s catalog validator) gets the guarantee. The
`PickValidator` doc now states the reason is shortened to fit the wire
field; `docs/research/net.md` records it.

## Regression leg (mm2_net 39 → 40, real loopback sockets)

- `an_overlong_refusal_reason_still_reaches_the_peer` — an id-echoing
  validator plus a `MAX_STRING`-byte `SetVehicle.vehicle`: the peer
  receives `VehicleRefused` with a `<= MAX_STRING` reason carrying the
  truncation mark, the host event reports the bounded reason against
  the full wire id, the peer then picks legally (`VehicleChanged`) and
  no `Left` was emitted — previously the same pick silently
  disconnected it.

## Gates

- `cargo fmt --all -- --check` — clean.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — clean.
- `cargo test -p mm2_net` — 40/40.
- `cargo test -p mm2_app` — all suites green: lib 62/62 (`net::` 10/10),
  `tests/net_host.rs` 1/1 (~2 s, separate `mm2-host` process). `mm2_net`
  is only consumed by `mm2_app`, so the diff's blast radius is covered;
  the external `verify.sh` run owns the full-workspace pass.

## Notes

- The review's verification gaps stand as disclosed: `mm2-host`
  exercised only with an empty install + `--dev-world` (no
  retail-install catalog picks), clients still share the test process
  (full AC01 topology and the AC06 LAN/Internet matrix are F24-C),
  distinct-pick rate-limiting and unbounded channels are F24-C/AC03
  scope, and the latent wire-decode range gaps (`EventRef.index`,
  `RaceCustomization.opponents`, laps) must be re-bound in the
  session-start leg.
- Defect class worth noting: any host-constructed wire field populated
  from consumer or peer-derived strings needs the bound enforced before
  the send, or encode errors must be distinguished from transport
  errors at the send site. `Session.summary`/`params` were already
  pre-encoded in `Host::set_session`; `Reject.message` inputs are
  bounded by construction; the refusal reason was the remaining gap.

---

# Last iteration — F24-B.3: vehicle/paint picks on the roster, gated by
# a consumer-supplied catalog validator (iteration 014, run
# 20260929T174954)

Implementation iteration on `ralph/night` (baseline `8aceb4c` — the
iteration-013 docs repair; external verify green, review **pass** with
verification gaps only, no blocking findings). One coherent slice: the
plan's named third leg of F24-B — vehicle/paint pick + validation.

## Task selection

No failing gate or review finding to repair — iteration 013's review
passed with gaps only (single-process clients, unbounded channels, no
`SetReady` rate limit — all disclosed F24-C scope). The plan offered
"vehicle/paint pick + validation or start/cancel"; I took the pick leg:
it lands the roster field the F24-A remainder anticipates and the
AC01 "choose compatible cars" surface, while start/cancel stays a
separate leg because it needs a session-lifecycle design (who triggers
start on a dedicated host, session generation, late-join policy).

## What landed

- `mm2_net::proto` — `PROTOCOL_VERSION` 1 → 2 (the roster entry's shape
  changed). `VehiclePick{vehicle, paint}` rides `RosterEntry::pick`
  (`Option`, `None` until picked); `SetVehicle` is the client→host pick
  request; `VehicleRefused{reason}` is host→*that peer alone*.
- `mm2_net::lobby` — `HostConfig::pick_validator: Option<PickValidator>`
  (`Arc<dyn Fn(&str, u8) -> Result<(), String>>`): the wire crate stays
  project-free, so the consumer supplies the legality check and the
  host loop applies it on the authoritative side. A legal pick lands on
  the slot and rebroadcasts the roster (`HostEvent::VehicleChanged`);
  a refused one sends `VehicleRefused` to the picker only, emits
  `HostEvent::VehicleRefused`, and leaves the roster unchanged — a bad
  pick is a refused request, not a drop. A pick identical to the
  current one is a no-op (no event, no broadcast) so a repeating client
  cannot flood the lobby. A dead `VehicleRefused` write reaps the peer
  via the shared `remove_player` disconnect discipline (factored out of
  `broadcast`/`PeerGone`). `Client::set_vehicle`; the reader thread
  forwards `SetVehicle` alongside `SetReady`. No validator = any
  bounded pick accepted (a content-free transport host).
- `mm2_app::net` — `vehicle_validator(&VehicleCatalog)`: designed
  policy — the empty wire id is the synthetic dev car (always legal,
  paint 0 only); a catalog pick names an exact lowercase `Ready` entry
  (display-name aliases are menu conveniences, not wire identity);
  `paint` is bounded by the entry's metadata `Colors` list with a
  one-job floor — the same bound the garage menu presents, while
  `load_vehicle`'s `paint_jobs` check stays authoritative at spawn.
  `encode_pick`/`decode_pick` map `VehicleSelection` ↔ `VehiclePick`
  (`id None` ↔ `""`; `paint > 255` refuses as `SessionWireError::Paint`,
  never clamps).
- `mm2-host` — scans the `VehicleCatalog` at startup and installs the
  validator; the record contract gains `event=vehicle id=… vehicle="…"
  paint=…` and `event=pick_refused id=… vehicle="…" paint=… reason="…"`.

## Evidence

- `mm2_net` 34 → 39, real loopback sockets: a pick lands on the roster
  for the picker, incumbents and a post-pick newcomer; a refused pick
  reaches the picker alone as `VehicleRefused` with the roster
  untouched and the client still alive; an identical re-pick produces
  neither event nor broadcast (deterministic leg: dup-pick then
  `SetReady` yields exactly the ready roster); no-validator hosts
  accept any bounded pick; a client sending `VehicleRefused` is a
  host-only-message violation dropped `Malformed`.
- `mm2_app::net` 7 → 10: pick codec round-trips (catalog pick, dev car,
  `paint 300` refused), the catalog validator policy (dev car paint
  bound, exact-id rule, `Ready` gating with the missing-deps reason,
  `Colors`-list paint bound + no-`Colors` floor, non-canonical
  spellings refused), and the empty-catalog dev-car-only shape.
- `tests/net_host.rs` — against the separate `mm2-host` process:
  alice's dev-car pick (`""`) lands (`event=vehicle`) and bob sees it
  on the roster snapshot; bob's `vpbug` pick on the empty-install host
  is refused (`event=pick_refused`, `VehicleRefused` to bob alone).

## Gates

- `cargo fmt --all -- --check` — clean (after one rustfmt reflow).
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — clean.
- `cargo test --locked --workspace` — green; mm2_net 39/39, mm2_app
  `net::` 10/10, `net_host` 1/1. The run was slow (~40 min) because
  every test-binary exec pays the intermittent `_dyld_start`
  security-evaluation stall on this machine (first noted in iter 012);
  two earlier `net_host` runs failed that way — the mm2-host child
  never exec'd inside the 15 s wait — then passed in 1.85 s once the
  stall cleared. Environment flakiness, not a code defect.

## Classification / remaining open items

- Implementation choice throughout (wire fields, validator boundary,
  no-op dedupe, paint bounds). The designed dev-car token (`""`)
  and the `Colors`-list paint bound are recorded in
  `docs/research/net.md`.
- Still open F24-B scope: start/cancel (un-picked players and start
  gating belong there — `pick: None` is a legal lobby state by design),
  session-content join gating, late-join, the Bevy-side bridge, AC04's
  consumer-facing error surface. The `SetReady`/`SetVehicle` rate gap
  stands as disclosed (identical-pick dedupe covers only the cheapest
  repeat). AC01's full topology (separate client processes), AC06's
  LAN/Internet matrix: F24-C.
- Verification gap carried forward: `mm2-host` exercised only with an
  empty install + `--dev-world` (dev-car-only validator leg); no
  retail-install run with real catalog picks is on record yet.

---

# Last iteration — external-review repair: the F24-B.2 ledger recorded
# an `mm2-host` CLI and a `Host::recv` signature that never shipped
# (iteration 013)

Repair iteration on `ralph/night` (baseline `920fdd8` — the F24-B.2
candidate; external verify green, review **fail** on one blocking
finding). Docs-only repair: no code changed.

## Root cause

The iteration-012 review found the three committed records of the
slice — `docs/research/net.md`, PLAN.md's F24-B.2 row and the
iteration-012 entry below — described an `mm2-host` interface the
binary does not implement: flags `--mode/--difficulty/--vehicle/
--paint/--name` and `--mod` (actual: none of those exist; the session
flags are `--dev-world`/`--city`, `--pro`, `--weather`,
`--time-of-day`, `--seed`, and the mod flag is `--mods`), a `--bind`
default of `127.0.0.1:47700` (actual: `127.0.0.1:0`, loopback +
ephemeral), and a `Host::recv` described as returning "the peer event
plus the message that accompanied it" (shipped: `Result<HostEvent,
RecvError>` — event only, lobby.rs:207). The phantom flags also
asserted a host-side negotiation surface the same docs correctly defer
to F24-B.3+. Ledger defect, not an implementation defect — the review
called the code slice itself solid; the fix is mechanical correction
of the docs against `mm2_host.rs`'s clap `Cli`.

## Repair

- `docs/research/net.md` "Dedicated host": rewritten against the
  actual clap surface — `--mm2-path` required (an empty dir is a valid
  content-free mount), `--mods`, `--bind` default `127.0.0.1:0`,
  `--dev-world` xor `--city` (default `london`, refused when the psdl
  does not resolve), `--pro`, `--weather`/`--time-of-day` 0–3,
  `--seed`; the `listening=` record then `event=` lines is the
  documented output contract. Vehicle/paint/name/non-cruise flags
  explicitly recorded as *not* existing yet.
- PLAN.md's F24-B.2 row: same flag/default corrections, plus
  `Host::recv` now reads `Result<HostEvent, RecvError>` (with
  `recv_timeout`/`try_recv` alongside), and an iter-013 repair note in
  the established convention.
- The iteration-012 entry below: both wrong passages corrected in
  place with a parenthetical noting what the original text claimed.

## Gates

Docs-only diff — markdown cannot move fmt/clippy/test, but the gates
were re-run anyway:

- `cargo fmt --all -- --check` — clean.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — clean.
- `cargo test --locked --workspace` — all suites green (exit 0).

## Notes

- F24-B.2 stays a candidate pending external check; this iteration
  only corrected its record. The review's verification gaps stand:
  `mm2-host` exercised only with an empty install + `--dev-world` (no
  retail-install run), clients still share the test process (full
  AC01 topology, LAN and Internet are F24-C), session revalidation of
  joined peers / join gating / vehicle-paint negotiation / start are
  later legs, and the Bevy-side in-app host (AC05's other leg) is
  unstarted.
- Defect class worth noting for future slices: the ledger was drafted
  from the *intended* CLI (which included negotiation flags later
  scoped out) rather than the shipped clap struct. Doc records of a
  binary's flags should be checked against `--help` output before
  commit.

---

# Last iteration — F24-B.2: session advertisement on the wire, the
# `mm2_app` SessionConfig bridge, the headless `mm2-host` binary
# (iteration 012, run 20260929T174954)

Implementation iteration on `ralph/night` (baseline `75cc858` — the
iteration-011 lobby-leak repair; external verify green, review **pass**
with verification gaps only, no blocking findings). One coherent slice:
the plan's named next leg of F24-B — session advertisement — landed
end-to-end so it arrives with a real consumer instead of a dead wire
field.

## Task selection

No failing gate or review finding to repair — iteration 011 passed with
verification gaps only (same-process loopback, unbounded channels,
no rate limit — all disclosed F24-C scope). The plan named "F24-B's
second leg (session advertisement — the `mm2_app` bridge mapping
`SessionConfig` ↔ wire fields, vehicle/paint pick + validation, or
start/cancel)". I scoped it to the advertisement + bridge + consumer:
advertising a session nobody can receive is a dead field, so the slice
includes the headless dedicated binary the spec's AC05 wants — which is
also the only way to exercise the advertisement in a separate process.
Vehicle/paint negotiation, start/cancel and join gating stay open as
F24-B legs three and four.

## What landed

- `mm2_net::proto` — `Message::Session` carrying
  `SessionAdvertisement { summary, params }`: a `MAX_STRING`-bounded
  display line plus an opaque blob bounded by `MAX_SESSION_PARAMS`
  (4 KiB) enforced on encode (`OversizeSessionParams`) and decode. The
  wire crate stays project-free — it ships the blob; it never parses
  cities, modes or settings.
- `mm2_net::lobby` — `Host::set_session` stores and rebroadcasts the
  advertisement to every connected peer; a newcomer receives
  Welcome → Session (if set) → Roster, so its first roster never
  precedes the session it belongs to; a peer whose session write fails
  is disconnected (`writer.disconnect()`, the iteration-011 discipline)
  and the corrected roster rebroadcasts; a client sending `Session` is
  out-of-turn and dropped as `Malformed` by the existing discipline.
  `broadcast_roster` generalized into a `broadcast` helper shared by
  both sends. `Host::recv` added — `Result<HostEvent, RecvError>`, a
  blocking wait for the next lobby event for consumers that live
  entirely on lobby traffic (the dedicated host); `recv_timeout` and
  `try_recv` sit alongside it.
- `mm2_app::net` (new module; `mm2_app` is the only crate where
  `mm2_game` and `mm2_net` may meet) — `SessionConfig` ↔
  `SessionAdvertisement`: params is bounded serde JSON of the
  session-legal fields (world/mode/difficulty/conditions/densities/
  customization/seed), and a human-readable `summary` line.
  `DevOverrides` are refused outright (`DevOverrides` error) rather than
  silently dropped; `authority`, `vehicle` and `mods_active` never
  serialize — a joining peer stamps `Remote` plus its own local state.
  Decode validates enum discriminants,
  weather/time-of-day selector ranges, density ranges and race event
  refs.
- `mm2-host` — a second `mm2_app` binary, the first real consumer and
  the AC05 binary leg: headless (no window, audio or GPU), mounts the
  VFS (`--mm2-path` required — an empty directory is a valid
  content-free mount for no-install runs; `--mods` adds a mod
  directory), computes the gameplay fingerprint, binds `--bind`
  (default `127.0.0.1:0` — loopback, ephemeral port), advertises a
  cruise `SessionConfig` built from `--dev-world` or `--city`
  (conflicting; default `london`, refused when `city/<name>.psdl` does
  not resolve), `--pro`, `--weather`, `--time-of-day` and `--seed`,
  then serves the lobby, printing `listening=<addr> fingerprint=…
  seed=… session="…"` once and one `event=` line per lobby event for
  harness consumption. (The committed text originally listed
  `--mode/--difficulty/--vehicle/--paint/--name`, `--mod` and a
  `127.0.0.1:47700` default — none of which shipped; corrected in
  iteration 013.)

## Evidence

- `mm2_net` 29 → 34 tests, real loopback sockets: session round-trip,
  oversize params refused on encode and decode, newcomer receives
  Session before its first Roster, session change rebroadcasts to all
  peers, a newcomer sees only the latest replacement, a sessionless
  lobby sends none, oversized refused before the wire.
- `mm2_app::net` 7 tests: config round-trip, malformed JSON, wrong
  authority both directions, out-of-range selectors and densities,
  oversized params, `DevOverrides` quarantine (dev-tuned config
  serializes identically to clean).
- `tests/net_host.rs` — the multi-process leg: spawns the built
  `mm2-host` (`CARGO_BIN_EXE_mm2-host`) as a separate OS process with
  `--dev-world`, reads its `listening=` line, joins two `Client`s,
  asserts the advertised session summary + params decode on both, the
  fingerprint echo in `Hello` acceptance, the two-entry roster and a
  quit reaching the host's event loop. This is partial AC01/AC05
  evidence: a *separate* host process serving real joins, configured
  bind, no window/audio — but the clients still share the test process,
  so it is not the full AC01 topology, and not LAN/Internet (AC06).

## Gates

- `cargo fmt --all -- --check` — clean (after one rustfmt reflow).
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — clean (one `field_reassign_with_default` fixed).
- `cargo test --locked --workspace` — all suites green (exit 0):
  mm2_net 34/34, mm2_app 59/59 (incl. the 7 `net::` legs),
  `tests/net_host.rs` 1/1 in ~1.9 s with `mm2-host` as a separate
  process.

## Notes

- Environment quirk worth recording: while a *previous* `cargo test`
  invocation was still churning through test binaries in the
  background, newly spawned child processes (the test's `mm2-host`
  child, and once a plain-parent spawn) intermittently stalled in
  `_dyld_start` with `com.apple.netsrc` control fds — an exec-time
  security-evaluation stall on this machine, not a code defect. After
  killing the stale pipeline, the identical binary runs instantly and
  the test passes in ~1.8 s. Consequence: never run two cargo pipelines
  concurrently on this box (which is already the shared-worktree rule).
- The advertisement is informational: a peer already in the lobby is
  not re-validated against a changed session's settings — session
  negotiation/validation is a later leg; the handshake fingerprint
  remains the only compatibility gate.
- `Host::recv` exists for `mm2-host`; the Bevy-side bridge (AC05's
  in-app leg) is unstarted. The mm2 bin's main window path is
  unaffected — `mm2_app` gained a module and a bin, no scheduling
  changes.

---

# Last iteration — external-review repair: close the F24-B.1 lobby
# drop/zombie leaks and bound `max_clients` (iteration 011, run
# 20260929T174954)

Repair iteration on `ralph/night` (baseline `2117f5a` — the F24-B.1
lobby candidate; external verify green, review **fail** on two blocking
findings).

## Root causes

Both findings are implementation defects in `mm2_net::lobby`:

1. **Host-initiated drops never disconnected the peer.**
   `broadcast_roster` removed a player whose roster write failed via
   `players.remove(&id)`, which only dropped the `Slot`'s `Writer` — a
   `try_clone`d handle on the same socket. The peer's reader thread
   stayed blocked in `conn.recv()` on the original handle, so the
   socket stayed open forever: the consumer saw `Left{Lost}`, the
   thread+fd leaked, and the client's `recv` blocked on a stale
   connection — remotely triggerable and uncapped (a write-stalled
   client kept its connection, thread and fd after being dropped, and
   no longer counted against `max_clients`/`MAX_PENDING`). The
   `PeerGone` path was safe only because the reporting reader had
   already exited and dropped its handle.
2. **`HostConfig::max_clients` was never validated against the wire
   bound.** A value above `MAX_PLAYERS` (8) admitted a roster
   `Roster::encode` cannot represent: the 9th join passed the seat
   check, was announced `Joined`, then every `broadcast_roster` send
   failed `OversizeRoster` and the whole roster was removed as `Lost` —
   a caller configuration error surfacing as silent mass disconnection.

## Repair

- Every roster removal now disconnects the peer socket:
  `broadcast_roster`'s write-failure removal calls
  `slot.writer.disconnect()` (`shutdown(Both)` wakes the blocked
  reader, which exits and drops the last handle — client sees the
  close, thread+fd reaped), and the `PeerGone` path does it uniformly
  so removal always means a closed socket. A newcomer removed by its
  own first roster send no longer gets a reader spawned for a departed
  slot.
- `Host::spawn` (the funnel for `listen`/`listen_loopback`) rejects
  `max_clients > MAX_PLAYERS` as `NetError::Config` — a new named
  variant, so a caller configuration error has a diagnostic instead of
  a mass drop.
- `conn::Writer` is re-exported (`mm2_net::Writer`) so external
  consumers can name `Conn::writer()`'s return type — the review's
  flagged pub-in-private-module wart.
- Doc overclaim corrected: `net.md`/PLAN's "slot ids never recycled"
  now reads "a live slot's id is never reused — freed ids re-mint only
  after `u16` wraparound".

## Regression legs (mm2_net 27 → 29, real loopback sockets)

- `a_failed_broadcast_disconnects_the_peer` — drives
  `broadcast_roster` over nine slots sharing one socket so the roster
  fails *encode* (`OversizeRoster`): no bytes and no FIN reach the
  peer, so every assertion isolates the removal path: all nine removed
  as `Lost`, the blocked reader thread wakes and is joined, and the
  peer's socket reads EOF rather than silence.
- `an_over_cap_max_clients_is_rejected` — `max_clients = 9` refuses to
  listen with `NetError::Config` naming the field; the bound itself
  still listens.

## Gates

- `cargo fmt --all -- --check` — clean.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — clean.
- `cargo test --locked --workspace` — all suites green (exit 0),
  mm2_net 29/29.

## Notes

- The review's non-blocking residuals stand as disclosed: loop/event
  channels are unbounded and `SetReady` → broadcast has no rate limit
  (F24-C's AC03 rate-excess leg); same-process loopback only — AC01,
  AC05, AC06 remain unevidenced; AC04 has wire-level causes but no
  consumer-facing error surface yet.
- The 9-slots-one-socket test shape exists because the production
  trigger (a peer that stops reading until `WRITE_TIMEOUT` fires)
  cannot be made fast and deterministic on real sockets; the encode
  failure exercises the identical removal path.

---

# Last iteration — F24-B.1: the `mm2_net` lobby driver — host accept
# loop, slot roster, readiness, leave/drop lifecycle (iteration 010,
# run 20260929T174954)

Implementation iteration on `ralph/night` (baseline `a8e2108` — the
iteration-009 handshake-deadline repair; external verify green, review
**pass** with verification gaps only, no blocking findings). One
coherent slice: the first leg of F24-B — the plan's named "listen
socket on a host thread + lobby roster scaffolding", now unblocked by
A.1.

## Task selection

No failing gate or review finding to repair. Iteration-009's residual
gaps (same-process loopback only, write-side frame bound, self-reported
fingerprint, Err-path deadline retention) are documented limitations,
not defects — none is repairable without the F24-C multi-process matrix
or a bigger protocol step. F24-B is the highest-value ready task; the
remaining plan candidates are still gated (F14-C needs F15-B research;
F05-B/F17-B/F18-A need C&R or replication; F07-B needs an audio device)
or evidence-only. Scoped to B.1: host/join + roster scaffolding at the
wire level. Session advertisement (`SessionConfig` cannot enter
`mm2_net` — the dependency rule keeps the crate project-free, so the
mapping belongs to the `mm2_app` bridge), vehicle/paint negotiation,
start/cancel, late-join, the Bevy bridge and the headless dedicated
binary (AC05) are later B legs.

## What landed

- `mm2_net::lobby` — `Host`: an accept thread forwards conns; a
  per-conn handshake thread runs the gate under `HANDSHAKE_TIMEOUT` and
  hands the conn back over the channel (a stalled peer never blocks
  accepts; in-flight handshakes capped by `MAX_PENDING = 8`, a flood
  drops at the door); one reader thread per player forwards
  `SetReady`/`Leave` and socket death. The single host loop owns the
  roster: mints `u16` slot ids monotonically from 1 (never recycled; 0
  reserved for the host player app-side), answers `Accept`+`Welcome`
  or `Reject{LobbyFull}` *after* the capacity check, applies ready
  changes, reaps gone peers, and rebroadcasts the complete `Roster`
  snapshot after every change. `HostEvent::{Joined, Left, ReadyChanged,
  JoinFailed}` is the consumer surface; `LeaveCause::{Quit, Lost,
  Malformed}` distinguishes clean quits, drops and protocol violations.
- `Client`: `join` (connect + `send_hello` + a bounded `Welcome` wait),
  `set_ready`, `send`, `recv`, `set_timeout`, and `leave` — which
  sends `Leave`, half-closes and drains briefly, because a socket
  dropped with unread inbound data resets and a deliberate quit would
  otherwise read as `Lost`.
- `proto`: `Welcome{player_id}`, `Roster{players}` (≤`MAX_PLAYERS`=8 —
  MP-1's documented TCP/IP ceiling — enforced on encode and decode),
  `SetReady{ready}` (strict bool byte), `Leave`; `RejectCode::LobbyFull`.
- `conn`: `accept_hello_within` factored into `recv_hello_within`
  (receive+gate+reject, no `Accept`) so the host loop can interpose the
  seat check before accepting — the public helpers' contract is
  unchanged. `Conn::writer` returns a `try_clone`d `Writer` (send +
  `disconnect` + `set_write_timeout`); `shutdown_write` backs `leave`.

## Repair found in test

`leave()` originally sent `Leave` and dropped the socket — the tests
caught it reporting `Lost`: an unread roster sat in the client's
receive buffer, so close produced RST and the host's reader errored
before seeing the queued `Leave`. The drain-on-quit above is the fix;
the cause distinction is now reliable, not best-effort.

## Evidence

- `mm2_net` 16→27 tests, all real loopback socket pairs/threads: slot
  assignment + self-in-roster, two-client grown-roster broadcast to
  incumbent and newcomer, ready rebroadcast to everyone, `Quit` vs
  `Lost` causes, fresh id on rejoin (no stale-id reuse), lobby-full
  reject via the normal handshake verdict, incompatible peer never
  rostered, out-of-turn message drops the peer, handshake-flood refusal
  at the door, host shutdown disconnects clients.
- Same-process loopback only — AC01 (multi-process lobby), AC04
  (disconnect UX), AC05 (headless dedicated binary), AC06 (scope
  matrix) stay open for F24-B/C.

## Gates

- `cargo fmt --all -- --check` — clean (after one rustfmt reflow).
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — clean.
- `cargo test --locked --workspace` — all suites green (exit 0),
  mm2_net 27/27.

## Classification / remaining open items

- Implementation choice throughout — the lobby protocol, thread layout,
  slot ids and bounds are designed. `MAX_PLAYERS=8` pins MP-1's
  documented ceiling; whether a host-as-player spends a seat is a
  `HostConfig::max_clients` decision (default 8 = dedicated host).
- Remaining B scope per PLAN.md: session advertisement, vehicle/paint
  negotiation, start/cancel, late-join, the `mm2_app` bridge, headless
  dedicated hosting, disconnect UX. No game-menu row exists to wire.
- Known limits, disclosed: broadcast writes bounded at 10 s (a reader
  that stalls mid-frame is dropped as `Lost`, not detected as
  malicious); the pending cap is a count, not a rate limiter; a host
  that `Accept`s then never sends `Welcome` stalls `join` for
  `HANDSHAKE_TIMEOUT` then errors — bounded; handshake flooding past
  `MAX_PENDING` is dropped silently (the peer sees a closed socket).

---

# Last iteration — external-review repair: bound the F24-A.1
# handshake helpers (iteration 009, run 20260929T174954)

Repair iteration on `ralph/night` (baseline `9fd61282` — the F24-A.1
candidate; external verify green, review **fail** on one blocking
finding).

## Root cause

The iteration-008 review found `conn.rs`'s `set_timeout` doc claimed
"the handshake sets this so a stalled peer cannot hang a join forever",
but neither `send_hello` nor `accept_hello` set any timeout —
`Conn::connect`/`Conn::accept` produce plain blocking sockets with no
deadline, so a peer that completes TCP and then idles hangs
`accept_hello`'s `recv` forever (and a silent host hangs `send_hello`
symmetrically) — an indefinite, remotely triggerable block on the
primary path F24-B's accept loop would have trusted. Every conn test
masked it by setting a 10 s timeout on both ends itself, so the
no-deadline default was never exercised. Implementation defect, not a
capability gap.

## Repair

Followed the review's first option — make the doc's contract true:

- `HANDSHAKE_TIMEOUT = 10 s` (designed bound: `Hello` out + verdict
  back is two small frames; generous even on a slow link).
- `send_hello`/`accept_hello` install it via `set_timeout` before any
  I/O and clear it (`None`) on `Ok` — an established control channel
  may idle between requests. On `Err` the deadline stays installed; a
  failed handshake's connection is expected to be dropped.
- `send_hello_within`/`accept_hello_within` (exported) take an
  explicit bound — the extension point for callers needing a different
  one, and how tests exercise enforcement without waiting 10 s.
- `docs/research/net.md` records the bound and the contract.

## Regression legs (mm2_net 13 → 16, all real loopback socket pairs)

- `the_handshake_helpers_install_the_default_deadline` — the missing
  no-deadline leg: a new `pair_untimed` harness pre-sets **no**
  timeout, so `read_timeout() == Some(HANDSHAKE_TIMEOUT)` observed on
  both ends after a failed handshake pins that the helpers installed
  it (not the test).
- `a_silent_client_cannot_stall_accept_hello` — client holds the
  socket open, sends nothing; `accept_hello_within` at 150 ms returns
  `NetError::Io` (WouldBlock/TimedOut) instead of hanging.
- `a_silent_host_cannot_stall_send_hello` — symmetric leg.
- `matching_peers_complete_the_handshake` now asserts
  read/write timeouts are `None` on both ends after `Ok` — the
  established-session clear is pinned too.

## Gates

- `cargo fmt --all -- --check` — clean (after one rustfmt reflow).
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — clean.
- `cargo test --locked --workspace` — all suites green (exit 0),
  mm2_net 16/16.

## Notes

- API surface added: `HANDSHAKE_TIMEOUT`, `send_hello_within`,
  `accept_hello_within` — `mm2_net` still has no production consumers.
- The review's other verification gaps stand unchanged: same-process
  loopback only (AC01/AC04/AC05/AC06 remain F24-B/C work), outgoing
  frames bounded only by `u32::MAX` on the write side (encode cannot
  exceed ~520 B today), fingerprint is self-reported not adversarial.
- The minor rustdoc nit from the same review (`[`PROTOCOL_VERSION`]`
  link in `mm2_content::fingerprint` wrapping a whole sentence) was
  left as-is — cosmetic, non-blocking, outside this repair's scope.

---

# Last iteration — F24-A.1: multiplayer protocol foundation —
# transport decision, framed wire protocol, content fingerprints,
# loopback handshake (iteration 008, run 20260929T174954)

Implementation iteration on `ralph/night` (baseline `679083f` — the
iteration-007 clippy repair; external verify green, review **pass**
with verification gaps only, no blocking findings). One coherent
slice: the first leg of F24-A — every queued task F24→F27 plus the
deferred replication legs in F05-B/F10-C sit behind it, and all of
F24-A's deps (F01-B checked, F02-A implemented) are satisfied.

## Task selection

No failing gate or review finding to repair — iteration 007 passed
with non-blocking residuals only (verbatim vertex reads, `.bnd`
families outside `build_model`, a narrow derived-width corner, doc
slips — all disclosed). The selection-policy list's candidates are
mostly gated (F14-C completability needs F15-B traversal research;
F05-B/F17-B/F18-A remainders need C&R/replication modes; F07-B needs
an audio device; F16-C's process leg needs interactive play) or
evidence-only (F11-C names no new runtime work). F24-A is the
highest-value *ready* task in TASKS.json: it roots the entire
multiplayer subtree. Scoped to A.1 — transport choice, protocol IDs,
authority boundary and content fingerprints — per the spec's own
"do not implement this whole document" rule; lobby/readiness and the
app wiring are F24-B.

## What landed

- `crates/mm2_net` (new crate, no project-local deps): `frame` — a
  `u32le` length-prefixed codec bounded by `MAX_FRAME` (256 KiB)
  checked *before* allocation; `proto` — `PROTOCOL_VERSION` 1, strict
  LE message codec (`Hello`/`Accept`/`Reject`, capped strings, hard
  errors on unknown tags/truncation/trailing bytes), and `admit`, the
  pure compatibility gate (exact version + gameplay-fingerprint
  match); `conn` — blocking `std::net` TCP wrapper plus the
  `send_hello`/`accept_hello` handshake pair (a refused peer gets a
  named `Reject`, not a dropped socket) and `listen_loopback`
  (`127.0.0.1:0` only — no public bind exists).
- `mm2_assets::fingerprint` — the FNV-1a-64 helper and the
  provenance+size catalog fingerprint moved here from `mm2_inspect`
  so the handshake and the auditor share one implementation. The
  inventory's retail value is bit-identical (`fnv1a64:e91e6cd4b2ae30d9`
  — verified by re-running the command, not just the unit test).
- `mm2_content::fingerprint::gameplay` — FNV-1a-64 over the resolved
  *bytes* of every gameplay-relevant logical path (`tune/`, `bound/`,
  `geometry/`, `race/`, `anim/`, `players/`, and `city/` minus the
  visual-only `.sky`/`.ldef`/`.lmap`/`.cpvs`/`.pvs`/`.pvshist`/`.ltNN`
  records). `path ‖ len ‖ bytes` per file, so edits, retargets and
  deletions all move it while texture/audio/menu-art changes cannot.
- `mm2-inspect fingerprint <dir>` — prints both fingerprints with the
  gameplay denominator (files + bytes hashed).
- `docs/research/net.md` — the transport decision record: TCP
  control plane on `std::net` (no async runtime in the tree; the
  lobby is low-rate request/response), blocking sockets + threads
  keeping the crate Bevy-free so a headless host needs no window/GPU;
  the per-tick dataplane is deliberately deferred to F25 (UDP
  candidate, decided with the replication design); loopback-only
  bind policy; auth/encryption deferred to maintained crates, not
  hand-rolled. `docs/architecture.md` gained the `mm2_net` boundary
  rule.

## Evidence

- `mm2_net` 13 tests green: real `127.0.0.1` socket pairs through the
  production helpers — accept round-trip, `VersionMismatch` and
  `ContentMismatch` rejects reaching *both* ends, non-Hello first
  frame → `Malformed` reject delivered to the peer, oversize frame
  refused before allocation, truncated/empty reads → `UnexpectedEof`,
  strict decode errors (bad tag/code, over-cap string, non-UTF-8,
  trailing bytes).
- `mm2_content` fingerprint tests green: classifier table,
  cosmetic-only change invariance (texture + `ldef` edits), gameplay
  edit/deletion sensitivity, mod-override sensitivity.
- Retail (`fnv1a64:e91e6cd4b2ae30d9`, read-only): `fingerprint`
  reports `catalog fnv1a64:e91e6cd4b2ae30d9` (unchanged by the
  refactor) and `gameplay fnv1a64:612016d26bd31b59` over **5,593**
  files / 28.8 MB in well under a second. Live mod legs: a texture
  override mod moves the catalog fingerprint (`…2bf7a274…`) while the
  gameplay fingerprint and its denominator stay identical — the
  AC02 cosmetic leg; a `tune/` mod moves the gameplay fingerprint
  (`…62a401dc…`, 5,594 files) — the AC02 gameplay leg.

## Gates

- `cargo fmt --all -- --check` — clean.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — clean.
- `cargo test --locked --workspace` — all suites green (exit 0),
  incl. mm2_net's 13 and mm2_content's 4 new legs.

## Classification / remaining open items

- Implementation choice throughout — `PROTOCOL_VERSION`, the frame
  format, `MAX_FRAME`, the fingerprint classifier and the transport
  are designed, not recovered original behavior (DirectPlay interop
  is a spec non-goal).
- Authority boundary for A.1 is documentation + handshake direction
  (host decides); the per-message authority split lands with the
  message set it constrains in F24-B.
- Evidence is same-process loopback — not multi-process, LAN or
  Internet. AC01 (host + two clients + lobby), AC04 (disconnect/host
  loss UX), AC05 (headless dedicated bind) and AC06's scope matrix all
  stay open for F24-B/F24-C.
- `Hello.driver`/`build` are carried but have no lobby consumer yet;
  the gameplay fingerprint is computed on demand (a per-session cache
  can come with the consumer). No app/menu wiring — there is no
  multiplayer menu row to fake.

---

# Last iteration — external-gate repair: clippy 1.98 `redundant_closure`
# in `mm2_content::model` (iteration 007, run 20260929T174954)

Repair iteration on `ralph/night` (baseline `5085e54` — the
iteration-006 `.mtx` magnitude-bound notes). External verify on that
candidate **failed** (exit 101,
`external_code_gate_failed_or_interrupted`).

## Root cause

Environment drift, not a logic defect: `rust-toolchain.toml` pins
`channel = "stable"`, and the installed stable floated from 1.97.1
(the run's gate toolchain) to 1.98.1 before the external verify ran.
Clippy 1.98's `redundant_closure` fires on two `model.rs` closures the
iteration-006 gate run accepted:

- `model.rs:376` — `.filter(|c| usable3(c))` (the wheel geometry-centre
  gate);
- `model.rs:542` — `.filter_map(|p| usable_aabb(p))` (the body-bound
  pass).

## Repair

Applied clippy's own suggestions verbatim — `.filter(usable3)` and
`.filter_map(&mut usable_aabb)` (`&mut` because the closure is `FnMut`
over `bound_warned`/`warnings` and is reused by the rest-parts pass at
line ~567). No semantic change; behaviour stays covered by the existing
`model::tests` legs and `dash::overflowing_dash_mtx_origin_reads_unauthored`.

## Gates (rustc/clippy 1.98.1)

- `cargo fmt --all -- --check` — clean.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — clean.
- `cargo test --locked --workspace` — all suites green (exit 0).

## Notes

- The floating `stable` channel means external verify can run a newer
  clippy than the in-iteration gate did; new-lint failures of this shape
  can recur. Whether to pin the toolchain is a repo policy decision —
  not changed here.
- Iteration-006's substantive claims are unchanged; this was a lint-only
  repair. No new test leg — the change is mechanical, and the covering
  tests are unchanged and green.

---

# Last iteration — authored-numbers robustness: magnitude-bound the
# `.mtx`/pkg-geometry family at `build_model` (iteration 006, run
# 20260929T174954)

Implementation iteration on `ralph/night` (baseline `5b9f6f0` — the
iteration-005 camera/dash magnitude-bound slice; external verify green,
review **pass** with no blocking findings). One coherent slice: close
the iteration-005 review's flagged residual — the `USABLE_BOUND` gate
reached `.mtx` data only at the dash consumer's `part.origin` read, the
hostile-origin path was untested, and every other `build_model`
consumer (vehicle part attach points, wheel origins, `WheelGeom`
physics conversion, `body_aabb`) still bound `.mtx` fields verbatim.

## Task selection

The review named three residuals in this family: `part.origin` bounded
at the dash consumer only, the same-shaped overflow reachable through
vehicle part/wheel origins, and no test leg through a hostile `.mtx`
origin at all. `Mtx::parse` has exactly two call sites
(`mm2_content::assemble` and `mm2_app::dash`), and both funnel into
`build_model` — the sole producer of `ModelPart.origin`,
`ModelPart.pivot`, `WheelVisual.origin` and `body_aabb` — so gating at
the producer covers every downstream composition (`pivot + offset`,
wheel-position `WheelGeom` math, the AABB translation) with one gate
rather than a per-consumer sweep. Same defect shape as iteration 005,
sibling record family, same fix contract.

## What landed

- `mm2_formats::mtx` — `Mtx::validate()` names each unusable field
  (`bounds_min`/`bounds_max`/`pivot`/`origin`) through the shared
  `vec_issue` helper, distinguishing "is not finite" from "exceeds the
  usable bound ±1e6". Raw fields stay verbatim — reported, not
  repaired. `camtrack`'s `USABLE_BOUND`/`usable3` docs now record the
  `.mtx` family sharing the bound.
- `mm2_content::model::build_model` — the central gate. Per part:
  `m.validate()` issues push into `model.warnings` (drained by the
  loaders); `part.origin` binds only when `usable3`, else reads
  unauthored (`None` = authored in place); `part.pivot` binds only
  when usable *and* non-zero as before. The wheel rig reads the
  already-gated `part.origin`, gates `wheel_radius`/`wheel_width`
  against `usable_f32`, gates the in-place-recentre target and the
  measured y/x extents (hostile pkg vertices can no longer inflate
  `radius`/`width`), and takes the geometry-centre fallback — itself
  gated — with a warning distinguishing "no mtx" from "unusable mtx".
  `body_aabb` excludes a part whose measured bound is unusable (one
  warning per part across the BODY/rest passes) instead of letting a
  `3e38` vertex inflate the bound the convert path centres mass on.
- `mm2_app::dash::spawn_dash` — drains `model.warnings` to `warn!` so
  the diagnostics the producer records actually surface.

## Evidence

- `mm2_formats` green, incl.
  `mtx::validate_names_unusable_fields_verbatim` (per-field naming,
  distinct finite/bound messages, verbatim retention).
- `mm2_content` green, incl.
  `model::unusable_mtx_fields_read_unauthored` (3e38 origin → `None`,
  inf pivot → `None`, wheel falls back to the geometry centre, all
  four fields + the fallback named in warnings),
  `model::usable_mtx_fields_bind_verbatim` (origin/pivot/wheel-centre
  bind verbatim, no warnings),
  `model::unusable_geometry_measurements_read_unauthored` (3e38 vertex
  cloud → geometry-centre `[0;3]`, designed radius floor, `body_aabb`
  excluded).
- `mm2_app` green, incl.
  `dash::overflowing_dash_mtx_origin_reads_unauthored` — the missing
  leg the review named: a synthetic `_dash.pkg` + `_dash.asnode` +
  hostile `.mtx` records through the real VFS; the needle's `3.4e38`
  origin and the wheel's `2e6` origin both read unauthored (nodes at
  the authored offsets), a rewritten usable `0.05` origin binds
  verbatim on respawn, and every `CockpitPart` transform stays finite
  through `drive_dash`.
- Retail audit (`fnv1a64:e91e6cd4b2ae30d9`, read-only): all **819**
  `.mtx` records across the DAVE archives scanned component-wise
  (9,828 `f32`s — every record in every archive, not just vehicles) —
  zero non-finite, max `|v|` = `2566.79` (a `bl_*` city prop bound) →
  the `1e6` bound rejects nothing authored. No loose `.mtx` files
  outside the archives.
- `mm2-inspect validate-cars` — 21/21 stock vehicles `ok`, warnings
  identical to the pre-change set (audio row counts, paint counts,
  back-back followers); `mm2-inspect handling` — all 21 within the
  arcade envelope, so the wheel-rig derivation is unchanged on stock.

## Gates

- `cargo fmt --all -- --check` — clean.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — clean.
- `cargo test --locked --workspace` — all suites green (exit 0).

## Classification / remaining open items

- Implementation choice throughout — `USABLE_BOUND` is a designed
  overflow guard shared by the camera/dash and `.mtx` families, not a
  recovered original limit. Sincere mods under ±1e6 still bind
  verbatim.
- Semantic widening: a finite `.mtx` field beyond ±1e6 now reads
  unauthored where it previously bound verbatim — the intended
  contract; `validate` names the field and loaders warn.
- No GPU/rendered/audio/network evidence this slice; none claimed —
  finiteness verified on component values in headless Bevy worlds.
- Still open: `authored-numbers.md` S1–S5 speculative list, the
  F19-A.4 ped sweep caveat; `PovCamSpec::track_to` and
  `DashSpec::gear_pivot_offset` remain validate-only (disclosed).
  `ModelPart.pivot` is gated at the producer but currently has no
  downstream reader — stored for future consumers. `.bnd` bound data
  and other record families outside `build_model` keep verbatim
  readers — a wider sweep is not claimed.

---

# Last iteration — authored-numbers robustness: magnitude-bound the
# camera/dash spec family (iteration 005, run 20260929T174954)

Implementation iteration on `ralph/night` (baseline `6c3823e` — the
iteration-004 finite-gate slice; external verify green, review **pass**
with no blocking findings). One coherent slice: close the asymmetric
overflow residual the iteration-004 review named as a verification gap
— finite-but-astronomical authored values still bound verbatim, and
only `Offset` had a derived-length overflow check.

## Task selection

The review's first verification gap was the only concrete residual of
the F22-B camera/dash contract: a finite ~`3e38` `TrackTo` can still
overflow `veh_rot * aim` into a non-finite look target (NaN via
`Transform::look_at`), `eye + v3(DashPos)` can overflow to an `inf`
translation from two finite inputs, and the same shape sits in
`MaxDist` → `dir * dist`, the `pivot + offset + pivot_offset` needle
chains, the ±sweep `(max − min) * frac`, `WheelFact * π`, and
`Pitch`/`Offset` child transforms. Component-finite gating cannot
close that class — the inputs are already finite — so the gate needed
a designed magnitude bound. (Verification gaps are not blocking
findings; this was selected ahead of TASKS-queue feature slices
because it is the unfinished edge of the just-checked work, and the
queued slices' deps are unchanged.)

## What landed

- `mm2_formats::camtrack` — `USABLE_BOUND = 1e6` (designed bound:
  orders above anything authored — retail max is `CameraFar 1330` —
  orders below `f32::MAX`, so every composed sum/product stays
  finite). The family's read gate is now `usable_f32`/`usable3` +
  `usable1`/`usable_vec` Option filters (replacing `finite3`/
  `finite1`/`finite_vec`): a field reads unauthored when non-finite
  *or* beyond the bound, `validate` distinguishes the two cases
  ("is not finite" vs "exceeds the usable bound ±1e6") via shared
  `vec_issue`/`scalar_issue` helpers, and the raw fields stay
  verbatim. `CameraFOV` keeps its tighter drawable `(0, 180)` bound.
- `mm2_formats::dash` — `PovCamSpec` and `DashSpec` read through the
  same helpers; both `validate()`s name beyond-bound fields the same
  way (needle sweeps get the pair-level message).
- `mm2_app::camera` — `ChaseLens::authored`'s hand-rolled
  offset-length filter is subsumed by the bound and removed; `aim`,
  the dist/speed windows, clips and flags all inherit it through the
  accessors (`3e38` `TrackTo`/`MaxDist` can no longer reach
  `veh_rot * …`).
- `mm2_app::dash` — `v3` reads through `usable_vec`, the `rot` closure
  through `usable_f32`, `WheelFact` through `usable1`, and the
  pkg/mtx-authored `part.origin` pivot joins the gate (a `3e38` binary
  origin would overflow `pivot + offset` identically — same defect
  shape, sibling record family).

## Evidence

- `mm2_formats` tests green, incl.
  `camtrack::beyond_bound_fields_are_named_and_read_unauthored`
  (verbatim retention, `validate` naming with the distinct bound
  message, accessors unauthored, ±bound edge legs),
  `dash::beyond_bound_fields_are_named_and_read_unauthored` (asnode +
  campovcs legs).
- `mm2_app` suites green, incl.
  `camtrack::overflowing_authored_fields_fall_back_to_the_designed_boom`
  (`3e38` TrackTo/MaxDist/etc → designed lens values + 60 finite
  `chase_follow` frames at speed),
  `dash::overflowing_asnode_reads_unauthored` (synthetic hostile
  `_dash.asnode` + `3e38` `campovcs` eye through the real VFS; parked
  sweep, `WheelFact` 1.0, all spawned transforms finite through
  `drive_dash`), `mirror::overflowing_authored_pov_fields_fall_back`.
- Retail audit (`fnv1a64:e91e6cd4b2ae30d9`, read-only): all 119
  authored camera/dash records re-dumped — 4,550 numeric tokens, zero
  non-finite, max `|v|` = `CameraFar 1330` → the `1e6` bound rejects
  nothing authored.

## Gates

- `cargo fmt --all -- --check` — clean.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — clean.
- `cargo test --locked --workspace` — all suites green (exit 0).

## Classification / remaining open items

- Implementation choice throughout — `USABLE_BOUND` is a designed
  overflow guard, not a plausibility claim about the original's own
  limits (unrecovered). Sincere mods under ±1e6 still bind verbatim.
- No GPU/rendered/audio/network evidence this slice; none claimed —
  finiteness verified on component values in headless Bevy worlds.
- Still open: `authored-numbers.md` S1–S5 speculative list, the
  F19-A.4 ped sweep caveat; `PovCamSpec::track_to` still has no
  consumer (validated, kept verbatim); `DashSpec::gear_pivot_offset`
  likewise. `part.origin` is bounded at this consumer only — a wider
  pkg/mtx sweep is not claimed.

---

# Last iteration — authored-numbers robustness: finite-gate the rest
# of the camera/dash spec family (iteration 004, run 20260929T174954)

Implementation iteration on `ralph/night` (baseline `01fad8f` — the
iteration-003 sweep completion; external verify green, review **pass**
with no blocking findings). One coherent slice: extend the finding-11
"non-finite reads unauthored + `validate()` names it" contract to every
remaining consumed field in the `camTrackCS`/`camPovCS`/`asNode` record
family — the exact residual the iteration-003 review flagged
(`DashSpec`'s `wheel_fact`/`speed_rot`/`rpm_rot`/`damage_rot`) plus the
same-shaped fields it implies.

## Task selection

No failing gate or review finding to repair. The queued TASKS work
carries active dependencies, and the iteration-003 review's
verification gaps named one concrete residual: `CameraFOV` was the only
finite-gated field — the other NaN/Infinity-capable fields on the same
records still reached transforms, projections, boom math and dashboard
rotations raw. `nan` survives `f32::clamp`/`f32::max` sinks (a `nan`
`CameraFar` became a 1 m far plane), a `nan` boolean-like flag
(`CollideType`, `MinMaxOn`) reads `!= 0.0` — silently *on* — and a
`nan` `Offset`/`TrackTo` poisons the whole chase transform.

## What landed

- `mm2_formats::camtrack` — shared `finite3`/`finite1`/`finite_vec`
  helpers; `TrackCamSpec` gained `offset_vec`, `track_to_vec`,
  `collides`, `min_max_gated`, `min_dist_m`/`max_dist_m`,
  `min_speed_mps`/`max_speed_mps`, `camera_near_m`/`camera_far_m` — all
  reading `Some`/`true` only when authored *and* finite. `validate()`
  now names every non-finite typed field (`Offset`, `TrackTo`,
  `CollideType`, `MinMaxOn`, `TrackBreak`, `MinDist`, `MaxDist`,
  `MinSpeed`, `MaxSpeed`, `LookAbove`, `LookAt`, `VertOffset`,
  `BlendTime`, `BlendGoal`, `CameraNear`, `CameraFar`), keeping
  `CameraFOV`'s drawable-range report.
- `mm2_formats::dash` — `PovCamSpec` gained `offset_vec`,
  `reverse_offset_vec`, `pitch_rad`, `camera_near_m`, `camera_far_m`
  and a matching `validate()` extension (`Offset`, `ReverseOffset`,
  `TrackTo`, `Pitch`, `CameraNear`, `CameraFar`). `DashSpec` gained its
  first `validate()` — the eleven `*Pos`/`*Offset` placement vectors,
  `WheelFact`, and the three `*RotMin`/`*RotMax` needle sweeps.
- `mm2_app::camera` — `ChaseLens::authored` reads every field through
  the accessors; an astronomical-but-finite `Offset` whose derived
  length overflows also reads unauthored. `spawn_mirror` reads the pov
  eye and clips through the same accessors.
- `mm2_app::dash` — `load_pov_cam` already warned; `spawn_dash` now
  warns each `DashSpec::validate` issue with the file path, `v3`/`rot`
  finite-filter every placement and sweep (a non-finite sweep parks the
  needle at `(0, 0)`), `wheel_fact` reads through `.filter`, and the
  cockpit camera binds `offset_vec`/`reverse_offset_vec`/`pitch_rad`/
  `camera_near_m`/`camera_far_m`.

## Evidence

- `mm2_formats` 247 unit tests green, incl.
  `camtrack::non_finite_fields_are_named_and_read_unauthored`,
  `dash::dash_spec_non_finite_fields_are_named`,
  `dash::pov_cam_spec_non_finite_fields_read_unauthored` — each asserts
  verbatim retention, `validate()` naming, unauthored reads and
  finite-authored verbatim binding.
- `mm2_app` suites green, incl.
  `camtrack::non_finite_authored_fields_fall_back_to_the_designed_boom`
  (hostile spec → designed boom `(0, 1.8, 5)`/aim `(0, 1, 0)`, finite
  projection and `chase_follow` steps; a `3e38` offset whose length
  overflows also falls back), `dash::non_finite_authored_pov_fields_fall_back`,
  `dash::hostile_asnode_reads_unauthored` (a synthetic `_dash.pkg` +
  hostile `_dash.asnode` mounts through the real VFS path; both parts
  bind, sweeps park, `WheelFact` defaults to 1.0, and every spawned
  transform stays finite through `drive_dash`),
  `mirror::non_finite_authored_pov_fields_fall_back`.
- Retail audit (`fnv1a64:e91e6cd4b2ae30d9`, read-only): all 119
  authored camera/dash records (49 `camtrackcs`, 47 `campovcs`, 23
  `_dash.asnode`) dumped via `mm2-inspect dump` and scanned — **zero**
  non-finite or f32-overflowing numeric tokens in 4,550 fields, so the
  new gates reject nothing authored.

## Gates

- `cargo fmt --all -- --check` — clean.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — clean.
- `cargo test --locked --workspace` — all 82 suites green (exit 0).

## Classification / remaining open items

- Implementation choice throughout — robustness bounds on
  hostile-but-grammar-valid input; no original-behavior claim. The
  designed defaults (boom, eye, clips, parked needle, `WheelFact` 1.0)
  are documented as designed.
- No GPU/rendered/audio/network evidence this slice; none claimed —
  projection/transform finiteness verified on component values in
  headless Bevy worlds.
- Still open in `authored-numbers.md`: the S1–S5 speculative list and
  the F19-A.4 ped sweep caveat. `PovCamSpec::track_to` is validated
  (kept verbatim) but still has no consumer — blend semantics
  unrecovered.

---

# Last iteration — authored-numbers sweep, second half: findings 5, 6,
# 8, 10, 11 (iteration 003, run 20260929T174954)

Implementation iteration on `ralph/night` (baseline `d9e3b8f` — the
iteration-002 repair and notes; external verify green, review **pass**
with no blocking findings). One coherent slice: close the five
remaining confirmed findings in `docs/research/authored-numbers.md`
(operator report 5's defect class — unchecked arithmetic on authored
numbers).

## Task selection

No failing gate or review finding to repair — iteration 002's candidate
passed external review with verification gaps only. The audit doc's
five still-open confirmed findings were the highest-value ready work:
each has a traced reachability path and a recorded fix shape, and
closing them completes the sweep rather than leaving a tail of known
defects. The speculative list (S1–S5) was deliberately not acted on —
it is explicitly not verified enough.

## What landed

- **Finding 5** (`race.rs` / `race_def.rs`) —
  `RaceDefinition::validate` gained `RaceError::NonFiniteGate` (a
  checkpoint or finish `center`/`heading_deg` non-finite) and
  `RaceError::NonFiniteStart` (a start slot's `position` or authored
  `yaw_deg` non-finite). The producer's existing
  `definition.validate()?` (`race_def.rs:162`) routes either into
  `RaceBuildError::Invalid`, so a `nan`/`1e999` start-points row fails
  the load instead of spawning a live NaN-posed body (the debug-profile
  Avian `assert_components_finite` panic the audit traced).
- **Finding 6** (`props.rs` / `proprules.rs`) — `walk_prop_rules` now
  skips a def with non-finite `start`/`distance`/`lerp_min`/`lerp_max`,
  negative `start` or non-positive `distance`, pushing a bounded
  `stats.issues` line naming the def and values before any arithmetic.
  The placement count is computed in `f64` and bounded by `maxUse`
  *before* the truncating cast — the old `… as u64) + 1` overflowed on
  a saturated cast (release wrapped to `want = 0`, silently deleting
  every prop on that side). `PropDefs::validate` gained
  `PropRuleIssue::NonFiniteField` naming the field and authored value;
  the pre-existing `NegativeStart`/`NonPositiveDistance` comparisons
  stay finite-only so a NaN is not misreported as both.
- **Finding 8** (`effects.rs` + app consumers) — shared
  `flipbook_span(start, end)` (`checked_sub` + `checked_add`) backs all
  three sites. `VehicleSmoke::puff` and `Precipitation::drop` now
  return `Option` — a window that cannot fit `i64`
  (`TexFrameStart i64::MIN`, `TexFrameEnd i64::MAX`) declines *before*
  drawing on the seeded RNG, preserving deterministic stream alignment;
  `PrecipDrop::frame` returns `Option<i64>`. `WheelPuff::frame` was
  already safe-by-policy and shares the helper, pinning the start tile
  on a hand-built hostile pair. `SmokeFxReport`/`PrecipReport` gained
  `undrawable` counters, surfaced in the headless record as `+Nu`. An
  inverted-but-representable window still pins the start tile.
- **Finding 10** (`crashdata.rs`) — `Event`, `Checkpoints` and the
  integer tail columns now parse through `int_cell`
  (`parse::<i64>()` → `TableDiagnostic` + row skip), matching
  `racedata.rs`: `2.7`, `nan`, `1e30` are diagnosed instead of silently
  truncated/saturated (`nan → 0` used to decode to a valid-looking
  `Jump` objective). `TimeLimit`/`AmbDensity` gained an `is_finite`
  diagnostic via `num_cell`, taking up the audit's parenthetical.
- **Finding 11** (`camtrack.rs` / `dash.rs` + app consumers) —
  `camtrack::drawable_fov` bounds `CameraFOV` to the open `(0, 180)`
  degree interval (finite required). `TrackCamSpec` and `PovCamSpec`
  gained `camera_fov_deg()` — an undrawable authored value reads as
  *unauthored* so the designed lens stands in (chase 70°, cockpit and
  mirror 60°) — plus `validate()`; `load_track_cams` and `load_pov_cam`
  `warn!` each issue with the file path, matching the existing loader
  pattern. The raw field stays verbatim — reported, not clamped.

## Evidence

- `mm2_formats` 244 unit tests + 24 vehicle-format integration tests
  green, incl. `propdefs_non_finite_fields_are_named`,
  `integer_columns_reject_non_integral_cells`,
  `non_finite_decimal_cells_are_diagnostics`,
  `undrawable_camera_fov_is_named_and_reads_unauthored`.
- `mm2_game` suites green, incl.
  `definition_validation_rejects_non_finite_authored_values`,
  `hostile_propdefs_skip_and_report_instead_of_stamping_nan`,
  `unrepresentable_flipbook_windows_decline_without_drawing`,
  `precip_declines_an_unrepresentable_flipbook_window` and the extended
  `precip_drop_frame_sweeps_the_authored_tiles`.
- `mm2_app` all suites green, incl. the three
  `undrawable_authored_fov_falls_back_to_the_designed_lens` legs
  (chase/cockpit/mirror) and the updated `Option`-typed smoke/precip
  call sites.
- Retail audits on `fnv1a64:e91e6cd4b2ae30d9` (read-only):
  `race-defs --strict` exits 0 — 45 sf events, 64 defs built, 0 failed
  builds (the new gate rejects no retail event);
  `crash-course --strict` exits 0 — 13/13 lessons ready (the `i64`
  columns parse every retail row clean);
  `proprules --strict` exits 2 on the same 48 pre-existing issues —
  zero new `NonFiniteField` diagnostics (the gate adds no false
  positives on stock data).

## Gates

- `cargo fmt --all -- --check` — clean.
- `cargo clippy --workspace --all-targets --all-features --
  -D warnings` — clean.
- `cargo test --workspace` — all suites green. Caveat: this machine
  intermittently stalls test binaries in dyld startup (0 CPU, never
  reaches `main`); six stalled binaries were rerun individually and
  every suite passed. Environment flake, not a code failure.

## Classification / remaining open items

- Implementation choice throughout — robustness bounds on
  hostile-but-grammar-valid input; no original-behavior claim. Camera
  FOV fallbacks are the designed lenses, explicitly not authored
  provenance.
- All eleven confirmed findings in `docs/research/authored-numbers.md`
  are now `Status: fixed` with named tests. Still open there: the S1–S5
  speculative list, and the F19-A.4 ped code outside the summation
  class still wants a dedicated sweep.
- No GPU/rendered/audio/network evidence this slice; none claimed.

---

# Last iteration — review repair: unbounded authored `mtxv`/`mtxn` sums
# in the ped code (iteration 002, run 20260929T174954)

Review-repair iteration on `ralph/night` (baseline `52359c9` — the
iteration-001 authored-numbers hardening; external verify green, review
**failed** with one blocking finding). One scoped repair: the same
unchecked-arithmetic-on-authored-numbers class the iteration was fixing
survived inside the F19-A.4 ped code it co-landed with.

## Finding and root cause

External review (task F19-A) found `matrix_bucket` in
`crates/mm2_game/src/ped.rs` accumulating the full-range authored `i64`
counts from `mtxv`/`mtxn` rows with a plain `at += count` — a hostile
`.mod` (`mtxv 1 9223372036854775807 1`) overflow-panics under
`overflow-checks` and wraps to a wrong-but-in-range bone binding in
release, contradicting `PedSkin`'s documented "errors or recorded
issues, never silently reshaped" contract. In-tree it was masked only
because `mm2-inspect peds` calls `PedMod::validate()` first, whose own
pre-existing `iter().sum()` accumulations over the same authored counts
panic on the same input (reproduced by the reviewer: exit 101,
"attempt to add with overflow").

## Actions

- `mm2_game::ped::matrix_bucket` — the cursor now saturates
  (`at = at.saturating_add(count)`). Correct for every reachable input:
  both call sites pass an already-range-checked resource index
  (< `i64::MAX`), so the first bucket whose running total saturates owns
  every not-yet-claimed index.
- `mm2_formats::ped::PedMod::validate` — all six `iter().sum()`
  accumulations over authored `i64`s now sum in `i128` (the `mtxv` and
  `mtxn` partition pre-checks, the `claimed_packets`/`claimed_adj`/
  `claimed_prims` material-claim sums, and the trailer `sums to`
  check). `i128` keeps the diagnostics' printed totals exact rather
  than reporting a saturated `i64::MAX`.
- `docs/research/authored-numbers.md` — the coverage-gap note records
  the post-landing pass, the defect, and the fix shape; the rest of the
  F19-A.4 ped code is still flagged as wanting a dedicated sweep.

## Evidence

- `cargo test -p mm2_formats ped` — 24/24 incl. new
  `mod_validate_survives_unbounded_authored_counts` (hostile
  `mtxv`/`mtxn` + `adjuncts:`/`primitives:`/`packets:` claims →
  diagnostics, no panic, sums not wrapped).
- `cargo test -p mm2_game ped` — 21/21 incl. new
  `skin_buckets_indices_past_a_saturating_mtxv_count` (verts past the
  huge count bucket correctly; `mtxn` agreement kept).
- `cargo test -p mm2_inspect peds` — 9/9 incl. new
  `audit_survives_hostile_mod_partition_counts` (synthetic install,
  flat-dialect hostile `mtxv` → audit completes, issue recorded, skin
  still assembles).
- Binary-level repro of the reviewer's case: synthetic install with
  `mtxv 1 9223372036854775807 1` → `mm2-inspect peds <dir> --strict`
  exits 2 reporting `mtxv sums to 9223372036854775809, expected 3` and
  the `mtxn` disagreement (was: exit 101 panic in `PedMod::validate`).
- Retail (`fnv1a64:e91e6cd4b2ae30d9`): `mm2-inspect peds --strict`
  exits 0 — `skins: 4 assembled, 292 deform samples`, quirk/issue lists
  identical to the F19-A.4 run.

## Gates

- `cargo fmt --all -- --check` — clean.
- `cargo clippy --workspace --all-targets --all-features --
  -D warnings` — clean.
- `cargo test --locked --workspace` — all suites green (exit 0).

## Classification / remaining open items

- Implementation choice throughout — robustness bounds on
  hostile-but-grammar-valid input; no original-behavior claim.
- All prior open items stand: authored-numbers findings 5, 6, 8, 10, 11
  remain open; F19-A stays `active` (F19-AC02..AC06 unclaimed — this
  slice is still domain-types/audit only, no rendered evidence); the
  F19-A.4 ped code beyond the summation class still wants a sweep pass;
  F19-A.2 review minors still open.

---

# Last iteration — authored-numbers hardening: four panic/hang-class
# findings from operator report 5 (iteration 001, run 20260929T174954)

First implementation iteration of run `20260929T174954` on `ralph/night`
(baseline `dc8d847` — operator report 5 plus its companion static sweep,
`docs/research/authored-numbers.md`). One coherent slice: repair the
audit findings that panic or hang in **both** build profiles — the four
the sweep itself ranked first (findings 1, 2, 3) plus the shared
tune-scalar root cause behind findings 4, 7 and 9.

## Task selection

Operator report 5 is marked PRIORITY: unchecked arithmetic on authored
numbers is a recurring defect class (16 review findings, 7 blocking)
that the external gate structurally cannot see because retail data is
well-formed. The companion sweep enumerates 11 confirmed-reachable
instances; findings 1–4 are the ones it names "worth turning into
regression tests first" since each panics or hangs in both profiles.
This iteration fixes those plus findings 7 and 9, which share finding
4's root cause (un-validated tune scalar readers). Findings 5, 6, 8,
10, 11 remain open — documented in the audit doc, not silently dropped.

## What landed

- **Finding 1** — `reanchor_pose` (`mm2_app::opponents`) could spin
  forever on a closed `.opp` route whose every leg has zero XZ length:
  `walked` never advanced and the open-route escape was disabled. The
  walk now carries `REANCHOR_MAX_STEPS` (16,384) in addition to
  `REANCHOR_WALK`, a non-finite candidate pose is never returned (the
  input pose stands in), and `n == 1` no longer hands back a non-finite
  anchor. Upstream, `OpponentRoute::drivable` (`mm2_game::opponent`)
  reports a non-finite or XZ-collapsed route at distillation as
  `OpponentIssue::DegenerateRoute` — the authored roster slot is kept
  with no wired route, same convention as `UnresolvedRoute`.
- **Finding 2** — `NavGraph::build` ran union-find over authored
  `Intersection::roads` indices with no range check (`Bai::validate`
  could report it, but nothing gated the build on validation). A
  dangling reference is now reported as
  `NavIssue::DanglingIntersectionRoad { intersection, road }` — the
  same shape `BaiIssue` uses — and only in-range pairs are unioned.
- **Finding 3** — `pkg.rs`'s `parse_geometry` capped index *counts* but
  never checked an index against the strip's vertex table, so a corrupt
  index panicked `Collider::trimesh`/`compute_normals` or silently
  mis-shaped the mesh. `PRIMTYPE_TRIANGLES` strips (the only kind
  observed on retail and the only one consumers interpret) are now
  range-checked; a bad index fails the chunk, which degrades to the
  documented `PkgChunk::Raw` preserve — now logged via `tracing::warn!`
  with the parse error, and counted as `partial` by `mm2-inspect scan`.
- **Findings 4, 7, 9** — the validate-less tune records (`vehCarSim`,
  `vehTrailer`, `aiVehicleData`, `asNode`) read scalars/vec3s verbatim:
  `SteeringLimit nan` reached `f32::clamp` as a NaN bound (panic in both
  profiles), `AutoNumGears 1e12`/`inf` saturated `as u32` into a ~17 GB
  `Vec::with_capacity`, and `aiVehicleData.Size` NaN poisoned the
  traffic `CenterOfMass` fallback. New finite readers in `veh.rs` —
  `req_finite_f32`, `opt_finite_f32`, `req_finite_vec3`,
  `opt_finite_vec3` — decode-error or warn-and-fall-back on non-finite
  values; `MAX_GEARS = 32` + `gear_count` bound both gear counts at
  decode, naming the authored value in the error. The
  `vehCarDamage`/`vehStuck`/`vehGyro` records deliberately keep verbatim
  readers — their `validate()` reports non-finite values — and
  `aiVehicleData.MaxAng` keeps `opt_vec3` so retail `va_garbagetruck`'s
  authored NaN is still preserved.
- `docs/research/authored-numbers.md` — per-finding `Status: fixed`
  lines plus the boundary refinement rationale.

Deviation from the audit's suggested shape for finding 7: the
plausibility bound sits in `veh.rs` decode rather than `convert()` —
the decode boundary reports the authored value (not a saturated
`u32::MAX`) and covers `ManualNumGears`'s identical cast for free.

## Evidence

Synthetic tests (new legs in parentheses):

- `mm2_app/tests/opponents.rs` — `reanchor_pose_bounds_a_collapsed_closed_route`,
  `reanchor_pose_bounds_a_nonfinite_route` (+2; suite 47/47).
- `mm2_content/tests/opponents.rs` — `a_degenerate_route_is_reported_not_wired` (+1; 10/10).
- `mm2_game/tests/nav.rs` — `a_dangling_intersection_road_is_an_issue_not_a_panic` (+1; 35/35).
- `mm2_formats::pkg` — `out_of_range_triangle_indices_degrade_to_raw` (+1; 7/7).
- `mm2_formats/tests/vehicle_formats.rs` —
  `vehcarsim_rejects_non_finite_scalars`, `vehcarsim_bounds_gear_counts`,
  `vehcarsim_vec3_fields_must_be_finite`,
  `aivehicledata_rejects_non_finite_scalars` (+4; 24/24).

Retail audits (`fnv1a64:e91e6cd4b2ae30d9`, read-only install, this
tree's `mm2-inspect`):

- `scan` — **zero** "geometry chunk failed to parse" warnings: no retail
  PKG carries an out-of-range triangle index; the ~33 pre-existing
  `partial` entries are unchanged (non-geometry raw chunks).
- `validate-cars` — 21/21 stock vehicles ok, same warning set as before
  (hitch fallbacks, paint-count mismatch, engine-sample row counts).
- `handling` — all 21 vehicles within the arcade envelope, unchanged.
- `traffic` — 23/23 + 23/23 ambient `aivehicledata` decode on both
  cities, including `va_garbagetruck`'s preserved `MaxAng` NaN.
- `nav` / `opponents` — same pre-existing issue counts; zero new
  `DegenerateRoute` or `DanglingIntersectionRoad` findings.

## Gates

`cargo fmt --all -- --check`, `cargo clippy --locked --workspace
--all-targets --all-features -- -D warnings`, `cargo test --locked
--workspace` — all green on the committed tree (one navarrow test
expectation shifted to `no-geometry` after the parse-level check
moved first; the `bad-index` rasterizer guard stays as
defence-in-depth).

## Classification

Implementation choice throughout — every change hardens malformed-input
handling and makes no original-behavior claim. Findings 1/2/3 had doc
comments claiming bounded/diagnostic behavior that the code did not
deliver; the fixes make those claims true. The gear bound (32 vs retail
≤ 6) and step cap (16,384) are designed limits, documented as such.

## Remaining open items

- Audit findings 5, 6, 8, 10, 11 remain open (`authored-numbers.md`):
  non-finite race start slots (`RaceDefinition::validate` gap), the
  `props.rs:796` `+1` overflow on a saturated prop-offset cast, the
  `effects.rs` `end - start + 1` flipbook overflow, the `crashdata.rs`
  integer-via-`f32` columns, and `CameraFOV` range. Each is diagnosed
  in the doc with a suggested fix shape.
- Everything here is candidate-level: unit/synthetic evidence plus
  retail audit runs, pending external gate + review.

# Last iteration — F19-A.4 `.mod` skin assembly + pose-driven deform
# (iteration 92)

Iteration 92 on `ralph/night` (baseline `fb81714` — the F19-A.3
review-repair commit; external verify + review green; thirty-seventh
iteration of run `20260925T144723`). One coherent slice: the missing
link between the parsed `.mod` meshes and the sampled poses —
assembling `pedmodel_*.mod` geometry against the rig and deforming it
over sampled world transforms, plus the audit legs that exercise both
on retail. This is the domain-type leg of AC02's "assembled meshes
over the sampled poses"; rendered output still does not exist.

## Task selection

No failing gate or review finding to repair — the F19-A.3 repair
passed external review with zero blocking findings. The plan's F19-A
row names AC02's need for "assembled meshes over the sampled poses,
not just domain types" as the remaining non-runtime leg, so this
iteration is A.4. It required recovering what A.2 left unknown: the
flat-dialect adjunct→bone binding and the vertex coordinate frame.

## What landed

- Format recovery (measured on retail `fnv1a64:e91e6cd4b2ae30d9` via
  `mm2-inspect dump` + a Python bind-pose reconstruction):
  - `.mod` `v` rows are **bone-local**: every authored vertex lies
    within ~0.55 m of the origin while the rig stands ~1.15–1.8 m
    tall; a model-space reading would need inverse-bind matrices the
    format does not carry.
  - `T_world(bone) · v` at the bind pose reassembles each mesh as a
    feet-on-the-ground standing figure — man y ≈ 0–2.0 (ankles ~0.01,
    head ~1.82), woman y ≈ 0–1.87 — so rigid skinning applies the
    posed bone transform directly (AGE `crModel`/`crBone`
    convention).
  - The vertex→bone map is the `mtxv` per-matrix contiguous count row
    over the `v` array in `.skel` pre-order — the flat dialect's only
    binding record. Packet `adj` slots resolve through their packet's
    `mtx` list to the same bone, and `mtxn` partitions normals the
    same way: both records agree on all 1946 retail adjuncts
    (man 248, manw 279, woman 696, womanw 723 — script-verified).
- `mm2_game::ped`:
  - `PedSkin::from_mod(m, rig)` — assembles both dialects into
    corner-indexed geometry: `PedCorner` (bone + bone-local
    pos/normal + colour/UVs, authored order — flat adjuncts then each
    packet's), `PedSkinMtl` (authored shading fields + a contiguous
    slice of the triangle list), `orphan_tris` for primitives no
    material group claims. Packet adjuncts bind via their `mtx` slot
    (`mtxv` fallback), flat adjuncts via `mtxv` alone. Out-of-range
    vert/normal/matrix-slot/bone indices are `PedSkinError`s;
    oob-triangle drops, strip primitives (winding unrecovered,
    UNK-41), out-of-range colour/tex indices, orphan primitives and
    `mtxn`-bucket disagreements are recorded `issues` — nothing is
    silently reshaped.
  - `PedSkin::deform(world)` — rigid skinning over the sampled world
    transforms (`pos' = t + r·v`, `n' = r·n`); `PedDeformError` on a
    short transform slice or non-finite bone transform.
- `mm2_formats::ped` — `PedMod::validate` gained the `mtxn`↔binding
  agreement cross-check (issue when a normal's `mtxn` bucket differs
  from its corner's bone).
- `mm2-inspect peds` — assembles every parsed `.mod` against its own
  rig (`skins` report field), deforms at the bind pose (must be
  finite and pass a plausible-standing-figure y-range check), then
  deforms at every sampled state-window pose (`skin_samples` field).
  Assembly errors, non-finite output and implausible bind shapes are
  all issues — `--strict` fails on them.
- `docs/research/pedanim.md` — the `.mod` section records the
  bone-local vertex measurement and the `mtxv`/`mtxn` binding
  semantics; the flat-dialect binding item leaves UNK-41.
- `docs/original-rules.md` — PED-1 records the recovered `.mod`
  skinning under `verified_original`.

## Evidence

- `cargo test --locked -p mm2_game ped` — 20/20 incl. 7 new:
  flat-dialect assembly + bind deform, packet-dialect assembly + bind
  deform, rotated-bone corner sweep (parent rotation swings the
  corner about the bone), invalid matrix-slot/bone/unbound-vertex
  errors, non-finite transform + short-input `deform` errors, `mtxn`
  disagreement recorded as an issue.
- `cargo test --locked -p mm2_inspect peds` — 8/8 incl. 2 new:
  `audit_assembles_and_deforms_skins` (packet + flat fixtures through
  the full audit — 14 samples, zero issues — plus a bone-9 `mtx`
  entry surfacing as an assembly issue) and
  `audit_flags_a_non_standing_bind_shape` (a vertex far above the
  skeleton → "not a plausible standing figure" issue).
- Retail (`fnv1a64:e91e6cd4b2ae30d9`): `mm2-inspect peds --strict`
  exits 0 — `skins: 4 assembled, 292 deform samples` (4 bind-pose +
  288 window-pose), all four `.mod` meshes assemble and deform to
  finite plausible geometry over every authored window; quirk list
  unchanged.

## Gates

- `cargo fmt --all -- --check` — clean.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — clean.
- `cargo test --locked --workspace` — all suites green (exit 0).

## Classification / remaining open items

- Recovered (measured, verified_original): bone-local `.mod` verts,
  `mtxv`/`mtxn` contiguous bone partitions, packet `mtx`↔`mtxv`
  agreement, direct-transform rigid skinning. Still UNK-41:
  `mtxv`-vs-`mtx` authority (moot on retail — they agree), `stp`
  rows, the optional 4th `packet` int, strip winding (strips are
  counted-not-expanded — none exist on retail), `.rays`/`.remap`/
  `motionHint`, window inclusivity, stepping/blend timing.
- Domain-types slice only: no Bevy mesh/skinning path, no spawning,
  no nav/reaction/audio/reset — F19-A stays `active`; F19-AC02..AC06
  remain unclaimed (AC02's rendered-evidence leg in particular — the
  deform is verified geometrically on synthetic fixtures and through
  the retail audit's finiteness/shape checks, not on screen).
- F19-A.2 review minors still open: duplicate-row overwrites, `mtl`
  integer-field degradation, `prim_check` line 0, `tangents:`
  assumption.

---

# Last iteration — F19-A.3 review repair: zero-frame clip + hostile
# window bounds (iteration 91)

Iteration 91 on `ralph/night` (baseline `2fb4157` — the F19-A.3
commit; external verify green, the review returned two blocking
findings; thirty-sixth iteration of run `20260925T144723`). One
scoped repair: the new pose-sampling code could panic or silently
mis-clamp on grammar-valid hostile input — a `frames=0` clip
referenced by a state row, and csv-authored `i64` frame extremes.

## Task selection

The F19-A.3 candidate failed external review on two blocking
findings, both in the iteration's own new code:

1. `tools/mm2_inspect/src/peds.rs` — the sampling leg computed
   `hi = clip.frames as i64 - 1`, so a parsed-but-empty clip
   (`frames=0`, which `PedAnim::parse` accepts and `validate()`
   already reports as `EmptyClip`) reached `clamp(0, -1)` and panicked
   `min > max`, aborting the whole audit. The reviewer reproduced it
   on the candidate build with a synthetic install (exit 101).
2. `peds.rs` (`st.first_frame - 1`, `st.last_frame - 1`) and
   `PedAnimator::new` in `mm2_game::ped` (`(s.first_frame - 1).max(0)
   as u32`) — rebasing the unbounded authored `i64` fields subtracts
   1 (overflow-panic on `i64::MIN` in debug builds) and the `as u32`
   cast truncates authored values past `u32::MAX` into
   wrong-but-in-range windows.

Repairing both was this iteration's only work.

## Findings and actions

- `peds.rs` — the sampling leg now guards on
  `clip.frames.checked_sub(1)`: a zero-frame clip skips sampling
  entirely (its `EmptyClip` is already an issue from `validate()`),
  and the authored window fields saturate via `saturating_sub(1)`
  before clamping into `[0, hi]`.
- `mm2_game::ped` — new `authored_window_frame()` helper:
  `saturating_sub(1).clamp(0, u32::MAX as i64) as u32`, so
  `i64::MIN`/`i64::MAX` csv rows saturate to `0`/`u32::MAX` instead of
  overflowing or truncating (`PedStates::validate` already reports
  such rows as issues).
- Regression tests:
  - `peds.rs::audit_degrades_zero_frame_clips_and_extreme_windows` —
    synthetic install with a `frames=0` clip referenced by a state
    row plus an `i64::MIN..i64::MAX` window row: audit completes,
    `zero frames`/`outside 1..=`/`exceeds clip frames` issues reported,
    `poses_sampled` still counts the two good windows.
  - `ped.rs::animator_saturates_hostile_frame_windows` —
    `PedAnimator::new` over `i64::MIN`/`i64::MAX` fields: window
    saturates to `0..=u32::MAX`, construction never panics, ticking
    clamps the window against the real clip length.
- Binary-level repro of the reviewer's case: synthetic install with
  `anim/pedanim_xzero.anim` (frames=0, fpf=12) + a state row
  referencing it + an `i64::MIN..i64::MAX` row →
  `mm2-inspect peds <dir> --strict` exits 2 with the EmptyClip and
  window issues enumerated (was: exit 101 panic at peds.rs:397).

## Evidence

- `cargo test -p mm2_game ped` — 13/13 incl. the new regression test.
- `cargo test -p mm2_inspect peds` — 6/6 incl. the new regression
  test.
- Retail (`fnv1a64:e91e6cd4b2ae30d9`): `mm2-inspect peds --strict`
  exits 0 — output identical to the F19-A.3 run (91 files, 66 clips,
  1342 frames, 288 pose samples, 18 unreferenced, quirk list
  unchanged).

## Gates

- `cargo fmt --all -- --check` — clean.
- `cargo clippy --workspace --all-targets --all-features --
  -D warnings` — clean.
- `cargo test --locked --workspace` — all suites green (exit 0).

## Classification / remaining open items

- No original-rule claim changes; both fixes are robustness bounds on
  hostile-but-grammar-valid input, not recovered rules.
- All F19-A.3 open items stand: domain-types slice only — no `.mod`
  geometry assembly, skinning, spawning; F19-A stays `active`;
  F19-AC02..AC06 remain unclaimed. UNK-41 stepping/blend timing still
  unrecovered.

---

# Last iteration — F19-A.3 pedestrian animation sampling + authored
# state stepping (iteration 90)

Iteration 90 on `ralph/night` (baseline `c0011ee` — the F19-A.2
review-repair commit; external verify + review green; thirty-fifth
iteration of run `20260925T144723`). One coherent slice: F19-A req 2's
domain leg — `.anim` clip sampling onto the skeleton and stepping the
authored csv state machine, both as reusable `mm2_game` domain types
with synthetic-fixture tests. Runtime assembly/spawning stays deferred.

## Task selection

No failing gate or review finding to repair — F19-A.2's repair passed
external review with zero blocking findings. The plan's F19-A row named
"req 2 — animation sampling/blending, authored-state stepping,
synthetic skeletal fixtures" as the remaining non-runtime leg, so this
iteration is A.3. It required recovering what A.1 left unknown: the
`.anim` channel layout and the rotation convention.

## What landed

- Format recovery (measured on retail, cross-checked against R3's
  `Pedestrian_animations.md` and mm2hook's `crAnimFrame`/`crBone`/
  `Matrix34` sources):
  - `.anim` frame = channel 0 root **world translation** (stands ~1.147
    m in idle; its −Z drift equals the state row's `Y AXIS DISTANCE`
    to ~1 mm on man walk/run) + one Euler rotation triple per bone in
    `.skel` pre-order — verified by the standing pose's mirrored L/R
    values landing on the `clavicle/shoulder/elbow/wrist_{r,l}` pairs.
  - Euler composition is the AGE `Matrix34` order `Rx·Ry·Rz`
    (`GetEulers` extracts exactly that product) = glam's
    `EulerRot::XYZEx`. Under this order the dive clips' end poses land
    prone along the dive direction; intrinsic-XYZ puts them
    perpendicular. The Blender importer's conversion is a Z-up fudge —
    not copied.
  - csv `* OFFSET`/`* DISTANCE` columns are forward/lateral per-window
    travel bookkeeping — chained rows accumulate (`0.281 + 1.409 →
    WALK_STAND` 1.69; dive chains carry ±2.2 m lateral). mm2hook names
    them `pedAnimationSequence.FSpeed`/`LSpeed`.
- `mm2_game::ped` (new module — domain types, no ECS):
  - `PedRig::from_skel` — flattens the hierarchy pre-order (the channel
    order); `PedRigError` on empty/multi-root rigs.
  - `PedRig::sample(clip, frame)` — fractional frame, lerping raw
    channel floats (mm2hook `crAnimFrame::Blend` shape), clamped into
    `0..frames`; `PedSampleError` on empty clips/ragged-or-narrow
    channel widths (trailing extras are tolerated).
  - `PedRig::world_transforms` — FK over bind offsets + clip rotations.
  - `PedPose::lerp` — translation lerp + slerp pose blending.
  - `PedAnimator` — the authored state machine: 1-based authored
    windows → 0-based indices, `last_frame` clamped against the actual
    clip (the authored `frames+1` overshoot rows are honoured, not
    out-of-bounds), `default next` chains by name, self-loops wrap
    keeping sub-frame phase, `request(target)` enters the authored
    `{CUR}_{TGT}` transition state at its first frame or switches
    directly when none exists (designed — DSN-64), unknown targets are
    refused, non-finite/`<=0` dt is inert, a per-state guard bounds
    degenerate empty-window chains.
  - `PED_STATE_FPS` = 30 — designed default; the original's stepping
    rate is unrecovered (UNK-41).
- `mm2-inspect peds` — exercises the production sampler over every
  authored state window (first/mid/clamped-last frames): poses must be
  finite, counted into a new `pose samples` report field; sampler
  errors and non-finite poses are issues. Rig-construction failures on
  parsed `.skel`s are issues too.
- `docs/research/pedanim.md` — the `.anim` section now records the
  recovered layout/Euler convention/evidence; the csv section records
  the measured offset/distance semantics; cross-checks list the
  pose-sampling leg.
- `docs/original-rules.md` — PED-1 narrowed (layout + columns
  recovered), UNK-41 narrowed (`.rays`/`.remap`/`motionHint` quantity,
  window inclusivity and stepping/blend timing remain open), DSN-64
  records the designed playback policies.
- `mm2_formats::ped` doc comments updated — no parser behaviour change.

## Evidence

- `cargo test -p mm2_game ped` — 12/12 new tests green: pre-order
  flatten + bind-pose FK accumulation, channel→bone mapping (a rotated
  parent swings its child's world offset), the fixed-axis XYZ Euler
  order pinned against `Rz·Ry·Rx`, fractional-frame channel lerp,
  frame clamping + empty/narrow/wide clip errors, window stepping +
  self-loop wrap with phase, `{CUR}_{TGT}` transition routing, direct
  switch without an authored transition, the `frames+1` overshoot
  clamp, unknown-target/idle-dt refusal, constructor errors, pose
  lerp.
- `cargo test -p mm2_inspect peds` — 5/5 incl. the new legs
  (`poses_sampled == 6` on the fixture, NaN channel → `non-finite
  pose` issue).
- Retail (`fnv1a64:e91e6cd4b2ae30d9`): `mm2-inspect peds --strict`
  exits 0 — 66 clips, 288 pose samples across the 96 authored windows
  (4 rigs × 24 states), zero issues, quirk list unchanged.

## Gates

- `cargo fmt --all -- --check` — clean.
- `cargo clippy --workspace --all-targets --all-features --
  -D warnings` — clean.
- `cargo test --workspace` — all suites green (exit 0).

## Classification / remaining open items

- Recovered (measured, verified_original): the `.anim` channel layout,
  the `Rx·Ry·Rz` Euler composition, the csv travel columns. Designed
  (DSN-64): `PED_STATE_FPS`, immediate-request transition policy,
  `frames+1` clamping, phase-preserving loop wrap.
- Still UNK-41: `.rays`/`.remap` semantics, `motionHint`'s exact
  quantity, whether authored windows are inclusive (the +1 rows),
  the original's stepping rate/interruption/blend timing.
- Domain-types slice only: no `.mod` geometry assembly, no skinning,
  no Bevy entity/mesh path, no spawning — F19-A stays `active`;
  F19-AC02..AC06 remain unclaimed. Sampling tests verify transforms
  on independent synthetic fixtures, not rendered output.
- F19-A.2 review minors still open: duplicate-row overwrites, `mtl`
  integer-field degradation, `prim_check` line 0, `tangents:`
  assumption.

---

# Last iteration — F19-A.2 review repair: `PedMod` carve-range panic
# (iteration 89)

Iteration 89 on `ralph/night` (baseline `f8a4b23` — the F19-A.2
commit; external verify green, the review returned one blocking
finding; thirty-fourth iteration of run `20260925T144723`). One
scoped repair: `PedMod::validate()` could panic on a packet-dialect
`.mod` whose material `packets:` counts overrun the actual packet
list.

## Task selection

The F19-A.2 candidate `f8a4b23` failed external review on one
blocking finding: the per-material range carving advanced `pkt_at`
by the declared `packets:` count but clamped only the range *end* —
once `pkt_at` exceeded `packets.len()`, a later material's
`packet_range` came out inverted (start > end) and
`self.packets[mtl.packet_range.clone()]` in `validate()` panicked
(`range start index 5 out of range for slice of length 1` on the
reviewer's two-`mtl` repro). `mm2-inspect peds` calls `validate()`
per `.mod`, so one malformed or modded mesh would abort the whole
audit instead of counting as an issue — and the iteration's "no
panic path" claim was false. Repairing that defect was this
iteration's only work.

## Findings and actions

- `mm2_formats::ped` — the carve now clamps *both* range ends:
  `adj_at.min(len)..end.min(len)` for `adjunct_range`,
  `primitive_range` and `packet_range`. The consumption cursors
  still advance by the declared counts, so downstream materials get
  empty clamped ranges rather than overlapping ones, and the
  existing `claimed_*`-vs-actual coverage checks report the
  overrun exactly as before.
- Regression test `mod_validate_reports_overdeclared_material_counts`
  reproduces the reviewer's shape in both dialects: packet — `mtl A`
  declares `packets: 5` against 2 real packets (`validate()` reports
  "materials claim 6 packets but 2 exist", no panic); flat — `mtl A`
  declares `adjuncts: 9`/`primitives: 9` against 4/2 records
  (`claim 10 … but 4/2 exist`, empty `4..4`/`2..2` carve for `mtl
  B`).

## Evidence

- `cargo test -p mm2_formats ped` — 32 green incl. the new
  regression test.
- `mm2-inspect peds <retail> --strict` — exit 0, output identical to
  the F19-A.2 run (man 117v/230p/18m packets, manw 128v/260p/17m,
  woman 123v/232p/17m flat, womanw 127v/241p/16m, wolf quirk
  intact).

## Gates

- `cargo fmt --all -- --check` — clean.
- `cargo clippy --workspace --all-targets --all-features --
  -D warnings` — clean.
- `cargo test --workspace` — all suites green (exit 0).

## Classification / remaining open items

- No original-rule claim changes; the clamp is a robustness bound on
  malformed declared counts, not a recovered rule.
- Review minors not addressed this iteration (non-blocking, recorded
  for a future pass): silent overwrites on duplicate `mtxv`/`mtxn`/
  `mtx`/`illum`/`textures` rows, silently-`None` malformed `mtl`
  integer fields, `prim_check` reporting line 0, the `tangents:`
  split-vs-sum assumption (documented as inferred).
- All other F19-A.2 open items stand: parser + audit slice only —
  no geometry assembly, skinning or spawning; F19-A stays `active`;
  F19-AC02..AC06 remain unclaimed.

---

# Last iteration — F19-A.2 `.mod` pedestrian mesh decode (iteration 88)

Iteration 88 on `ralph/night` (baseline `e01d429` — the F19-A.1 commit;
external verify + review green; thirty-third iteration of run
`20260925T144723`). One coherent slice: the `pedmodel_*.mod` ASCII
skinned meshes are decoded, validated and cross-checked through the
same audit path; runtime consumption stays open.

## Task selection

No failing gate or review finding to repair — F19-A.1 passed external
review with zero blocking findings. The plan's F19-A row named `.mod`
mesh decode as A.2, the next leg. Direct measurement plus the R3
`Pedestrian_model.md` reference recovered two retail dialects, so this
iteration adds the parser, folds it into `mm2-inspect peds`, and
records the measured invariants. Geometry assembly, skinning and
spawning are deliberately deferred.

## What landed

- `mm2_formats::ped::PedMod` + `PedModDialect` (pure parser): the
  `version:` header, ten declared counts, `v`/`n`/`c`/`t1`/`t2`/`ts`/
  `tt` resource lists, `mtl <name> { … }` shader groups
  (`packets:`/`adjuncts:`/`primitives:`/`textures:`/`texture:`/
  `illum:`/`ambient`/`diffuse`/`specular`), `packet { adj tri mtx }`
  blocks, flat `adj`/`tri` lists, and `mtxv`/`mtxn` matrix-count
  trailers. Two dialects: **packet** (`pedmodel_man`/`manw` — six-field
  adjuncts whose last field indexes the packet's own `mtx` bone list)
  and **flat** (`pedmodel_woman`/`womanw` — five-field adjuncts in one
  global list partitioned to materials by declared count). Unknown
  records, non-integer fields, unclosed blocks and truncated input all
  degrade to `TableDiagnostic`s — no panic path.
- `PedMod::validate()` — every declared header count vs actual,
  per-packet and per-material declared counts vs owned data, adjunct
  vertex/normal/colour/uv index bounds (empty lists accept index 0 —
  retail `tex2s: 0` shape), `tri` index bounds, packet `mtx` entries
  vs `matrices:`, adjunct matrix-slot bounds, `mtxv`/`mtxn` entry
  counts vs `matrices:` and partition sums vs verts/normals, material
  ownership coverage of the shared lists (no orphaned packets or
  adjuncts), mixed-dialect files, unknown `illum` values, texture-row
  counts, and two measured retail invariants: `adjuncts:` ==
  `normals:` == distinct (vertex, normal) tuples, and packet
  adjunct→bone bindings agreeing with the `mtxv` vertex partition.
- `mm2-inspect peds` — every discovered `.mod` is deep-parsed; the
  archetype line reports `.mod <verts>v/<prims>p/<materials>m
  (packets|flat)`; diagnostics and validate issues count as issues,
  parse failures as failures; cross-checks `matrices:` vs the parsed
  skeleton's `NumBones` and `mtl` count vs `.shaders`
  shaders-per-paint-job.
- `mm2-inspect inventory` note and `docs/research/pedanim.md` updated
  — the `.mod` section now records the grammar, both dialects and the
  measured invariants instead of "inventoried, not decoded".

## Evidence

- `cargo test -p mm2_formats ped` — 31 green incl. 6 new `PedMod`
  tests (packet + flat dialect fixtures, missing version rejection,
  malformed-record diagnostics, index/partition validation, packet
  declared-count/slot/`mtxv`-agreement errors, mixed dialect).
- `cargo test -p mm2_inspect peds` — 5 green incl. a new audit test
  (`.mod` field extraction, skeleton/matrix mismatch → issue,
  unparseable mesh → failure, material/shader-count gap → issue).
- Retail (`fnv1a64:e91e6cd4b2ae30d9`): `mm2-inspect peds` — all four
  `.mod` files parse with zero issues: `pedmodel_man` 117v/230p/18m
  (packets), `pedmodel_manw` 128v/260p/17m, `pedmodel_woman`
  123v/232p/17m (flat), `pedmodel_womanw` 127v/241p/16m. The rest of
  the report is unchanged (91 files, 66 clips, wolf quirk, authored
  `frames+1` quirks); `--strict` exits 0.

## Gates

- `cargo fmt --all -- --check` — clean.
- `cargo clippy --workspace --all-targets --all-features --
  -D warnings` — clean.
- `cargo test --workspace` — all suites green (exit 0).

## Classification / remaining open items

- Original-data claim: grammar + invariants measured against the named
  fingerprint (PED-1); the flat-dialect bone-binding mechanism,
  `mtxv`-vs-`mtx` authority, `stp` rows and the optional 4th packet
  int stay UNK-41.
- Parser + audit slice only: no geometry assembly, no skinning, no
  sampling, no spawning — F19-A stays `active`; F19-AC02..AC06 remain
  unclaimed. A clean audit is not evidence of runtime pedestrian
  fidelity.

---

# Last iteration — F19-A.1 pedestrian-rig definition parsers +
# `mm2-inspect peds` (iteration 87)

Iteration 87 on `ralph/night` (baseline `4353f1e` — the F18-B.5
review-repair docs commit; external verify + review green; thirty-second
iteration of run `20260925T144723`). One coherent slice: the `anim/`
pedestrian corpus's definition-side formats are recovered, parsed and
cross-checked through the VFS; runtime import stays open.

## Task selection

No failing gate or review finding to repair — F18-B.5 passed external
review with zero blocking findings. From the ready set, F19-A was the
highest-value unblocked feature (deps F00-B/F01-B/F09-B all satisfied;
pedestrians are the largest untouched single-player content family, and
F17-A's deferred ped-density consumer rides on it). F19-A is broad, so
it is split — this iteration is A.1, the audit-first definition slice
(the same shape as F10-A.1/F21-A.1/F07-A.1): recover and parse the
`.skel`/`.csv`/`.remap`/`.rays`/`.anim`/`.shaders` grammars, cross-check
them, report the corpus honestly. `.mod` mesh decode, clip sampling and
any runtime spawning are deliberately deferred to A.2+.

## What landed

- `mm2_formats::ped` (new module, pure parsers):
  - `PedSkel` — `NumBones <n>` header + recursive
    `bone <name> { offset x y z … }` tree; malformed directives, bad
    offsets, unclosed/unbalanced blocks all degrade to
    `TableDiagnostic`; `validate()` reports declared-vs-actual count
    mismatches, duplicate bone names and non-finite offsets. Bone depth
    is bounded (64).
  - `PedStates` — the `pedmodel_*.csv` 9-cell state model (`#` comments
    skipped); `validate()` flags duplicate state names, invalid frame
    windows, dangling `next` links and non-finite floats.
  - `PedRemap` — count + whitespace-separated indices (the single
    retail file ships 17); count mismatches and negative indices
    validate.
  - `PedRays` — count, `count` `f3 + i2` rows, then the integer grid
    (width checked against the count); semantics unknown, preserved.
  - `PedAnim` — strict binary grammar measured byte-exact on all 66
    retail clips: `u32 reserved, u32 frames, u32 floatsPerFrame,
    f32 motionHint, u8 kind` then `frames × fpf` LE f32 samples;
    `frames × fpf` is bounded (1M floats) so a hostile header cannot
    force a huge allocation; trailing bytes are an error. `validate()`
    reports unexpected reserved/kind, empty clips, non-multiple-of-3
    frame sizes and the first non-finite sample.
- `mm2_formats::pkg::PkgShaders::parse` — standalone `.shaders` files
  reuse the existing PKG shader-chunk parser (measured: the four retail
  files are exactly that grammar, float shaders, empty texture names),
  plus a strict trailing-byte check.
- `mm2-inspect peds <install> [--strict]` — censuses `anim/`,
  deep-parses every archetype member, and cross-checks: state-model
  clip stems resolve to discovered `.anim` files, authored frame
  windows vs clip length (`frames + 1` overshoots report as authored
  quirks — 36 rows on retail, never more), clip `floatsPerFrame ==
  3 × (bones + 1)`, `.rays` count vs `NumBones`, remap validity, and
  the EXPECTED_PEDS roster. Partial/extra archetypes (`pedmodel_wolf`)
  and authored misfits (the ASCII scene lists `pedanim_manantrnch.anim`
  and the extensionless `anim/pedmodel_woman`, `grog.bat`, `anim/cvs/*`)
  are reported, not failed. `--strict` exits nonzero on failures,
  issues and missing expected archetypes — quirks stay non-fatal.
- `mm2-inspect inventory` pedestrian note updated: the definition-side
  formats are now parsed by `peds`; `.mod` remains undecoded, and
  records stay `unverified` until a runtime consumer exists.
- `docs/research/pedanim.md` (new) — measured grammars, corpus census,
  cross-check results. Ledger: PED-1 (verified_original corpus/grammar
  facts) + UNK-41 (`.rays`/`.remap`/`.anim` channel semantics,
  `motionHint`, csv offset/distance columns, window inclusivity,
  transition timing — all unrecovered).

## Evidence

- `cargo test -p mm2_formats` — 230 green incl. 15 new `ped` tests +
  the standalone `PkgShaders` test (synthetic fixtures; malformed,
  truncated, oversized and ragged inputs all covered).
- `cargo test -p mm2_inspect` — +4 `peds` tests over synthetic VFS
  installs (complete archetype, off-by-one window quirk, misfit clip,
  orphan clip, missing clip, channel-width mismatch, `.rays`/`.skel`
  count mismatch, truncated clip failure, nested/extra records).
- Retail (`fnv1a64:e91e6cd4b2ae30d9`): `mm2-inspect peds` — 91 `anim/`
  files, 66 clips parsed (1342 frames), 18 unreferenced reported,
  5 archetypes (4 complete 19-bone rigs with 24 states each, wolf
  partial → quirk), shaders byte-exact (48×18 / 24×17 / 48×17 /
  24×16), zero issues, zero failures; `--strict` exits 0.

## Gates

- `cargo fmt --all -- --check` — clean.
- `cargo clippy --workspace --all-targets --all-features --
  -D warnings` — clean.
- `cargo test --workspace` — all suites green (exit 0).

## Classification / remaining open items

- Original-data claim: grammar + corpus facts only (PED-1,
  verified_original against the named fingerprint). All runtime
  semantics are UNK-41 — nothing claims the original's channel order,
  transition timing or `.rays`/`.remap` purpose.
- This is a parser + audit slice: no Bevy assembly, no sampling, no
  spawning — F19-A stays `active` (`.mod` decode, sampling/blending,
  bind verification, pause/unload legs all open). F19-B (movement,
  density, reactions) and F17-A's ped-density consumer remain blocked
  on it.
- The `peds` audit does not fail strict on the two authored ASCII
  misfits — they are quirks; a genuinely broken binary clip still
  counts as a failure.

---

# Last iteration — F18-B.5 review repair: the cue-suffix overflow
# (iteration 86)

Iteration 86 on `ralph/night` (baseline `4d64682` — the F18-B.5 docs
commit; external verify green, the review returned one blocking
finding; thirty-first iteration of run `20260925T144723`). One scoped
repair: `draw_cue_suffix` could panic on authored `end`/`add` windows
overflowing `i64`.

## Task selection

The F18-B.5 candidate `4d64682` failed external review on one
blocking finding: `draw_cue_suffix` summed `add + 1 + rng % end` on
verbatim `i64` fields, so a modded or corrupt
`aud/spchdata/*_prerace.csv` authoring values near the `i64` edge
(`WEARAIN,3,9223372036854775807`) overflows — a panic under dev/test
overflow checks, a wrapped bogus stem in release — reachable through
`resolve_commentary` at session start, against the module's
diagnose-not-panic contract for authored data. Repairing that defect
was this iteration's only work.

## Findings and actions

- `mm2_game::audio` — `draw_cue_suffix` now returns `None` when
  `end <= 0` *or* the window top `add + end` overflows `i64` (with
  `end >= 1` the `checked_add` covers `add + 1` too, so the summed
  suffix is provably in range for every draw). The decline happens
  before the rng draw is consumed, so an undrawable row counts
  `failed` downstream exactly like a non-positive `end` and never
  shifts the seeded stream the drawable rows replay.
- `mm2_formats::spchdata` — `CueTable::validate` flags the same
  shape (`end > 0`, `add + end` overflow) as an advisory diagnostic,
  matching the draw's verdict the way the `end <= 0`/`add < 0` legs
  already do.

## Evidence

- `cargo test -p mm2_formats spchdata` — 11/11 incl. the new
  `validate_flags_an_unrepresentable_sufix_range`.
- `cargo test -p mm2_game audio` — 33/33 incl. the new
  `an_unrepresentable_cue_window_is_undrawable_not_a_panic`
  (i64-edge windows → `None` with no panic and no draw consumed; a
  representable `i64::MAX`-topping window still lands in-window).
- `cargo test -p mm2_app --test audio` — 86/86 incl. the new
  `an_overflowing_sufix_range_counts_failed_not_panics` (a
  `WEARAIN,3,<i64::MAX>` fixture through the production
  `resolve_commentary` path counts `failed` once and the time cue
  still plays — the reviewer's reachable panic path).
- Retail re-run (`fnv1a64:e91e6cd4b2ae30d9`, london headless
  `--weather 2 --time-of-day 3 --frames 1200`):
  `aud=0h/46v/0s/4l/4a/1r/8i/8c/1k/1g/20e/16n/1q+13d+1x` — the `al5`
  `WEAFOG` authored miss still counts `+1x`, `timenight` still plays
  (`1q`). The `+Nd` term is the ambient-voice bound count and scales
  with traffic exposure/run length — `13d` at 1200 frames vs the
  iteration-85 entry's `+2d` and audio.md's bare `+1x` are the same
  record shape at different run lengths, not a regression.

## Gates

- `cargo fmt --all -- --check` — clean.
- `cargo clippy --workspace --all-targets --all-features --
  -D warnings` — clean.
- `cargo test --workspace` — all suites green (exit 0).

## Classification / remaining open items

- No original-rule claim changes: the draw stays the designed
  `add + 1 + rng % end` (DSN-63); the overflow decline is a
  robustness bound on unrepresentable authored input, not a
  recovered rule.
- Review minors addressed: the PLAN inventory line now reads `538
  VFS entries — 526 CSVs + 12 speaker-dir entries` (measured via
  `mm2-inspect list`); the `1q+2d+1x`/`1q+1x` record difference is
  documented above as `+Nd` run-length variance. The
  validate-not-invoked cosmetic note stands — `resolve_commentary`
  reads the table's parse diagnostics directly; `validate` remains
  the audit-side advisory.
- All F18-B.5 open items stand: AC02/AC04/AC05 unclaimed, F18-B stays
  `active`.

---

# Prior iterations

Iteration 85 on `ralph/night` (baseline `c0517af` — the F18-B.4 docs
commit; external verify + review green; thirtieth iteration of run
`20260925T144723`). One coherent slice: the `aud/spchdata`
commentary grammar is recovered and its environmental
`WEATHER`/`TIMEOFDAY` pre-race cue families now bind and sequence
through the session audio path.

## Task selection

The plan's F18-B row named "`wearain` commentary cues" as the next
leg. Retail carries the full system: `aud/spchdata` ships 526 cue
CSVs (`al1..al6`, `as1/as2/as4/as5`, `ccs`, `ccl` speaker dirs plus
`sf`/`london` announcer registries) and the exe names the whole
binding — `aud\spchdata\as%d`/`\al%d`, `%s_prerace`, the
`WEATHER`/`TIMEOFDAY`/`PRERACE`/`FINALCHECKPOINT`/`RESULTS*`/
`UNLOCK*`/`CNR*`/`BULLSHIT` section headers, the
`weaclr`/`weacldy`/`weafog`/`wearain` + `timemorn`/`timenoon`/
`timeeve`/`timenight` prerace stems and `nospeech`. The cue grammar
(`<prefix>,<end>,<add>[,extra]` under `X header,,` sections → waves
`<speaker><prefix><NN>`) measures cleanly against the corpus — every
row fits `int,int[,int]` — so the environmental leg is a recovered
data binding; the draw shape/cadence are designed readings
(DSN-63, UNK-25).

## Findings and actions

- **`mm2_formats::spchdata`** (new) — `CueTable`/`CueSection`/
  `CueRow`/`AnnouncerIndex`: the `Name prefix/type header,end sufix
  value,sufix add value` column header, `X header,,` section
  markers, `<prefix>,<end>,<add>[,extra…]` rows with the C&R fourth
  column and `AL1\AL1ROBROB`-style qualified prefixes preserved;
  `sf.csv`/`london.csv` registries parse `Num announcers`/`prefix`
  (5/`AS`, 6/`AL`). Malformed rows, rows outside sections, missing
  headers, duplicate sections and non-positive/negative ranges all
  diagnose; `ccl/cc_cpoint_indexinfo.csv` (a third bare-index
  grammar) is diagnosed, not force-fit.
- **`mm2_game::audio`** — `prerace_weather_stem`/`prerace_tod_stem`
  (the exe-ordered selector→stem maps, matching the measured `.ltNN`
  grid — documented binding, not designed), `draw_speaker` (1-based
  over the authored count — the `as3` gap stays a real draw gap),
  `draw_cue_suffix` (`add + 1 + rng % end` — the designed reading;
  `add` is 0 on every live retail weather/time row),
  `cue_wave_stem` (flat: `<speaker><prefix><NN>`;
  separator-qualified prefixes name their own leaf — the C&R
  shape).
- **`mm2_app::audio`** — `CommentaryAudio` session resource (bound
  in `load_session_world` off the shared `effective_conditions`
  pick; dev worlds bind none), `CommentaryVoice`,
  `VoiceKind::Commentary`, `AudioReport.commentary`, and
  `commentary_voices` (Update after `drive_session`, both
  schedules): resolves the registry → speaker → `<stem>_prerace`
  table → section → first row → suffix → wave chain once on the
  first `Countdown`/`Playing` frame, then sequences the ≤2 decoded
  clips as `SessionEntity`-stamped `PlaybackMode::Despawn`
  one-shots — each after the prior clip's decoded duration +
  `COMMENTARY_GAP` 0.25 s (cadence designed). Every miss counts
  `failed` once, never retried, never substituted (F18-AC06); all
  draws ride one `COMMENTARY_DOMAIN`-separated `NavRng` off the
  session seed (req-5 deterministic leg). `PcmAudio::duration()`
  added for sequencing.
- **`session.rs`** — inserts `CommentaryAudio` beside
  `WeatherAudio`; teardown removes it, the entity sweep reclaims
  stamped voices.
- **`smoke.rs`** — `aud=` gains `/<n>q` only when a cue spawned;
  misses surface through shared `+Nx` — quiet records stay
  bit-identical.
- **`mm2_formats/src/lib.rs`, `mm2_game/src/lib.rs`** — module
  exposure only.

## Evidence

- `cargo test -p mm2_formats` — spchdata legs: weather/time
  sections, C&R qualified prefixes + fourth-column preservation,
  bare-index diagnostic, malformed/orphan/missing-header/
  undrawable-range diagnostics, both registries.
- `cargo test -p mm2_game` — stem maps, speaker/suffix draws,
  flat vs qualified `cue_wave_stem` (`al1robrob05` pin).
- `cargo test -p mm2_app --test audio` — 85/85 incl. the F18-B.5
  suite: dev-world none, city bind through the production
  `load_session_world` path, missing registry/table/wave counted
  once with no substitution, seeded replay, weather-before-time
  ordering, teardown sweep.
- Retail (`fnv1a64:e91e6cd4b2ae30d9`, headless):
  - sf `--weather 3 --time-of-day 1` → `aud=…/2q` — both
    environmental cues resolved and played.
  - london `--weather 2 --time-of-day 3` → `aud=…/1q+2d+1x` — the
    draw landed `al5`: its `weafog_prerace.csv` authors `WEAFOG`
    while the archive ships `al5weasfog01/02` — a genuine authored
    gap, counted once and never substituted; `timenight` still
    played. (Companion quirk verified: every `weacldy` table
    authors `WEACLD`; `as3` is a real draw gap inside SF's `5`.)
  - sf `checkpoint:0` event → `aud=…/2q` inside the countdown
    window on the event's authored clear-morning conditions.
  - `sunk=0` throughout — headless has no output device; the `q`
    counts prove resolve→spawn, not audibility.

## Gates

- `cargo fmt --all -- --check` — clean.
- `cargo clippy --workspace --all-targets --all-features --
  -D warnings` — clean.
- `cargo test --workspace` — all suites green.

## Classification / remaining open items

- Verified data: the cue-table/registry grammars, section names,
  stem lists, wave inventory, the exe's directory/format strings,
  the `as3` gap and the `al5` `weafog`/`weasfog` authored mismatch.
- Designed (DSN-63): the suffix-draw shape (`add + 1 + rng % end`),
  the 1-based speaker draw, weather-before-time sequencing,
  duration + 0.25 s cadence, `COMMENTARY_VOLUME` 1.0, the
  `COMMENTARY_DOMAIN` stream split.
- Unknown (UNK-25): the original's speaker-pick, `add`/fourth-column
  semantics, cue cadence, gap-draw behavior, and every
  non-prerace section's trigger (`PRERACE`/`RESULTS*`/`UNLOCK*`/
  `CNR*`/`BULLSHIT` — F08 scope).
- No audible-output or playtest evidence — headless proves
  resolve→spawn→despawn; F18-AC05's audio leg stays open.
- F18-B stays `active`: non-prerace cue sections, wetness
  presentation beyond particles, and weather-state replication
  (req 5 network leg → F24+) remain.

---

# Prior iterations

Iteration 84 on `ralph/night` (baseline `d26dc0d` — the F18-B.3 docs
commit; external verify + review green; twenty-ninth iteration of run
`20260925T144723`). One coherent slice: the `ptxindex`/`ptxthreshold`
leg of F18-B req 3 — authored surface materials now select up to two
wheel-particle effect channels that emit at grounded wheel contacts.

## Task selection

The plan's F18-B row named "surface-effect legs beyond the DSN-43 wet
table". `materials.mtl` authors `ptxindex`/`ptxthreshold` pairs on all
eight materials with no consumer (UNK-23); the exe carries a
contiguous `dirt,dust,grass,leaf,smoke,snow,splash,rock` string block
immediately after the `ptx_wheel` atlas name plus a `tune/effects`
directory string — retail ships all eight `tune/effects/*.asbirthrule`
rules and a measured 8×8-tile `ptx_wheel` sheet. The index space is
thus recovered data (every authored pair lands coherently:
`water` `-1 6` → splash, `grass` `1 2` → dust+grass); the trigger
quantity/cadence are not, so the runtime is a designed reading
(DSN-62, UNK-23 stands).

## Findings and actions

- **`mm2_formats::banger`** — `StandaloneBirthRule` now captures the
  effects-file superset fields `Damp`/`DampVar`/`Height`/`Intensity`/
  `Color` (previously warned-and-discarded; `Color` authors a packed
  decimal word — `smoke` `-251989786`, `splash` `-331546`, `-1` =
  opaque white elsewhere). The standalone `known` list covers them;
  embedded `dgBangerData` decode is unchanged.
- **`mm2_formats::materials`** — `MaterialDef::ptx()` →
  `PtxChannels{index[2],threshold[2]}`: integral indexes and finite
  thresholds; `validate()` rejects the same malformed shapes the
  accessor refuses so the two can never disagree.
- **`mm2_content::surface`** — `SurfaceTables::ptx_channels(material)`:
  authored indexes read their own def, `SurfaceMaterial::Unspecified`
  reads `_default`, unresolvable → `None`.
- **`mm2_game::effects`** — `PTX_RULE_NAMES` (the recovered table),
  `PTX_ATLAS_TILES` 8, `WheelPtxPolicy{max_live:128}` (F18-AC03),
  `WheelPtx`/`WheelChannels`/`WheelDraw`/`WheelEmission`/`WheelPuff`.
  Per-(vehicle, wheel) `NavRng` streams domain-separated by
  `WHEEL_PTX_DOMAIN` — deterministic per session seed (req 5's leg).
  Each channel gates on the wheel's `tire_slippage` utilization — the
  same measure skid audio reads — strict `>` vs `ptxthreshold`, so
  `water`'s authored `0 0` still demands nonzero tire work (a parked
  wheel stays dark). `InitialBlast` credits on each rising gate edge
  (reground re-fires it), `SpewRate` accumulates inside
  `SpewTimeLimit`, surface change rebinds, airborne closes gates.
  `Damp`/`Height` ride the spec unconsumed (semantics unrecovered).
- **`mm2_app::wheel_fx`** — session bind resolves all eight
  `tune/effects/<name>.asbirthrule` rules through the VFS into
  `ParticleSpec`s (each miss counts `WheelFxReport.failed` once, stays
  dark, never substituted — F18-AC06) and builds the `ptx_wheel`
  sprite quads (missing atlas → `+ut`, untextured emission continues).
  `emit_wheel_fx`/`advance_wheel_fx` run windowed + headless: grounded
  local wheels only (remote/unidentified cars stay dark), puffs spawn
  `SessionEntity`-stamped at the contact point, `Velocity` rotated
  onto the contact normal (the records' `Position` means are authoring
  leftovers — `smoke` carries a fixed world offset), `PositionVar`
  jitters around the contact, billboarded tiles flipbook over
  `TexFrame*`, `Color` alpha + `DAlpha` + `Intensity` drive alpha.
- **`session.rs`** — `load_session_world` inserts `WheelFx` +
  `WheelFxReport`; teardown removes the resource and the entity sweep
  reclaims the puffs; `reset_wheel_fx_report` clears counters on
  unload.
- **`smoke.rs`** — `wfx=<r>r/<e>e/<x>x[+Nd+Nf+ut]`, printed only on
  activity/anomaly — quiet runs stay bit-identical.

## Evidence

- `cargo test -p mm2_app --test wheel_fx` — 15/15: eight-rule bind,
  authored-table channel resolution, `_default` fallback on unmarked
  contacts, threshold gating incl. threshold-0 parked-dark, dual-channel
  emission at the contact, missing rule/atlas diagnostics, restart
  rebind + sweep, remote/unidentified suppression, deterministic
  replay, pool bound + expiry conservation.
- `cargo test -p mm2_game --test effects` — 37/37: gate/blast/reground/
  rebind/seeded determinism/pool-bound legs plus integrator, flipbook
  and alpha; `ptx_rule_names_match_the_retail_string_table` pins the
  index table.
- Retail (`fnv1a64:e91e6cd4b2ae30d9`, headless `--frames 600`):
  - sf cruise → `wfx=8r/318e/192x+849d` — cobblestone `4 7`
    smoke+rock channels emit under the Hold driver's slip.
  - sf Golden Gate Park grass `--spawn=-1706,50,336,0` (`s_grass`
    room) → `wfx=8r/386e/262x+8702d` — `1 2` dust+grass live; the
    drops are the 128-puff bound discarding 64-burst blasts.
  - london Thames `--spawn=-80,2,805,0` → *no* `wfx=` field —
    `deepwater` authors `-1 -1` and stays dark through real wading
    (`rcv=3w` confirms water contact).
  - `s_water`/`s_pond`/`s_flower` sit in the PSDL texture tables but
    are referenced by no room attribute (probe-scanned both cities) —
    `water`/`sand`/`dirt`/`wood` have no stock-city-reachable surface;
    the splash channel is test-verified only.

## Gates

- `cargo fmt --all -- --check` — clean.
- `cargo clippy --workspace --all-targets --all-features --
  -D warnings` — clean.
- `cargo test --workspace` — all suites green.

## Classification / remaining open items

- Verified data: the index→name table (exe string block + authored
  pairs), the rule files, the 8×8 atlas, the field grammar.
- Designed (DSN-62): `tire_slippage` as the gate quantity, strict `>`,
  blast-on-edge/spew-while-held cadence, the 128/vehicle bound, the
  contact-point + contact-normal emission frame, billboard/flipbook/
  tint presentation.
- Unknown (UNK-23): the original's trigger quantity, emission cadence
  and whether `Damp`/`Height`/`Intensity` feed it — only the field
  names and values are evidenced.
- No visual capture or playtest — headless `--frames` runs freeze
  input so a slipping car can't be screenshot; emission evidence is
  the `wfx=` counters (spawn→advance→expire path exercised; billboards
  share the proven precip/damage quad path). F18-AC02's visual leg
  stays open.
- F18-B stays `active`: `wearain` cues, wetness presentation beyond
  particles, and weather-state replication (req 5 network leg → F24+)
  remain.

---

# Prior iterations

Iteration 83 on `ralph/night` (baseline `8f3e740` — the F18-B.2
candidate; external verify + review green; twenty-eighth iteration of
run `20260925T144723`). One coherent slice: F18-B's spec-req-4
precipitation *audio* leg — the authored `rainexterior`/`raininterior`/
`thunder` waves now drive session-scoped bed loops and seeded thunder
claps off the same effective-weather pick the particle rig reads.

## Task selection

The plan's F18-B row named precipitation audio hooks (`wearain`/interior
waves) as the next split. The retail install ships all three stems under
`aud/aud{11,22}` and the exe carries a `Rainexterior`/`Raininterior`/
`Thunder` string block beside the floats `0.65`, `0.85`, `13.0`, `15.0`,
`1.0` — authored data plus a parameter block, not an invented effect.
The original's runtime semantics are unrecovered (UNK-25), so the
consumer is a designed reading adopting those constants; the `wearain`
commentary-cue families stay unbound.

## Findings and actions

- **`mm2_app::audio`** — `WeatherAudio` (session-scoped resource
  `load_session_world` inserts when `Weather::precipitation` names a
  spec; `None` on dry), `WeatherVoice`/`WeatherRole` bed components,
  `VoiceKind::{Weather,Thunder}`, and `AudioReport` fields
  `weather`/`thunder`/`interior`. `weather_voices` lazily resolves the
  `<name>exterior`/`<name>interior`/`<name>`-adjacent `thunder` stems
  through the session `WaveBank` (resolve once, `failed` counted once,
  never retried or substituted — F18-AC06), spawns the beds as
  `PlaybackMode::Loop` voices at volume 0, and re-mixes them every
  update: `interior_mix` eases toward sheltered at
  `RAIN_CROSSFADE_PER_SEC` 4.0 under the same `precip::COVER_PROBE` 64 m
  upward cast from the active `WorldCamera3d` the drop emitter reads
  (the declared covered-interior approximation; no camera or no physics
  holds the last mix rather than snapping). Thunder draws `13.0`–`15.0`
  s delays from a domain-separated `NavRng` (restart-deterministic — the
  precip rig owns the bare seed stream), `Playing`-phase only, bounded
  `MAX_THUNDER_VOICES` 4, `PlaybackMode::Despawn` `SessionEntity`
  one-shots. The adopted constants are an inferred positional reading of
  the exe's adjacent floats — exterior `0.85`, interior `0.65`, thunder
  `1.0` — not recovered semantics.
- **`mm2_app::precip`** — `COVER_PROBE` is now `pub(crate)` so the bed
  crossfade and the drop emitter share one shelter distance instead of
  duplicating the constant.
- **`mm2_app::session`** — `load_session_world` inserts `WeatherAudio`
  after the siren block; `drive_session`'s teardown removes it.
- **`main.rs`/`smoke.rs`** — `weather_voices` runs
  `.after(session::drive_session)` on both paths; `aud=` gains
  `/<n>m/<n>t[i]` only when weather audio is live (`i` = sheltered at
  record time) — dry records stay bit-identical.

## Evidence

- `cargo test -p mm2_app --test audio` — 76/76 (+5): dry binds none,
  rainy binds both beds exposed at the exterior level, shelter-probe
  crossfade swings to the interior bed and back, a missing stem counts
  `failed` once with no substitution, the seeded clap lands inside the
  13–15 s window identically across same-seed apps, teardown sweeps all
  voices.
- `cargo test -p mm2_app --test precip` — 9/9 (+1 production leg):
  `load_session_world` binds `WeatherAudio` on a rainy session and
  spawns both beds, a dry session binds no resource, a restart rebinds
  generation 2 with no stale voices.
- Retail (`fnv1a64:e91e6cd4b2ae30d9`, sf headless `--weather 3
  --frames 1200`): `aud=0h/58v/0s/4l/4a/1r/4i/8c/1k/0g/31e/16n/2m/1t
  +22d` — both authored beds resolved, one clap fired inside the delay
  window, `0s` still honestly reports no output device attached.
  `--weather 0 --frames 300` records no `m`/`t` fields — dry runs stay
  bit-identical.

## Gates

- `cargo fmt --all -- --check` — clean.
- `cargo clippy --workspace --all-targets --all-features --
  -D warnings` — clean.
- `cargo test --workspace` — all suites green.

## Classification / remaining open items

- Designed (DSN-61): the adopted constants, the shelter-probe interior
  selection, the crossfade rate, the clap schedule and the voice bound.
- Unknown (UNK-25): the original's bed trigger/mix, interior selection
  and thunder semantics — only the stem names and the adjacent float
  block are evidenced; the `wearain` cue families remain unbound.
- No audible-output device, no playtest, no original parity comparison —
  F18-AC05's audio leg stays open.
- F18-B stays `active`: wetness presentation beyond particles,
  `wearain` cues, and condition replication (req 5's network leg is
  F24+) remain.

---

# Prior iterations

Iteration 82 on `ralph/night` (baseline `b05f9a0` — the F18-B.1 review
repair; external verify + review green; twenty-seventh iteration of run
`20260925T144723`). One coherent slice: F18-B's spec-req-2 precipitation
leg — the authored `tune/*.asbirthrule` particle definitions now drive a
bounded, deterministic, camera-relative precipitation rig on the shared
effective-conditions pick.

## Task selection

The plan's F18-B row named precipitation particles as the next split.
The retail install ships authored `asBirthRule` records
(`tune/rain.asbirthrule` — `Velocity 2,-35,0 ±2,5,2`, `PositionVar
25,0,25`, `Life 1`, `Radius .5±.1`, `SpewRate 200`, `Gravity -9.8`,
`TexFrame 0..15`, `BirthFlags 8`; `tune/snow.asbirthrule` —
`Velocity 0,-1,0`, `Life 1`, `Radius .06±.02`, `DRotation -2±5`,
`SpewRate 150`, `Gravity -6.8`, `TexFrame 5..7`) plus a measured 64×64
paletted `texture/ptx_rain.tex` card sheet — real authored data, not an
invented effect. The spec names covered-interior handling as a declared
approximation, so the consumer is a designed reading of authored inputs
(UNK-40 keeps the original `asParticles` runtime unrecovered).

## Findings and actions

- **`mm2_formats::banger`** — `BirthRule::parse_file` /
  `StandaloneBirthRule`: standalone records accept `asBirthRule` or
  `BirthRule` roots with an optional `type:` header, and default the
  fields these files omit (`Position`, the `D*` deltas, `LifeVar` —
  now decoded where present) to 0. The embedded `dgBangerData` decode
  keeps its strict required-field expectations; vehicle damage files
  are unaffected.
- **`mm2_game::config`** — `Weather::precipitation()`: `rainy` (3)
  → `"rain"`, every other selector → `None` (designed binding, DSN-60;
  `snow` stays parsed-but-unbound — no shipped selector names it).
- **`mm2_game::effects`** — `ParticleSpec` (distilled authored spec),
  `Precipitation` (session rig: seeded `NavRng` → deterministic stream;
  `SpewRate` draws inside `SpewTimeLimit` with sub-frame carry,
  `InitialBlast` credited on the first tick even at `SpewRate 0`, live
  bound `SpewRate × (Life+LifeVar)` + margin clamped to
  `PRECIP_MAX_LIVE` 4096) and `PrecipDrop` (designed integrator:
  `Gravity` accel, `Drag` exponential decay, `DRadius`/`DRotation`/
  `DAlpha` rates, authored `TexFrame` flipbook sweep).
- **`mm2_app::precip`** — VFS binding (`tune/<name>.asbirthrule` +
  `texture/ptx_<name>`) off `effective_conditions` in
  `load_session_world`; per-tile UV quads on a
  `ceil(√(TexFrameEnd+1))²` atlas space (4×4 on retail `ptx_rain`) via
  `damage_fx::tile_quad` (now crate-visible); `emit_precip`/
  `advance_precip` chained in Update on both windowed and headless
  paths. The emitter anchors on the active `Camera3d` with the
  authored `PositionVar` jitter around it (drops stay world-anchored);
  a 64 m upward probe suppresses sheltered spawns (`covered` — the
  spec's declared approximation); a swept segment+radius probe despawns
  drops on world contact (`landed` — nothing passes through the road).
  Billboards face the camera with `DRotation` roll; alpha drifts per
  drop on cloned materials.
- **Diagnostics** — a named rule that cannot resolve/read/parse marks
  `PrecipReport.absent` (`rule unavailable`/`unreadable`/`unparseable`)
  rather than silently running dry; a missing atlas warns and emits
  untextured drops (`+ut`). Resources are session-scoped
  (`SessionEntity`-stamped drops, `PrecipFx`/`Precipitation` removed on
  teardown, counters reset via `reset_precip_report` — `drive_session`
  stays inside Bevy's 16-param system limit).
- **Smoke** — `ppt=<name>:<emitted>e/<expired>x[+Nc+Nl+ut]` /
  `ppt=<name>!<diag>`; dry sessions record no `ppt=` field.

## Evidence

- `cargo test --locked -p mm2_formats` — banger unit tests +6 (rain/
  snow retail-shaped parses, non-particle root, missing-required,
  malformed-optional, unknown-field warnings); embedded decode tests
  unchanged.
- `cargo test --locked -p mm2_game` — effects +10, race +1 (selector
  map; spec mapping, rate/carry, bound incl. degenerate clamp, spew
  limit, initial blast, envelope/determinism, integrator, flipbook,
  alpha).
- `cargo test --locked -p mm2_app --test precip` — 8/8: authored bind,
  dry-none, missing/unparseable diagnostics, bounded camera-relative
  emission + conservation, contact-landed + life-expired legs, cover
  suppression, restart rebind.
- Retail (`fnv1a64:e91e6cd4b2ae30d9`, sf headless `--frames 300`):
  `--weather 3` cruise → `env=lt03(rainy-morning) ...
  ppt=rain:525e/0x+475c+522l surf=wet traction=0.8` (rule + atlas
  resolved, emission bounded, ~half the ±25 m envelope sheltered at
  the downtown spawn, drops landing on contact before expiry);
  `--weather 0` records no `ppt=` field — dry runs stay bit-identical.

## Gates

- `cargo fmt --all -- --check` — clean.
- `cargo clippy --workspace --all-targets --all-features --
  -D warnings` — clean (`Finished dev profile`, exit 0).
- `cargo test --workspace` — all suites green, exit 0.

## Classification / remaining open items

- Designed (DSN-60): the selector→rule binding, camera-anchor shape,
  emitter envelope, cover/contact approximation, atlas grid derivation,
  integrator and presentation.
- Unknown (UNK-40): the original `asParticles` integrator, anchor,
  coverage policy, atlas layout and presentation; `snow`'s runtime
  binding.
- A headless capture (`--weather 3 --cam=-1319,67,255,180,-12
  --frames 90 --screenshot`, retail sf — reproducible, kept local)
  shows faint droplet streaks around the camera: the quads render.
  **No playtest or parity comparison against the original exists** —
  F18-AC03's visual leg stays open.
- F18-B stays `active`: precipitation audio hooks (`wearain` —
  F07/F08 scope), wetness presentation beyond particles, and condition
  replication (req 5's network leg is F24+) remain.

---

# Prior iterations

Iteration 81 on `ralph/night` (baseline `bc7fee0` — the F18-B.1
candidate; external verify green, review verdict **fail** on one
blocking finding; twenty-sixth iteration of run `20260925T144723`).
One coherent slice: repair the review's record-integrity blocker —
close the `dev.traction = Some(1.0)` eligibility hole F18-B.1 opened.

## Task selection

The F18-B.1 external review's single blocking finding:
`record_eligibility` (`mm2_game::progression`) exempted a traction
pin via `dev.traction.is_some_and(|t| t != 1.0)`. That carve-out was
sound only while `TireConditions` read `dev.traction.unwrap_or(1.0)`
— i.e. while `Some(1.0)` was a guaranteed no-op. F18-B.1 made it
physics-active: on a session whose effective weather is rainy the pin
dries the tires 0.8 → 1.0 while the run stays record-eligible, so
`--traction 1` on authored `checkpoint:4 --pro` (rainy-noon) would
record a dry-grip result as a default-conditions run (DRV-6
violated). Repair before any new feature work.

## Findings and actions

- **`mm2_game::progression::record_eligibility`** — the arm is now
  `dev.traction.is_some()` unconditionally. No value-based exemption
  can be correct at this gate: it is config-only and cannot see the
  event's authored weather, and a `1.0` pin on a dry session is a
  bit-identical run anyway, so nothing of value is lost.
- **`mm2_game::config::DevOverrides::traction`** — doc now states any
  pin (`1.0` included, since it is physics-active wherever the
  effective weather wets the road) is `Ineligible::DevOverride`.
- **Ledger** — DSN-59's "keeps its `Ineligible::DevOverride`
  exclusion" claim was false for the `1.0` case; corrected to record
  the unconditional rejection and why.
- **Regression test** —
  `record_eligibility_gates_dev_and_modded_sessions` gains the
  `Some(1.0)` leg asserting `Err(Ineligible::DevOverride("traction"))`.

## Evidence

- `cargo test --locked -p mm2_game --test progression` — 19/19; the
  new `Some(1.0)` leg pins `Err(Ineligible::DevOverride("traction"))`.
- `record_eligibility` is the only consumption point
  (`mm2_app::progression::record_session_results` calls it for the
  ledger drain) — no parallel value-based carve-out exists.

## Gates

- `cargo fmt --all -- --check` — clean.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — clean (`Finished dev profile`).
- `cargo test --locked --workspace` — 80 suites, all `0 failed`.

## Classification / remaining open items

- F18-B.1 stays `implemented` (candidate) pending external re-check;
  the review's non-blocking notes stand — no rendered wet-driving
  evidence, symmetric opponent/trailer application by construction,
  and a `--traction 1` pin on a dry run still records `traction=1`
  (disclosed, harmless).
- F18-B stays `active`: precipitation particles (req 2),
  precipitation audio hooks and condition replication (req 5's
  network leg is F24+) remain.

---

Iteration 80 on `ralph/night` (baseline `88b447a`, F17-A.7 — external
verify + review green, non-blocking warts only; twenty-fifth
iteration of run `20260925T144723`). One coherent slice: F18-B's
weather→traction leg — the session's effective weather selector now
writes the environment traction modifier, so authored/menu/configured
rain wets every tire contact.

## Task selection

The F17-A.7 review passed with non-blocking warts only — none failing.
The plan's named list offered the F18-A remainder "→ F18-B/C scope":
F18-B spec req 3 wants surface wetness connected to F06 traction, and
the codebase already carried the whole path as a dev-only stand-in —
`--traction` pinned `TireConditions.traction` while the doc comment
explicitly deferred the session-legal writer to F18. The slice is one
small change: a designed `Weather::traction_factor` mapping plus the
session writer, riding the same `effective_conditions` pick the
lighting/fog/dome/wet-audio bindings already resolve. Precipitation
particles (req 2) are a deliberately separate, larger render slice.

## Findings and actions

- **`mm2_game::config`** — `Weather::traction_factor()` + `WET_TRACTION
  = 0.8` (designed, DSN-59): `rainy` (selector 3 — the only authored
  precipitation state, WLD-21) wets the road; every other selector is
  dry `1.0`, matching the single-wet-state rule
  `SurfaceVariant::for_weather` applies to the audio tables (DSN-43).
  No authored wet-grip data exists — the `surface{dry,wet}` CSVs are
  audio bindings (AUD-11) — so the original's rule stays unrecovered
  (UNK-39).
- **`mm2_app::session`** — `TireConditions` is stamped from
  `session_conditions.weather.traction_factor()`, with
  `dev.traction` kept as a quarantined pin *over* the factor (a `1.0`
  pin dries a rainy session; `Ineligible::DevOverride` unchanged —
  wrong: the `!= 1.0` carve-out let a pinned-dry rainy race record;
  corrected in iteration 81 to reject any pin).
  Player, opponents and trailers share the factor symmetrically —
  one tire path, one session resource.
- **`mm2_app::smoke`** — `traction=<f>` now records the *effective*
  modifier when non-default or explicitly pinned (previously it only
  echoed the dev flag); unmodified runs stay bit-identical.
- **Docs** — `DevOverrides::traction`, `--traction` help,
  `TireConditions` and `SurfaceState` comments updated (the dev flag
  is no longer the only writer); ledger gains DSN-59 + UNK-39;
  DSN-28's stale "wetness unconsumed" tail corrected;
  `environment.md`'s open list updated. The F17-A.7 row's `RaceRule`
  naming slip (`CheckpointRule` has exactly `AnyOrder`/`Ordered`) is
  fixed in PLAN.md/LAST_ITERATION.md per the review's wart note.

## Evidence

- `cargo test --locked -p mm2_game --test race` — +1
  (`only_rainy_weather_wets_the_tires`: selector census, `0 <
  WET_TRACTION < 1`).
- `cargo test --locked -p mm2_app --test environment` — 20/20, +3
  (`rainy_weather_wets_the_session_tires`: configured rainy →
  `WET_TRACTION`, foggy → `1.0`;
  `authored_rainy_event_wets_the_session_tires`: the authored row's
  Weather=3 beats configured dry through `effective_conditions`;
  `the_traction_pin_overrides_weather_wetness`: pin → 0.4, `1.0` pin
  → dry).
- Retail (`fnv1a64:e91e6cd4b2ae30d9`, sf headless `--frames 60`):
  `--weather 3` cruise → `env=lt03(rainy-morning) surf=wet
  traction=0.8`; `checkpoint:4 --pro` (authored rainy-noon) →
  `env=lt07(rainy-noon) surf=wet traction=0.8`; the same event's
  authored foggy amateur block → `env=lt06(foggy-noon)` with neither
  field; `--weather 0` cruise records neither — dry runs stay
  bit-identical.
- Sim-level causality was already covered
  (`mm2_vehicle/tests/surface.rs::a_wet_environment_limits_delivered_drive_force`
  drives the same `TireConditions` resource the session now writes).

## Gates

- `cargo fmt --all -- --check` — clean.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — clean.
- `cargo test --locked --workspace` — green, all suites.

## Classification / remaining open items

- Designed (DSN-59): rainy-only mapping, the `0.8` factor, pin-over-
  weather precedence, symmetric application via the shared
  `TireConditions` term.
- Unknown (UNK-39): whether the original scales grip by weather at
  all, its per-selector factor, material composition, and opponent/
  traffic symmetry.
- Authored rainy events stay record-eligible (authored conditions are
  the default run); customized rain remains `Ineligible::Customized`.
- F18-B stays `active`: precipitation particles (req 2 — covered/
  interior handling needs a declared approximation), precipitation
  audio hooks and condition replication (req 5's network leg is
  F24+) remain. No rendered rain-visual claim is made — this slice is
  physics-only; the wet *look* is unchanged beyond the authored
  `.ltNN`/fog/dome bindings that already selected on the same slot.

Iteration 79 on `ralph/night` (baseline `b183907`, F22-A.7 review
repair — external verify + review green, no blocking findings;
twenty-fourth iteration of run `20260925T144723`). One coherent
slice: the Circuit leg F17-A.6 deferred — RACE-3's parenthetical
laps + opponents options on a beaten Circuit event's options screen,
carried through `SessionCustomization::race` onto the built event
setup.

## Task selection

The plan's first-named slice is F14-C's unblocked legs, but its
remaining completability claims are gated on F15-B's unresolved
opponent research and the catalog-validation legs are a multi-source
audit — not one coherent small change. F17-A.6's own row names the
Circuit laps/opponents options as deferred pending "authored writers";
the authored data (`NumLaps`, `Opponents`, the `[Opponent]` aimap
roster) is all parsed and wired today, so the missing piece was one
focused change: a `RaceCustomization` pick, the writer applying it to
the built event, and the Circuit-only menu rows. It is a documented
original capability (RACE-3: `…weather, time of day, traffic density,
pedestrian density, cop density; for Circuit races the number of laps
and the number of opponents`), not an invented feature.

## Findings and actions

- **`mm2_game::config`** — `SessionCustomization.race:
  Option<RaceCustomization{laps, opponents}>`; `SessionConfig::validate`
  rejects `laps == 0` as the new `ConfigError::ZeroLaps` (a zero-lap
  Ordered race can never advance — rejected at the boundary rather
  than built).
- **`mm2_game::race`** — `apply_race_picks(&mut def, &mut roster,
  picks)` rewrites `RaceDefinition::laps` only on
  `CheckpointRule::Ordered` definitions (the Blitz/Checkpoint
  `AnyOrder` rule untouched — `NumLaps` is meaningless there, UNK-5), truncates
  `OpponentRoster.entries` to `min(picks.opponents, wired aimap
  count)` — a file-order prefix, never fabricated opponents — and
  syncs `definition.params.opponents`. `RacePicksReport` names what
  bound (incl. `opponents_clamped`). `CUSTOMIZE_LAP_MAX = 10` is the
  designed picker ceiling (authored Circuits write 2–4, CIR-5; the
  original's range is unrecovered).
- **`mm2_app::session::load_session_world`** — applies the picks
  between `event_race_setup` and `event_race` storage, before grid,
  HUD, minimap and session consumers read the setup.
- **`mm2_app::menu`** — `Screen::Customize` carries `race`/`seed_race`
  (`Some` only on `EventTableKind::Circuit`); `authored_seed` parses
  `NumLaps`/`Opponents` from the selected difficulty's authored block
  and refuses to fabricate on out-of-range values (row disabled with
  the reason). `Laps:`/`Opponents:` rows cycle `1..=CUSTOMIZE_LAP_MAX`
  and `0..=authored` via `CycleLaps`/`CycleOpponents` + `step_bounded`;
  `LaunchCustomize` folds `race != seed_race` into the same
  picks-differ-from-seed rule as conditions/densities, so an unchanged
  Circuit visit still launches a record-eligible default run and any
  changed pick is `Ineligible::Customized` (DRV-6).
- **Ledger** — DSN-58 records the designed semantics (lap bound,
  prefix truncation, roster cap); UNK-38 records what is unrecovered
  (original picker range, whether opponents could exceed the authored
  count, which entries a reduced pick fields, persistence across
  difficulty switches); DSN-29's deferred note updated;
  `docs/research/menu.md`'s UI-2 row updated.

## Evidence

- `cargo test --locked -p mm2_game --test race` — green (+3: Ordered
  laps rewrite + `EventParams.opponents` sync, `AnyOrder` untouched,
  roster truncation + clamp) plus config zero-laps validation.
- `cargo test --locked -p mm2_app --test opponents` — green (+2:
  `circuit_race_picks_apply_to_the_session_definition_and_roster`,
  `circuit_race_picks_never_exceed_the_wired_roster` — picked laps
  land on `RaceState.definition`, the picked prefix spawns, a pick
  beyond the wired aimap count clamps).
- `cargo test --locked -p mm2_app --test menu` — green (+4:
  authored-seeded `Laps`/`Opponents` rows on the beaten Circuit
  event, wrap-bounded cycling, a changed launch carrying
  `customization.race` through to the built `RaceState.definition`,
  a returned-to-seed visit launching `customization: None` +
  record-eligible).

## Gates

- `cargo fmt --all -- --check` — clean.
- `cargo clippy --workspace --all-targets --all-features -- -D
  warnings` — clean (the authored-seed tuple return refactored to a
  `Result` for `type_complexity`; `field_reassign_with_default` in
  the new zero-laps test fixed by constructing `SessionConfig`
  directly).
- `cargo test --workspace` — green, every suite.

## Classification / remaining open items

- Original requirement (documented, RACE-3/UI-2): laps + opponents
  options exist on a beaten Circuit event — implemented.
- Designed (DSN-58): `1..=10` laps ceiling, `0..=authored` opponents
  cap, file-order prefix truncation, `laps == 0` boundary rejection,
  text row presentation.
- Unknown (UNK-38): the original's picker ranges, whether a pick
  could exceed the authored roster, which entries a reduced pick
  fields, and persistence across difficulty switches.
- Not claimed: retail/original-content evidence (all fixtures are
  synthetic), a manual UI playtest, original opponent-AI semantics
  (UNK-11). F17-A stays `active` — ped/cop density consumers, Quick
  Race options and the AC03 interactive evidence leg remain.

---

Iteration 78 on `ralph/night` (baseline `8b8455f`, F22-A.7 — external
verify + review green, no blocking findings; twenty-third iteration of
run `20260925T144723`). One coherent slice: the review's non-blocking
wart — pad button edges bypassed the window-focus check
`vehicle_input`/`horn_input` carry — repaired through the shared
`control_just_pressed` gate plus the held glance stick.

## Task selection

The F22-A.7 review passed with two non-blocking warts; the actionable
one was the focus asymmetry: gilrs-style backends deliver pad input
while the window is unfocused where the OS never delivers keys, so a
pad edge (e.g. `North` reset) could fire during `Playing` while
alt-tabbed — a context keys never had. The review's suggested
remediation was a shared focus gate on `control_just_pressed`
consumers; that is exactly this change. (The other wart — a stray
`EOF`/`)` heredoc artifact in commit `8b8455f`'s message — is already
committed and externally checked; history is not rewritten.) A third
minor note, the F22-A parent row enumerating only `A.1…A.6`, is fixed
in PLAN.md.

## Findings and actions

- **`control_just_pressed` owns the gate.** The shared helper gained a
  `windows: &Query<&Window>` parameter and ANDs `windows_focused` onto
  the key-or-pad edge, so an unfocused window makes the *control*
  inert — device-agnostic, matching `vehicle_input`'s "unfocused
  zeroes everything" contract. `windows_focused` (extracted from the
  `windows.iter().all(|w| w.focused)` idiom `vehicle_input`/`horn_input`
  already wrote inline) treats zero windows as focused, so headless
  runs and the windowless test harnesses are unchanged.
- **Every consumer threads the query through:** `toggle_camera`,
  `mirror_input`, `hud_input`, `indicator_input`, `nav_target_input`,
  `hudmap_input`, `reset_input`, and `horn_input` — whose own inline
  `&& focused` was dropped now that the helper carries it (its
  `Playing` gate stays).
- **`cockpit_look`'s held stick reads released while unfocused.** The
  one non-edge pad input in the map: an alt-tab mid-glance eases home
  like a release instead of freezing mid-look, and resumes if the
  stick is still held on refocus.
- **Left alone deliberately:** the overlay pad rows (`pause.rs`,
  `results.rs`, `menu.rs`, `session_control_input`'s `Start`) predate
  the A.7 map, are overlay-owned, and their effects are menu
  navigation/pause — the review scoped the wart to `control_just_pressed`
  consumers. `hudmap_input`'s `Q`/`Esc` pause-map keys are key-only
  (no pad binding exists) so they needed no change.

## Evidence

- `cargo test --locked -p mm2_app` targeted suites — all green:
  session 25/25 (+1 `unfocused_window_gates_the_pad_map` — under a
  spawned `Window{focused:false}` a pad `North` reset, a pad `East`
  mirror toggle and a synthetic `R` edge are all inert; after refocus
  the mirror toggles and the reset lands through the production
  `Teleported` path), dash 13/13 (+1 `pad_look_releases_while_unfocused`
  — a held left-stick glance eases home across the focus loss and
  resumes on refocus), plus input 3/3, camtrack 22/22, mirror 9/9,
  hud 9/9, oppind 6/6, race 44/44, audio 70/70 unchanged.

## Gates

- `cargo fmt --all -- --check` — clean (one hudmap reflow applied).
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — clean.
- `cargo test --locked --workspace` — green, all suites (session 25,
  dash 13, input 3, camtrack 22, mirror 9, hud 9, oppind 6, race 44,
  audio 70 + the rest of the workspace).

## Classification / remaining open items

- Designed policy (DSN-57 extension, recorded in the ledger row): pad
  input shares the keyboard's effective focus contract — no original
  behavior claim, the original's pad handling is unrecovered.
- F22-A/F22-B stay `active`: AC06's multi-resolution/clipped-UI sweep
  and real-hardware pad playtest remain the open manual legs; a real
  unfocused-window pad press is unexercised (all evidence is the
  synthetic `Window{focused:false}` component + bevy's documented
  mocking surface — gilrs delivering input unfocused is documented
  backend behavior, observed in review/source only).
- Overlay pad rows (pause/results/menu `just_pressed` reads) keep the
  theoretical same asymmetry — navigation while alt-tabbed — but were
  deliberately out of scope: pre-existing surface, overlay-owned keys
  don't have a focus gate either, and a pause/menu cursor move while
  unfocused is benign.

---

Iteration 77 on `ralph/night` (baseline `6e65da3`, F22-B.5 — external
verify + review green, no blocking findings; twenty-second iteration
of run `20260925T144723`). One coherent slice: F22-AC06's bindings
leg — every in-session control the documented keys own now answers to
a designed gamepad binding through the same production systems, with
synthetic pad coverage through bevy's documented mocking surface.

## Task selection

F22-A/F22-B stay `active`; AC06 (keyboard/gamepad bindings + scaling,
no accidental driving in free-camera mode) had only the keyboard half
and three analog drive axes covered — every toggle/cycle leg was
keyboard-only, untestable for a pad and unbindable in the windowed
app. The plan names the remaining manual legs (multi-resolution
sweep, original `camTrackCS`/`camPovCS` semantics, hands-on
wall/mirror inspection) as open; the bindings leg was the actionable
one. F23's full controls/options scope (rebinding, dead zones,
persistence, navigation) is a separate feature — this slice is only
the designed in-session map AC06 asks about, and the original's pad
layout is unrecovered (MM2HELP's joystick topics are documented but
not transcribed; no decompiler locally), so every binding is DSN-57,
never a claimed original map.

## Findings and actions

- **`input::pad` names the map; `control_just_pressed` shares the
  key's gate.** One `pub mod pad` in `input.rs` holds the designed
  bindings: `RightThumb`=`C` camera cycle, `West`=`V` cockpit toggle,
  `East`=BACKSPACE mirror, `North`=`R` reset, `LeftThumb`=ENTER horn,
  `Select`=TAB map view, `DPadLeft`/`DPadRight`=`E`/`F` map
  zoom/rotate, `DPadUp`/`DPadDown`=`H`/`I` HUD/indicators,
  `LeftTrigger`/`RightTrigger` (the bumpers — the analog `*Trigger2`s
  stay on brake/throttle)=`Z`/`X` nav-target cycle, right stick =
  numpad cockpit glances at a designed 0.5 threshold. The pre-existing
  drive legs (`LeftStickX` steer, `RT2`/`LT2` analog throttle/brake,
  `South` handbrake) keep their precedence rules — non-neutral axes
  outrank held keys.
- **The pad ORs into each owning system, never a second context.**
  `control_just_pressed(keys, pads, key, button)` returns the
  documented key's edge OR the first connected pad's `just_pressed`
  — wired inside `toggle_camera` (C/V), `mirror_input`, `hud_input`,
  `indicator_input`, `nav_target_input` (X/Z), `hudmap_input`
  (TAB/E/F incl. the free-camera E/Q ownership split),
  `horn_input` and `reset_input`. Every existing phase gate —
  Playing/Countdown-only toggles, overlay key ownership,
  `allows_pause`, `Free`-camera detach — applies to the pad
  identically; menus keep their own pad row (South select/East
  back/West delete — West/East in menus never reach the cockpit/
  mirror toggles because those systems don't run there).
- **`input::reset_input` owns the `R` reset now.** The reader moved
  out of `main` into `input.rs` so both devices share
  `session::spawn_resets` verbatim — one implementation, scheduled
  identically in the windowed chain.
- **`dash::cockpit_look` rides the right stick.** `stick.y < −0.5`
  looks back through the authored `ReverseOffset`, `±x` glances
  sideways with the same exponential ease the numpad owns — held,
  not latched.

## Evidence

- New `tests/input.rs` 3/3: `pad_axes_drive_the_player` (analog
  steer/throttle/brake, South handbrake, non-neutral stick outranks a
  held key), `free_camera_detaches_the_pad` (maxed axes write a zeroed
  `VehicleInput` under `CameraMode::Free` — AC06's no-accidental-
  driving leg — and read again back in a drive view),
  `non_playing_phase_zeroes_the_pad` (Paused clears a held trigger).
- `camtrack` 22/22 (+1 `pad_walks_the_same_chain` — RightThumb walks
  Chase→Cockpit→Free→Chase, West shortcuts cockpit↔chase through the
  production `toggle_camera`).
- `mirror` 9/9 (+1 `pad_east_toggles_the_strip_while_driving` —
  toggles in Playing, inert in Menu/Paused/Results).
- `hud` 9/9 (+1 `pad_dpad_up_toggles_the_layer` — off/on in Playing,
  owned by the pause phase).
- `oppind` 6/6 (+1 `pad_dpad_down_toggles_the_indicators` — arms in
  Countdown, toggles in Playing, inert in Paused).
- `race` 44/44 (+1 `arrow_pick_cycles_through_the_pad` — bumpers walk
  `TargetSelection` forward/back with wrap).
- `session` 24/24 (+2 `pad_buttons_drive_the_map_controls` — Select
  view cycle + DPad zoom/rotate through `hudmap_input`;
  `pad_north_resets_the_player_to_spawn` — North emits the production
  `spawn_resets` bundle and the teleport lands via `Teleported`,
  identical to `R`/`--reset-at`).
- `dash` 12/12 (+1 `pad_stick_glances_and_reverses` — stick down rides
  the authored `ReverseOffset`, left eases toward +π/2, sub-threshold
  deflection never reaches the look; the harness's pinned 60 Hz
  `ManualDuration` clock makes the eased asserts deterministic).
- `audio` 70/70 (+1 `pad_left_thumb_fires_the_authored_horn` — one
  press spawns the authored `HornRequest` voice; the Menu phase gate
  is shared).
- Retail headless sanity (`fnv1a64:e91e6cd4b2ae30d9`): sf cruise
  `status=pass` — no pad means identical records; the bindings are
  additive input, not sim state.

## Gates

- `cargo fmt --all -- --check` — clean.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — clean.
- `cargo test --locked --workspace` — green, all suites.

## Classification / remaining open items

- F22-A/F22-B stay `active`: AC06's multi-resolution/clipped-UI sweep
  and real-hardware pad playtest are manual legs this iteration did
  not run; the map is DSN-57 designed — original pad bindings
  unrecovered (MM2HELP joystick topics not yet transcribed).
- F23 (persistent rebinding, dead zones/sensitivity, transmission,
  accessibility) is untouched scope — this slice adds no settings,
  no rebind layer and no menu pad navigation changes.
- The right-stick glance threshold (0.5) and first-pad-wins rule are
  designed constants; hot-plug/focus-loss device behavior is F23's.

---



Iteration 76 on `ralph/night` (baseline `1c75ed8`, F22-B.4 — external
verify + review green, no blocking findings; twenty-first iteration of
run `20260925T144723`). One coherent slice: the F22-AC05
transition/reset legs — the chase boom now snaps on implausible
displacement, and two scheduled dev overrides let a frozen-input
capture exercise a reset and a `C` transition through the production
paths.

## Task selection

F22-B stays `active`; the plan's open legs were the transition/reset
side of AC05 (atypical-vehicle framing landed in B.4). Reading the
camera code showed `chase_follow` lerping unconditionally — an `R`
reset, recovery or mode re-entry swept the view through the world —
and no capture-time control existed to trigger either while input is
frozen. The slice became: snap fix + shared reset/chain bundles +
scheduled evidence flags + captures.

## Findings and actions

- **Boom never snapped on teleports (fixed).** Every `chase_follow`
  frame lerped toward the boom target, so a `ResetVehicle` teleport
  (`R`, `dev_reset_at`), a water/stuck/disabled recovery or a scripted
  re-anchor sent the camera gliding in a straight line across the map
  — through walls, props and the city itself. `ChaseCamera` now keeps
  `last_pos` and snaps to the new anchor when one frame's displacement
  exceeds `BOOM_SNAP_SPEED × dt` (120 m/s — designed above every
  authored top speed, below any real teleport). The same check covers
  mode re-entry: the tracker goes stale while `chase_follow` is gated
  out, so the first chase frame after a far-away stint under
  Cockpit/Free snaps instead of flying back across the city. Ordinary
  motion, the decimetre-scale upright hop and the near↔far lens swap
  still ease — the vehicle doesn't move on a lens swap, so no jump
  registers. A rig's first tracked frame snaps too (no history to ease
  from), landing on the authored anchor rather than gliding in from
  the camera's spawn point. Designed policy — original transition
  semantics unrecovered (UNK-36).
- **`R` and the scheduled reset share one bundle.** The `R` key's
  inline player+trailer message construction moved to
  `session::spawn_resets` (player at `SpawnPoint`, every trailer at
  `spawn + yaw × authored_offset`); `DevOverrides::reset_at` /
  `--reset-at <ticks>` fires it once when the session clock reaches
  the tick while `Playing` — the production `ResetVehicle`/
  `Teleported` path verbatim, so race progress re-anchors identically.
  Record-ineligible (`Ineligible::DevOverride("reset-at")`): a
  dev-scheduled teleport changes the run's course like
  `--finish`/`--restart-at`; the `R` key stays legal play.
- **`--cam-cycle-at <ticks>` walks the real `C` chain once.**
  `toggle_camera`'s successor/activation rules factored into
  `next_available`/`activate_mode` shared with `dev_cam_cycle_at` — the
  scheduled leg takes the exact chain the key presses
  (Chase→Cockpit→ChaseFar→Free, absent cameras skipped). Render-only;
  stays out of `record_eligibility` like `--cockpit`/`--far`.
- Both overrides are `DevOverrides` evidence aids scheduled before
  `drive_session`/`chase_follow` in the windowed and headless chains,
  like `--restart-at`.

## Evidence

- `cargo test -p mm2_app --test camtrack` — 21/21 (+8):
  `teleport_snaps_the_boom` (200 m jump → boom on the anchor in one
  tracked frame), `small_displacements_stay_smooth` (1.5 m hop eases,
  no snap), `mode_reentry_snaps_the_stale_boom` (80 m driven under
  Cockpit → snap on re-entry), `lens_transition_stays_smooth` (near→far
  is not a jump), `first_track_lands_on_the_boom`, and three
  `cam_cycle_at_*` legs (chain advance + one-shot latch, absent-camera
  skip, pre-tick inert).
- `cargo test -p mm2_app --test session` — 22/22 (+3):
  `reset_at_teleports_the_player_back_to_spawn` (production
  `ResetVehicle`/`Teleported` path fires at its tick, session still
  `Playing`), `reset_at_beyond_the_run_never_fires`,
  `spawn_resets_reseats_the_whole_rig` (yaw-rotated trailer offsets).
- `cargo test -p mm2_game --test progression` — 19/19 (+2 arms:
  `reset_at` → `DevOverride("reset-at")`, `cam_cycle_at` → eligible).
- Retail headless `--seq --reset-at` records (the evidence drivers do
  run headless — the large-displacement leg):
  - sf `vpsemi --reset-at 720 --frames 800` vs control: accelerate
    stage net 13.5 m vs 21.9 m, peak 13.6 vs 22.3 m/s, gearbox held at
    F1 vs F3 +3 clutch — the teleport cut the drive mid-accelerate.
  - sf `vpbug --reset-at 1100 --frames 700`: `peak=30.5m/s` then
    `moved=3m final=(-1319,66.0,223)` — ~150 m of driving erased back
    to the spawn line.
- Retail windowed captures (Apple Silicon/Metal, `/tmp/f22b5/`,
  local-only): sf `vpsemi --seq --reset-at 1150` frames 565/585 — the
  rig's downhill creep (19.0 km/h, wheels 0/4 mid-bump) resets to
  2.9 km/h grounded at the spawn line, boom on the authored near
  anchor; `vpbug --reset-at 1100` frames 540/575 vs a no-reset control
  at 575 shows the same re-anchor. `vpsemi --seq --cam-cycle-at 800`
  frames 430/700 — the scheduled `C` lands the authored cockpit
  mid-run (dash cluster, CB mic, wheel) and holds it one-shot.
  Caveat: under `--frames`, live input *and* the evidence drivers are
  frozen, so windowed reset displacement is only the car's ~5–10 m
  neutral creep — the big-teleport leg is the headless record above.

## Gates

- `cargo fmt --all -- --check` — clean (one reformat applied).
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — clean.
- `cargo test --locked --workspace` — green, all suites.

## Classification / remaining open items

- F22-B stays `active`: the multi-resolution sweep (F22-C scope) and
  the recovered-runtime semantics (UNK-36 `camTrackCS` dynamics,
  UNK-37 `camPovCS` `CameraNear`) stay open; AC05's interactive
  wall-proximity/mirror legs on atypical sizes still owe hands-on
  inspection beyond the capture legs landed.
- The snap threshold is a designed constant (120 m/s), not derived
  per-vehicle — documented as such; original transition semantics
  unrecovered.

---



Iteration 75 on `ralph/night` (baseline `ba1005e`, F22-B.3 — external
verify + review green, no blocking findings; twentieth iteration of
run `20260925T144723`). One coherent slice: the F22-AC05
atypical-vehicle framing leg, which surfaced two real camera defects
on trailered/interior content plus the three non-blocking review
nits folded in.

## Task selection

F22-B was `active` with F22-AC05 (camera transitions, wall proximity,
mirrors and reset *visually inspected on atypical vehicle sizes*) open
and the review naming three gaps worth closing while adjacent: the
claimed `sized`-fallback and far-mode-input tests did not exist, and
the towed-trailer/occlusion interaction was untested. Exercising the
trailered stock cars on retail found real breakage, so the slice
became: fix + test + capture.

## Findings and actions

- **Own trailer counted as an occluder (fixed).** vpsemi's authored
  `_near` boom anchor (`Offset` z=7.73, rest ≈8.7 m) lands *inside*
  its ~14 m trailer box (trailer front ≈2.25 m behind the cab origin),
  so the `CollideType` ray hit the trailer and parked the camera in
  the hitch gap — the near view rendered a close-up of the cab's rear
  wall, and the far view (`Offset` z=19.5, past the trailer) clamped
  behind the trailer's rear face. `chase_follow` now builds the
  exclusion set from `SpawnPoint.trailers` in addition to the player
  entity: the player's own rig never occludes itself, other vehicles'
  trailers still do. Designed reading — the original's occluder set is
  unrecovered (UNK-36). Verified: vpsemi near now frames the cab over
  the flatbed deck, far shows the whole rig.
- **`camPovCS` `CameraNear 3.0` clipped whole interiors (fixed).**
  Four `_dash.campovcs` records (`vpsemi`, `vpcentury`, `vpcoop2k`,
  `vpvw_dune`) author a 3.0 m near plane while their interior cluster
  sits ~1 m ahead of the eye — the verbatim binding rendered a bare
  windshield with no dash at all. The cockpit camera now caps the
  authored near at a designed 0.5 m (every authored cluster lies
  closer); the authored value still reaches the mirror camera, where
  the high clip usefully hides the towed rig. UNK-37 — whether the
  original clamps, renders the interior in a separate pass, or truly
  hides those four dashes is unrecovered. Verified: vpsemi/vpcentury
  cockpits now render their full authored clusters.
- **Review's missing test legs added for real**: `tests/camtrack.rs`
  gains `drive_views_steer_and_free_detaches` (Chase/Cockpit/ChaseFar
  all write throttle; Free zeroes — the B.3 gate change now covered),
  `sized_lens_drives_the_fallback_boom` (chassis-derived offset,
  0–60 m/s window, `authored=false`, converged boom = rest), plus the
  trailer legs `own_trailer_is_not_an_occluder` /
  `other_trailer_still_occludes`.
- `docs/original-rules.md`: UNK-36 extended (own-rig exclusion),
  new UNK-37 (`camPovCS` `CameraNear` semantics), HUD-3 row updated.

## Evidence

- `cargo test -p mm2_app --test camtrack` — 13/13 (+4).
- `cargo test -p mm2_app --test dash` — 11/11 (+1: authored 3.0 → 0.5
  cap, authored 0.1 passthrough).
- Retail windowed captures (Apple Silicon/Metal, `/tmp/f22b4/`,
  local-only): sf vpsemi near/far **before** (cab-rear wall; trailer
  rear wall) and **after** (cab over flatbed deck; whole rig);
  vpsemi+vpcentury `--cockpit` after (full dashes rendered);
  vpbus near+far, vpcentury near, vpmoonrover near; vpsemi `--mirror`
  strip; dev-world `vpbug` pull-in at the z=200 perimeter wall
  (`cam …,199.3` vs the ~200.2 unconstrained boom).
- Retail headless unchanged in shape: sf vpsemi `trk=near+far`
  `dash=11p/cam`, `status=pass`.

## Gates

- `cargo fmt --all -- --check` — clean.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — clean.
- `cargo test --locked --workspace` — green.

## Classification / remaining open items

- F22-B stays `active`: AC05's transition/reset legs and the
  multi-resolution sweep (F22-C scope) remain open; UNK-36/UNK-37 hold
  the unrecovered semantics honestly.
- The near view inside a trailer volume relies on backface culling of
  the trailer's own walls — an open flatbed renders its deck and the
  cab correctly; a *closed* box trailer might show dark interior
  faces. No closed-box stock trailer exists to check (vpsemi/vpcentury
  are the only trailered roster cars), but a mod could ship one.

---



Iteration 74 on `ralph/night` (baseline `6426bd2`, F21-A.1 review
repair — external verify + review green, no blocking findings;
nineteenth iteration of run `20260925T144723`). One coherent slice:
the documented HUD-3 camera chain's missing third view — authored
`camTrackCS` near/far chase rigs bound per vehicle.

## Task selection

No failing gate or blocking review finding, so the highest-value
ready slice from the plan: F22-B was `active` with its
"occlusion handling and chase-near/far pair remainder" open.
Investigation found the plan's HUD-3 wording documents
`C` = Chase Near → Cockpit → Chase Far while the code cycled
Chase → Cockpit → Free (Free occupying the documented far slot —
the DSN-48 deviation), and that every stock `vp*` authors
`tune/camera/<id>_{near,far}.camtrackcs` (`camTrackCS` records;
mm2hook recovers `camTrackCS : camCarCS` with `Offset`, `TrackTo`,
`MinDist`/`MaxDist`/`MinMaxOn`, `MinSpeed`/`MaxSpeed`, `CollideType`,
FOV/near/far and approach/steer/hill dynamics fields). The data and
the documented chain both existed — only the binding was missing.

## Actions

- `mm2_formats::camtrack`: `TrackCamSpec` parser over the shared
  tune grammar — optional `type:` tag preserved, known scalar/vec
  fields decoded, sparse records tolerated, malformed/short vectors
  treated absent, wrong root block rejected, unknown fields retained
  for diagnostics.
- `mm2_app::camera`: `CameraMode::ChaseFar`; `ChaseLens` carrying the
  authored boom (`Offset` length = rest distance), `TrackTo` aim,
  `MinDist`/`MaxDist` bounds, `MinSpeed..MaxSpeed` window,
  `CollideType` flag and authored FOV/near/far, plus an `authored`
  flag; `ChaseLens::sized` is the designed chassis-size fallback;
  `load_track_cams` binds `tune/camera/<car>_{near,far}.camtrackcs`
  through the VFS; `TrackReport` surfaces `trk=near+far|near|far|
  sized` on the smoke record (stock sessions only — dev-world
  records stay bit-identical).
- `C` now runs the documented Chase-Near → Cockpit → Chase-Far chain
  marker-driven (Chase-Far skipped when no far record exists); Free
  is appended after the chain as the dev extension — the DSN-48
  stand-in deviation is repaired. `V` still enters/leaves Cockpit.
- `chase_follow` serves both chase modes off the active lens:
  authored offset + `TrackTo` aim + velocity look-ahead, speed-window
  boom extension toward `MaxDist` (designed reading), authored
  projection bound per lens, and `CollideType != 0` ray-cast
  occlusion pull-in excluding the player (0.25 m margin / 0.05 m
  floor designed — UNK-36). Approach/steer/hill/reverse fields parse
  but stay unbound pending UNK-36 recovery.
- `session.rs`: loads both records, builds authored lenses or the
  sized near fallback (never a fabricated far lens), inserts/removes
  `TrackReport`, resolves invalid persisted modes (Cockpit without
  `camPovCS`, ChaseFar without a far record → Chase) and spawns the
  chase camera on the active lens's projection.
- `input.rs`: the driving gate ran on `CameraMode::Chase` only —
  cockpit couldn't steer. Now every non-Free mode drives (chase,
  cockpit, far); only Free detaches input.
- `DevOverrides::far` + `--far` select Chase-Far at spawn
  (conflicts `--cam`/`--cockpit`, render-only, out of
  `record_eligibility`).
- `docs/original-rules.md`: HUD-3 row and DSN-48 corrected to the
  real chain; new DSN-56 (rig binding + designed readings) and
  UNK-36 (unrecovered `camTrackCS` dynamics semantics).

## Evidence

- `cargo test -p mm2_formats camtrack` — 4/4 (full record, wrong
  root, short-vector absence, sparse tolerance).
- `cargo test -p mm2_app --test camtrack` — 9/9 (chain order incl.
  far-skip and absent-camera no-op, lens binding + authored
  projection, speed-window extension, occlusion pull-in and
  disabled-path, sized fallback, far-mode input, smoke `trk=`
  shapes).
- Retail headless (`fnv1a64` install, read-only): sf `vpbug`
  `trk=near+far` `dash=11p/cam`; london `vpbus` `trk=near+far`;
  sf `--far` headless pass.
- Windowed captures: sf near vs `--far` frames show the authored
  booms differ (far pulls back/up, pitch −10 vs +4 on the HUD `cam`
  readout); cockpit capture unaffected.

## Gates

- `cargo fmt --all -- --check` — clean (exit 0, silent).
- `cargo clippy --workspace --all-targets --all-features --
  -D warnings` — clean (exit 0).
- `cargo test --workspace` — green: every suite ok, 0 failures,
  through the final doc-tests.

## Classification / remaining open items

- Authored-side verified: both `camTrackCS` records exist per stock
  vehicle and bind through the VFS (`trk=near+far` on retail).
- Designed readings (UNK-36): speed-window semantics + units, the
  occlusion margin/floor, the `TrackTo` frame reading, and every
  unbound approach/steer/hill/reverse field.
- F22-B stays `active`: F22-AC05 atypical-vehicle framing/
  transition validation and the remaining spec legs are still open.

---

# Last iteration — F21-A.1 review repair: falsified retail measurements (iteration 73)

Iteration 73 on `ralph/night` (baseline `88ff7ac`, F21-A.1 — external
verify green but review `fail` on three falsified measured-data
claims in the committed research doc; eighteenth iteration of run
`20260925T144723`). Doc/audit-surface repair only — no evaluator or
runtime work.

## Root cause

The F21-A.1 research doc recorded tail/aimap correlations that did
not match the committed tool's own output on the same install:

1. "sf `crash12` wires 10 `[Exceptions]` road overrides — the only
   lesson with any" — false: sf `crash1`, `crash2`, `crash4` and
   `crash12` each wire an *identical* ten-road block (roads 10–19,
   `1.0 35.0`) in both `.aimap` and `.aimap_p`; london wires none.
2. "All other tail cells are 0 on retail" — false: `tail[1]`=1 on
   london `crash4` `map` and sf `crash4` `oneeighty` rows (4 rows,
   `[0,1,0,0,0,0]`).
3. The `tail[3]`/`tail[4]` bullet misattributed the sf
   `stop`/`exam1_2` rows — they carry `tail[2]`=1 (the `numopp`
   position), which extends the numopp↔wired-opponent correlation to
   the e8 stop family (sf `crash6`/`crash7` both wire `vpford`).
   `tail[3]` is set on only three sf rows (`crash10` `follow`
   amateur, `crash11` `exam1_3` both difficulties); `tail[4]`/
   `tail[5]` are 0 on every retail row.

## Actions

- `docs/research/crashcourse.md`: rewrote the observed-correlation
  bullets from the audit output (with the `tail[k]` = extras index =
  file column k+5 convention stated), corrected the Exceptions bullet
  to the four-lesson identical block, and updated the open-questions
  tail line. `docs/original-rules.md` CC-7 and the
  `AimapWiring::exceptions`/`LessonObjective::Stop` doc comments
  corrected the same misattributions.
- Closed the review's first verification gap: `LessonTable` now
  carries `CrashDataFile::diagnostics` and the audit prints them as
  `note:` lines, so a malformed authored row can no longer silently
  shrink the audit. Diagnostics stay informational — retail headers
  legitimately carry quirks (`AmbDenisty`, omitted `Filename`) and
  zero rows drop on retail.

## Gates

- `cargo test -p mm2_content --test crashcourse` — 6/6 (+1:
  diagnostics visibility over a misspelled header plus a dropped
  malformed row).
- `cargo fmt --all -- --check` clean; `cargo clippy --locked
  --workspace --all-targets --all-features -- -D warnings` clean;
  `cargo test --locked --workspace` green.
- Re-ran `mm2-inspect crash-course <retail> --strict`
  (`fnv1a64:e91e6cd4b2ae30d9`, read-only): exit 0, both cities 13/13
  `ready`; the corrected doc was written from this output and the
  four sf `[Exceptions]` blocks were diffed — identical.

## Remaining open items

- F21-A stays `active`; UNK-35 unchanged (runtime semantics, `Event`
  dispatch, `Checkpoints`, `tail[1]`/`tail[3]` meaning, pro/amateur
  param divergence).
- `Aimap::validate` issues remain per-lesson informational prints
  outside the strict denominator (authored-quirk warnings).
- F21-B/F21-C stay queued; F21-AC02..AC05 evidence still pending.

---

# Last iteration — F21-A.1 the Crash Course lesson catalog audit (iteration 72)

Iteration 72 on `ralph/night` (baseline `b1ba2a1`, F22-A.6 — external
verify + review green, no blocking findings; seventeenth iteration of
run `20260925T144723`). One coherent slice: F21-A's first audit leg —
an independently audited Crash Course lesson catalog for both cities
(structural, not playable — see Classification).

## Task selection

No failing gate or blocking review finding, so the highest-value ready
slice from the plan: F21-A was `queued` with all dependencies
(F02-B/F11-B/F16-B) landed. Its spec demands a course/lesson catalog
with prerequisites, start conditions, vehicles, props, objectives,
limits, feedback and rewards before evaluators are built. The generic
`mm2-inspect event` audit already validated record closure, but no
course-oriented view existed for stages, sub-event tables, objective
codes or crash-specific aimap wiring — this iteration adds that layer
(DSN-55) and defers evaluators/instruction flow/retry to F21-B.

## What landed

- `mm2_content::crashcourse` (new): `CourseCatalog::scan` views the
  shared `EventCatalog`'s CrashCourse events as lessons —
  `LessonStage` (the authored `lesson`/`midtrm`/`final` tags), both
  `mmcrashdata` param blocks verbatim, `data.csv`/`data_p.csv`
  `LessonTable`s split Amateur/Professional (inferred `_p`), each row
  a `LessonSubEvent` carrying the raw `Event` code plus the inferred
  `LessonObjective` decode (measured correlation — unknown codes stay
  `Unknown(n)`), `Filename` waypoint links resolved through the VFS,
  own-stem aimap per difficulty distilled to police/vehicle ids +
  `.opp` route resolution (case-insensitive) + chase
  distance/exceptions/ambient counts, `<object>_crash<N>` extras
  re-attributed per lesson (unclaimed extras counted), `crash,N`
  rewards attached.
- `mm2_formats::crashdata`: retains the authored header cells
  (`columns`) — the only in-file tail-column evidence.
- `mm2-inspect crash-course <install> [--city] [--strict]`: audits
  every lesson per city; strict fails on empty catalog, incomplete
  events, unresolved links, missing/empty difficulty tables, aimap
  errors, dead `.opp` wires, or wired vehicle ids outside
  `VehicleCatalog`.
- `docs/research/crashcourse.md`: the measured file layout, `Event`
  decode table, tail-column correlations and open questions.
- `docs/original-rules.md`: CC-7 (inferred lesson composition),
  DSN-55 (catalog layer), UNK-35 (runtime semantics).

## Gates

- `cargo test -p mm2_formats crashdata` — 6/6.
- `cargo test -p mm2_content --test crashcourse` — 5/5: stage parse,
  the full objective enum (incl. 1/6 staying `Unknown`), a complete
  synthetic lesson (tables split, link + `.opp` resolution, wiring,
  override attribution, extras denied), incomplete + unresolved-link
  reporting, the empty-catalog denominator.
- `cargo test -p mm2_inspect crashcourse` — 3/3: clean course, an
  unknown-vehicle wire flagged by the cross-check, a table-less city.
- `cargo fmt --all -- --check` clean; `cargo clippy --locked
  --workspace --all-targets --all-features -- -D warnings` clean;
  `cargo test --locked --workspace` green.
- Retail audit (`fnv1a64:e91e6cd4b2ae30d9`, read-only):
  `mm2-inspect crash-course <retail>` → both cities 13/13 lessons
  `ready`, 0 unresolved links, 0 dead `.opp` wires, 0 vehicles
  outside the catalog, rewards at `crash3/7/11/12` matching CC-6;
  london 26 + sf 23 unclaimed extras counted, not filtered.

## Retail findings (new)

- sf `crash6` (stop) wires a scripted `vpford` opponent —
  corroborating the `numopp`=1 tail correlation on its row.
- sf `crash1`/`crash2`/`crash4`/`crash12` wire an identical ten-road
  `[Exceptions]` block (corrected in iteration 73 — the original
  entry presented it as crash12-only); sf `crash5` owns the only
  authored `[CopChaseDistance]` (150).
- The `Event` code belongs to the row, not the filename: london
  `exam1_2`=7 (maneuver) vs sf `exam1_2`=8 (stop); london
  `exam1_3`=7 vs sf `exam1_3`=2 (follow).
- `data_p` tables author tighter limits/higher density and, on sf
  `crash9`, a different waypoint file (`reverse180_p.csv`).
- Professional `mmcrashdata` rows author different tod/weather than
  Amateur on several lessons (london `crash6`/`crash9`, sf
  `crash5`/`crash9`…) — deliberate harder conditions or revision
  drift (UNK-35).

## Classification

Everything verified-original here is *data presence and correlation*
(CC-7's structure, the reward bindings, the aimap wiring). The
`Event`-code→family map and the `_p`=Professional/`_crash<N>`
attribution are inferred (recorded, not claimed as recovered); the
catalog layer itself is an implementation choice (DSN-55). No lesson
is playable: evaluators, instruction flow, retry and reward
consumption remain future work — structural catalog success is
explicitly not lesson execution.

## Remaining open items

- F21-A stays `active`: prerequisites/start conditions exist as data
  (CC-3's gating already lands in `AvailabilityTable`), but the
  environment-prop runtime binding, instruction/voice/subtitle
  linkage (location unrecovered — UNK-35) and evaluator semantics are
  open.
- UNK-35 covers the `Event` dispatch, tail-column semantics, the
  `Checkpoints` column's meaning (1 everywhere), pass/fail criteria
  and the pro/amateur param divergence.
- F21-B (evaluators + session flow) and F21-C (validation) remain
  queued.

---

# Last iteration — F22-A.6 the race standings cluster (iteration 71)

Iteration 71 on `ralph/night` (baseline `f24fa53`, F22-A.5 — external
verify + review green, no blocking findings; sixteenth iteration of
run `20260925T144723`). One coherent slice: HUD-2's remaining
instruments — the checkpoint list, laps record and place indicator —
bound to the authored `digitac_*_half` glyph set, plus the three
non-blocking review findings on F22-A.5's rasterizer.

## Task selection

Review passed with three verification gaps worth folding in while
adjacent: unguarded file-supplied strip indices (panic risk on a
hostile mod pkg), no best-LOD chunk dedupe (`pkg_to_parts` parity),
and a missing why-comment on `rasterize_tri`'s targeted allow. The
named F22-A remainder was the checkpoint list/lap/place instruments —
previously gated on a false premise: iteration 70 recorded the
`race_*` tiles as "alpha-masked TGAs the reader rejects". Direct
inspection this iteration proved both halves wrong: they are plain
24bpp TGA 2.0 files that decode cleanly through `city::load_image`,
and their content is menu/results artwork ("Laps", "Opponents",
"Race Records", "Select Vehicle" panels) — not in-race instrument
labels. The authored digit path the timer already binds
(`digitac_*_half`) is the right art for compact standings readouts.

## What landed

- `mm2_app::racestat` (new) — `spawn_race_stats` binds all ten
  authored `digitac_*_half` stems through `city::load_image` (any
  miss → `absent:missing-glyphs`, never a half-bound cluster or
  substitute art) and spawns a `SessionEntity`-stamped top-right
  translucent-plate column: `PLACE n/total` (the local participant's
  `live_order` standing; hidden while the field is a single
  participant — the `pos=` contract), `LAP n/total` (spawned only for
  `Ordered` definitions; a resolved participant parks at
  `laps/laps`), `CHECKPOINTS` listing every authored gate's 1-based
  index in the authored digits (cleared dims, the armed objective —
  `next` under `Ordered`, the arrow's `navigation_target` pick
  honouring `TargetSelection` under `AnyOrder` — lights warm), and a
  `FIN` entry under AnyOrder-with-finish that arms once every gate
  clears. `update_race_stats` reads `RaceState`/`RaceProgress`/
  `live_order`/`navigation_target` — authoritative state only, no
  parallel counters — and hides on `Complete`, stale generation,
  no-race and under the `H` gate while `RaceStatReport` keeps
  composing demand like `tmr=`'s `display`.
- Wiring: `lib.rs` module export; `session.rs` event-arm spawn +
  teardown `remove_resource`; `smoke.rs` appends ` sta=<glyphs>g/
  p<n>of<m>/l<n>of<m>/c<n>of<m>` (`-` per idle instrument) or
  `absent:<why>` on event sessions only — cruise/dev-world records
  stay bit-identical — and schedules `update_race_stats` after
  `drive_session` headless; `main.rs` same ungated slot so
  `--frames`/`--screenshot` captures see live state; `camera.rs`
  `HudNodes` retargets `RaceStats` to the active world camera.
- navarrow review repairs: strip indices are bounds-checked before
  the vertex-table index (`absent:bad-index` rather than a panic);
  the rasterizer now picks the best-LOD chunk per stem via
  `lod_split` and skips `shadow`/`dmg` stems, matching
  `city.rs::pkg_to_parts`; the `too_many_arguments` allow carries its
  why-comment.

## Gates

- `cargo test -p mm2_app --test navarrow` — 17/17 (+2: bad-index
  regression, best-LOD/shadow exclusion over a multi-chunk fixture).
- `cargo test -p mm2_app --test racestat` — 18/18: slot composition,
  full/partial/missing glyph binding (no half-bound cluster), the
  Ordered/AnyOrder spawn shapes (lap row and `FIN` scoping), live
  place off `live_order` through real `advance` crossings, cleared/
  armed/pending gate tints, `FIN` arming, `Complete`/stale/cruise
  release, the `H` gate hiding while the report composes,
  `SessionEntity` teardown, synthetic-event `sta=` legs through the
  real `headless_smoke` pipeline, dev-world field absence.
- `cargo fmt --all -- --check` clean; `cargo clippy --locked
  --workspace --all-targets --all-features -- -D warnings` clean;
  `cargo test --locked --workspace` green.
- Retail headless (`fnv1a64:e91e6cd4b2ae30d9`, read-only):
  `--city sf --event checkpoint:0 --frames 200` →
  `sta=10g/p3of7/-/c0of6` beside `pos=3/7`/`cp=0/6` in the dev
  telemetry — the instrument agrees with the authoritative line;
  `--city london --event blitz:0` → `sta=10g/-/-/c0of3` (solo blitz
  field hides the place row correctly);
  `--city sf --event circuit:0` → `sta=10g/p5of5/l1of3/c0of9` beside
  `lap=1/3`/`pos=5/5`/`cp=0/9` — the Ordered lap row on authored data.
- Retail windowed (Apple M1, Metal): `--city sf --event checkpoint:0
  --frames 150 --screenshot` renders the top-right cluster — `PLACE
  3/7`, `CHECKPOINTS 1..6`, `FIN` — in the authored green digits beside
  the arrow/timer/indicators (`/tmp/f22a6_stats.png`, local, not
  committed).

## Classification

The instruments are documented original members of `mmHUD` (HUD-2)
and the digit artwork is verified retail content. Everything about
*composition* is designed (DSN-54/UNK-34): cluster placement
(top-right, opposite the authored map inset), the dev-font
`PLACE`/`LAP`/`CHECKPOINTS`/`FIN` labels (no authored label art was
identified — the `race_*` tiles are menu panels, not instrument
labels), the `n/total` pair form, per-gate index list (vs any
remaining-count form the original might have drawn), and the
cleared/armed tints. `docs/original-rules.md` updated: HUD-2,
DSN-54, UNK-34, plus the `race_*` correction propagated into
DSN-53/UNK-32.

## Remaining open items

- F22-A stays `active` — every HUD-2 instrument now has an authored-
  art binding, but F22-AC01–AC06 acceptance evidence is still
  partial (multi-leg retail runs over `H`/`I`/restart interactions),
  and every instrument's original presentation is unrecovered
  (UNK-30..34 — designed readings all).
- UNK-34 needs a retail-original capture or recovered draw bodies to
  pin the real standings layout — our cluster verifies our own
  rendering, not the original's.
- The armed-gate tint is legible but subtle against the authored
  green digits — worth revisiting if a retail capture shows a
  stronger cue.

---

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
