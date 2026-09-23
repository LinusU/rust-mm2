# Last iteration — F04-C.4 breakable-prop momentum transfer

Iteration 48 on `ralph/night`, continuing from the F18-A.3 fog-table
candidate. Operator report 4's item 2 was next in the report's own
order (item 4 landed last iteration): hitting light breakables —
parking-meter-class props — behaved like hitting a wall, with extreme
bounce and total speed loss, where the original lets momentum carry
through at speed. The report's scope note explicitly clears this work
(prop collision response is authored-data fidelity, not the
operator-owned handling feel). F04/F05 stay **implemented** — the
exact original exchange quantity remains open under UNK-22.

## Root cause (measured, not guessed)

The report's lead was the mechanism, confirmed in code: Avian solves
each step before `activate_bangers` runs (`FixedLast`), so a dormant
prop answered its first contact as an infinite-mass static body. The
striker took the full wall impulse; only then did the prop activate
with a flat approach-speed kick. More speed meant a harder wall —
exactly the reported symptom.

A second latent defect surfaced while wiring the fix: Avian's
`Collisions::get` is order-independent and `ContactPair` keeps
broad-phase order, so `anchor1`/`normal` did not mean "the caller's
first entity" — every consumer silently read pair-order data.

## What changed

- `mm2_app::contracts`: `deepest_contact` returns `ContactDetails`
  (point, severity, combined restitution, solver-applied normal
  impulse, both body anchors) with normal/anchors re-expressed in
  *caller argument order* — the stored-pair-order leak is fixed for
  `collect_impacts`, `activate_bangers` and `knock_ambient` alike.
- `mm2_app::banger`: a qualifying activation replays the hit as a
  two-body transfer — `J = (1+e)·v·μ` (reduced mass of striker and
  authored `Mass`); the striker's correction returns the applied wall
  impulse along `applied_dir` (the normal the solver actually pushed
  on, so glancing contacts keep their deflection) and pays `J` instead,
  plus a contact-anchor torque term; the prop launches at
  `J/prop_mass` plus the authored `Size` spin kick. `StrikeBound`
  overlaps pay the same share (`applied_impulse = 0`, record
  `Elasticity`); a bound strike coincident with a below-limit world
  contact folds the real solve data in — charged once. Unresolvable
  striker mass degrades to the pre-transfer launch.
- Authored `Mass`/`Elasticity`/`ImpulseLimit2` paths untouched; no
  tuning constants. Vehicle handling parameters unchanged.

## Verification (this tree)

- `cargo test -p mm2_app --test banger` — pass, 20 tests. +3
  regressions: `a_light_prop_takes_only_its_momentum_share` (striker
  keeps ~19 of 20 m/s where the wall response left ≈−5),
  `a_bound_strike_charges_the_striker_its_momentum_share` (overlap
  with no manifold still pays its share),
  `a_coincident_contact_and_bound_strike_charge_once` (the returned
  wall impulse is not double-paid).
- `cargo fmt --all -- --check` — pass.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — pass.
- `cargo test --locked --workspace` — pass, 64 suites, 0 failures.

## Retail evidence (fingerprinted install `fnv1a64:e91e6cd4b2ae30d9`)

Identical commands, same install, only the transfer changed:

- london `vpbug --spawn=0.4,5.5,-720,0 --headless --frames 1500`
  (the `sp_bollard_black_l` row, 13.6 kg): before `moved=55m
  peak=17.4m/s bng_ev=2a/1s/0b` — wall-stopped, wandered into a
  second bollard; after `moved=124m peak=24.5m/s bng_ev=1a/0s/0b` —
  carried speed through and drove on.
- sf `vpddbus --spawn=-1641.6,36.7,410,0 --headless --frames 800`
  (the `sp_cone_f` cluster, 10.6 kg): before `moved=46m peak=19.6m/s
  bng_ev=2a/0s/0b` — stopped dead; after `moved=99m peak=19.7m/s
  bng_ev=3a/1s/0b`.

Headless counters prove momentum retention, not rendered feel — no
windowed/GPU capture this iteration, no original-executable
comparison (retail binary not runnable here).

## Ledger / docs

- `docs/research/banger.md`: transfer mechanism documented under the
  implemented-slice list; before/after evidence added to the observed
  strikes; UNK-22 bullet extended — the exchange quantity stays
  provisional.
- `docs/original-rules.md`: DSN-10 updated (two-body transfer, F04-C.4,
  still UNK-22-classified).
- `docs/ralph/PLAN.md`: F04-C.4 task row; report 4 item 2 annotated
  implemented/candidate; selection narrative updated.

## Not done / blockers

- The original's exact striker↔prop exchange is unrecovered (UNK-22) —
  the transfer is an implementation choice repairing a measured
  defect, not a verified original rule.
- Break-vs-tip threshold semantics, `NumParts` runtime role,
  `BirthRule`/audio/flash effects remain UNK-22.
- Operator report 4 remaining items: 1 (traffic intersection
  teleport), 3 (trees/large poles breakability — research first), 5
  (startup warn aggregation).
