# F02 - Complete vehicle loading and handling regression coverage

## Outcome

Preserve and verify the newly added vehicle loader so every stock player vehicle drives and renders with its own data.

**Priority:** 2 (lower is earlier, subject to dependencies).
**Input contracts:** F00-B, F01-B
**Read alongside:** `PROJECT.md`, `ARCHITECTURE.md`, `QUALITY-GATES.md`.

Dependencies identify minimum usable input slices, not permission to assume an entire predecessor is verified. Search the current source first. Existing code that satisfies this spec should be retained and tested, not replaced.

## Current context and research

The current source already includes VehicleCatalog, VehicleDef, conversion reports, model parts, trailers, car_visual, and --car/--paint/--list-cars. Audit and extend these; do not replace them with the older cuboid-only design.

Source keys: R1, R3, R4; see `SOURCES.md`. Original-specific rules require evidence from the supplied installation/help or a documented reference observation. Source links are research entry points, not proof of implemented support.

## Required behavior

1. Use the previous vehicle milestone as the behavioral contract: complete expected player roster including unlockable entries, correct original models/paints/pivots/bounds, mod-aware dependencies, and no hidden dev-car fallback.
2. Check imported mass/inertia/center of gravity, power/RPM, transmission speed-to-ratio conversion, driven axles, wheel dimensions, suspension, front/rear grip, brakes, and intended enhanced assists against conversion provenance. Display statistics are not authoritative physics.
3. Preserve distinct handling rather than normalizing every vehicle to the same torque or chassis. Extra visual wheels must not multiply mass, available traction, braking, or engine output. Validate actually needed trailer/articulation behavior.
4. Provide a synthetic benchmark using the production Avian systems: settled ride height, controlled acceleration, braking, cornering, reverse, landing, reset. Report unreachable targets and timeouts explicitly.
5. Verify all declared paint bindings and wheel-linked parts. Check lights, intact breakable pieces, camera framing, spawn clearance, and interpolation after teleport/reset.
6. Retain generic offline dev-car behavior only when no original vehicle is requested. Keep telemetry usable by audio, damage, AI, and networking without those consumers accessing parser internals.

## Acceptance checklist

Each criterion needs a reproducible command/scenario and recorded result. Compilation, synthetic fixtures, real-content parsing, rendered output and manual playtesting are different evidence levels.

- [ ] **F02-AC01** - Every independently expected stock player ID is cataloged; malformed/missing cars remain in the denominator and fail strict validation.
- [ ] **F02-AC02** - Every stock car loads its own effective handling and model, can accelerate/brake/steer/reverse/reset, and stays finite in the benchmark.
- [ ] **F02-AC03** - Every declared paint variant resolves correct shader sets; at least one default-paint render per stock vehicle is checked when a GPU is available.
- [ ] **F02-AC04** - A power or mass override changes a non-traction-limited acceleration measurement; grip/brake overrides affect suitable unsaturated tests.
- [ ] **F02-AC05** - Small, tall/heavy, unusual-wheel, and articulated vehicles spawn and reset without penetrating roads or each other.
- [ ] **F02-AC06** - Both city startup paths, existing synthetic tests, mod overrides, and invalid-car CLI errors remain working.

## Edge cases to cover

Reverse/neutral gear entries; absent optional tuning; false four-wheel assumptions; original ratio units; mismatched override wheel rigs; detached trailers after reset.

## Suggested small implementation slices

### F02-A

Audit current vehicle catalog/assembly against the expected roster and conversion reports.

### F02-B

Fix verified per-car loading, rig, physics, or telemetry gaps with causal regression tests.

### F02-C

Run the complete roster/paint/handling matrix and publish honest original-data versus synthetic coverage.

A slice is not automatically one session: split further into `.1`, `.2`, etc. in the implementation plan when needed. Keep the parent requirement open until every child and acceptance criterion is satisfied. Do not spend one iteration implementing this whole document.

## Non-goals and completion discipline

No wholesale physics rewrite, AI texture work, or new vehicle art. Full cockpit instruments and destruction are handled separately.

No original assets in public git, no implicit installs/purchases/public services, no unchecked "all supported" claim. Record actual tested content and failures. Missing capabilities may block verification while allowing unrelated tasks to proceed.
