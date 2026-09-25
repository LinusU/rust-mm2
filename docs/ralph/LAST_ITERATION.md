# Last iteration — F10-B.13 wreck bound + striker-class disclosure (iteration 50)

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
