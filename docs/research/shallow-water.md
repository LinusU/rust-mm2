# Shallow water: placed bounds, wheel drag and splashes

Recovered 2026-10-11 with Kuna v1.785 from `retail/Midtown2.exe`, SHA-256
`93afb6c00be3d3b12a6e5d88d8e4f711a13f5a4100dbdfab77943c30083f98a2`.
Addresses below use this executable's image base `0x400000`.

## Original evidence

The Hyde Park lake used by London's "Hyde And Go Seek" is a placed
`kl_hydeparklake_l` object. Its `bound/kl_hydeparklake_l_bound.bnd` has
45 vertices and 39 polygons, all selecting material slot 1, named `water`.
The bounds include the sloping perimeter and the submerged floor, about
0.5 m below the perimeter. The visible texture and the PSDL material map
cannot identify this contact material.

**verified_original:** `phBoundGeometry::Load` (`0x48579b`) obtains each
bound material via the global manager's virtual method at vtable offset 4.
The city installs the derived manager (`0x466980`, vtable `0x5b0b04`), whose
method `0x466a40` parses the local material name, searches the existing
named material and returns it. An already loaded `water` therefore retains
the city's properties, rather than the bound's older friction/elasticity
values replacing them. The city table supplies friction 0.68, drag 0.119,
depth 0.1 and particle indices `-1 6`, thresholds `0 0`.

**verified_original:** wheel contact (`0x4d29f0`) obtains the hit polygon's
material and reads drag at material+0x30 and friction at +0x2c. Wheel forces
(`0x4d34d0`) add water resistance outside the tire friction circle:

```text
lateral drag      = -abs(vLat) * TireDragCoefLat * load * vLat * material.drag
longitudinal drag = -(1 + sinkDepth) * abs(vFwd) * TireDragCoefLong
                    * load * vFwd * material.drag
```

This is speed-dependent resistance, not an arbitrary speed cap or timer.
The port already implements both quadratic drag terms, with sinkDepth zero
(sink-depth evolution remains outside this change).

**verified_original:** `vehWheelPtx` (`0x4d1e80`) reads the two material
particle indices at +0x40/+0x42 and thresholds at +0x44/+0x48. Its emitter
(`0x4d1ee0`) compares `vehWheel+0x220` (visual slip) with the threshold.
Assembly `0x4d1eec..0x4d1efa` uses `FCOMP`, `FNSTSW`, `TEST AH,0x41`,
then skips on less-than/equality: emission requires strict `slip > threshold`.
The splash channel thus needs nonzero visual slip, including rolling/launch
transients, even though its authored threshold is zero. The port's native
car effects already consume this value. Generic cars retain their designed
utilization proxy. The same routine reads integer `asBirthRule+0xa8` (`InitialBlast`; the
field binding is explicit at `0x45fac0..0x45fad2`) and accumulates
`InitialBlast * dt * loadFactor`, truncating whole particles and keeping
the fractional remainder (`0x4d20e5..0x4d2115`). It does not use an
entry burst or `SpewRate` for wheel particles. The load factor is:

```text
N < L:  (N/L + 1) * 0.25
N >= L: min(((N-L)/(springRate*suspensionLimit) + 1) * 0.5, 1)
```

Here N is suspension force and L is static wheel load. The port now uses
this continuous rate for native cars, so the splash rule's `SpewRate 0`
does not stop the effect after an entry burst. Fractional credit survives
closed slip gates. The existing per-wheel credit streams and live-particle
pool are retained; retail shares two channel credits across four wheels.
The same routine rewrites the emitter means (`0x4d1f8a..0x4d20e2`):

```text
origin = lastHit + rearDir * radius * sign(-retailOmega)
         + worldUp * loadFactor * wheelWidth * rule.Radius * 0.5
particleRadius = loadFactor * wheelWidth * rule.Radius * 0.5
velocity = lateralDir * vLat * rule.Velocity.x
           + normal * abs(omega) * radius * rule.Velocity.y
           - rearDir * vSlip * rule.Velocity.z
```

The field at birth-rule+0x58 is **Radius**, verified by its loader binding
at `0x45f9b2..0x45f9be` (string `Radius` at `0x5c5300`); it is not Height.
The port now uses these means, with its positive-forward wheel-spin
convention. Scaling the upward velocity by rolling speed is necessary:
the splash rule's unscaled 0.5 m/s spray stays hidden below the lake's
visible water mesh. Random variance, per-particle integration and
billboarding retain the port's designed policy; full visual parity is not
claimed.

**authored data:** `texture/ptx_wheel.tex` is an 8×8 RGBA atlas. Its
unused rows visibly label 56..63 at the top, then 48..55, 40..47 and
32..39: frame numbering starts at bottom left. The splash rule selects
16..21, the spray graphics in the third row from the bottom. The port's
wheel-only UV mapping now flips the row index to image coordinates;
previously it sampled unused transparent tiles, hiding emitted splashes.
Shared smoke/precipitation atlas conventions are unchanged.

## Port integration

Static props whose ASCII bound contains a named material with nonzero
authored drag now use that bound's geometry instead of their render mesh
for collision. Triangles are partitioned by authored material, preserving
dry banks in mixed-material bounds. Each collider carries `SurfaceMaterial`,
`TireSurface`, original hull-contact material and restitution derived from
the same session table. Wheels, audio and particles share the identity;
no texture-name guess or global below-water plane is involved.

Liquid-bearing bounds apply to every static prop. Dry bounds replace the
render mesh for INST placements only (WLD-37, placement.md § Golden Gate
deck); stamped props, moving props and absent/malformed bounds retain the
existing PKG collision path. Binary bounds and `.ter` acceleration data are
not newly interpreted. Mods resolve through the existing VFS priority for
both the bound and city material tables. Collider entities retain session
ownership for unload/restart. Shallow `water` remains below the deadly-water
drag threshold (0.3); this change does not turn the pond into a recovery zone.

## Reproduction

Synthetic mixed bank/water bounds check separate identities, drag/grip,
particle selection, collision height, invalid material indices and nonfinite
vertices:

```sh
cargo test -p mm2_app --lib city::tests::liquid_bounds
```

The original-content regression loads the London city through the production
session importer, places an imported Mini on the lake and drives for three
seconds. It repeats with only the collider's drag disabled, measuring actual
speed, grounded water contacts and emitted particles:

```sh
MM2_RETAIL="$PWD/retail" cargo test -p mm2_app --test app \
  retail_hyde_park_water -- --nocapture
```

This validates the production physics/effect path against authored content,
not an original-game live trajectory comparison or a manual race playtest.

Measured on the imported Mini: after 180 driven 60 Hz samples, water
reaches **9.858 m/s** versus **10.687 m/s** with drag disabled. Both runs
retain all 720 measured wheel contacts on the water bound; the wet run
emits 150 splash particles. The dry control changes only drag, keeping
water geometry, grip and particle selection identical.

The rendered capture confirms small spray sprites around the moving wheels:

```sh
cargo run -p mm2_app --bin mm2 -- --mm2-path retail --city london \
  --car vpcoop --no-profile --spawn=-589.9305,-1.0752,-300.4589,36.6693 \
  --bot --frames 90 --screenshot /tmp/mm2-shallow-water-final.png
```

Unlike a stationary capture, `--bot` explicitly enables scripted driving
while live input is frozen. The camera follows the imported Mini.
