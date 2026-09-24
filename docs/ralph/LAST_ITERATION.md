# Last iteration — F13-C.3: sf-8 hypervelocity banger cascade diagnosed and bounded

Task slice on `ralph/night` (baseline `6ab6325`, the externally checked
F13-C.2 commit). Selected the named F13-C remainder: the scripted sf-8
leg that ran ~8-12 updates/s, recorded `dropped=53955` and a transient
`peak=17070 m/s`, and (reproduced this iteration) ended
`status=fail — fell through the world` after ~9 min for 1200 frames.

## Diagnosis (retail `fnv1a64:e91e6cd4b2ae30d9`, instrumented run)

The leg is not merely slow — it is a physics energy explosion:

- Frame diagnostics (`MM2_SMOKE_DIAG`, now a permanent env-gated
  instrumentation block in `smoke.rs`) show the cascade onset at
  ~f797: `CollisionStart` jumps to 10-25k/frame, dormant bangers burn
  5953→120 in ~10 frames, frame times hit 1-2 s. Process footprint
  reached ~19 GB of `MALLOC_SMALL` (76M live nodes) — Avian's
  `store_contact_impulses`/`warm_start` dominating `sample` profiles.
- Per-body velocity tracing shows the amplifier: fragment
  `sp_lightstreet_f-break01` goes 158→10077 m/s in one frame, then
  `*-breakNN` fragments across the city reach ~4×10⁶ m/s and tunnel
  block-wide per substep, breaking props wherever they land.
- Mechanism: `BangerDefinition::angular_kick` divides torque by a
  cuboid inertia estimate that is tiny on small break pieces → huge ω
  → a later contact's `normal_speed` reads the ω×r surface velocity as
  approach speed → `resolve_transfer` launches the struck prop's
  pieces at ~that magnitude → exponential across generations. A ~4M
  m/s fragment hitting the car produced the recorded 17070 m/s peak
  and the below-world fail.

## Fix (implementation choice — the records carry no speed limit)

- `mm2_game::banger`: `MAX_BANGER_LINEAR_SPEED = 200 m/s`,
  `MAX_BANGER_ANGULAR_SPEED = 60 rad/s` next to `DEFAULT_ACTIVE_POOL`.
  A legitimate transfer launch cannot exceed ~(1+e)·striker speed
  (~180 m/s for the fastest stock car); the caps only clip runaway.
- `banger_bundle` (mm2_app): every banger body — dormant, activated,
  fragment, breakaway piece — now carries Avian `MaxLinearSpeed` /
  `MaxAngularSpeed`. The integrator clamps solver-body velocity every
  substep and writes the clamped value back.
- Write-side clamps so components never hold absurd values between
  frames (constraint prep reads them pre-clamp): `angular_kick`
  clamps its output, `activate_bangers` clamps `severity` at
  `Activation` construction on both the contact and bound-strike
  paths (bounding the authored-limit estimate, the transfer impulse —
  hence the striker payment — and the recorded cause), and clamps the
  written `launch` at its single use site.
- Regression tests (`tests/banger.rs`): the bundle stamps the caps; a
  body spawned at 5000 m/s / 10⁶ rad/s is clamped within a frame; a
  hypervelocity striker overlapping a dormant prop activates it but
  the launched prop stays bounded (the transfer can no longer
  propagate the spike to the next generation).

## Verification

- sf-8 scripted, 12000 frames: **70 s wall** (was ~9 min for 1200
  frames at the failure point), `status=pass`, `peak=30.1 m/s`,
  `dropped=0`, `wheels=4/4`, `bng=5941d/13a/4s/3b` (3 props broken —
  normal), `impacts=333`, race progressing `cp=4/8`, `pos=1/7`. Zero
  bodies exceeded 150 m/s in the diagnostic run.
- Gates: `cargo fmt --all -- --check` clean; `cargo clippy --locked
  --workspace --all-targets --all-features -- -D warnings` clean;
  `cargo test --locked --workspace` green (64 suites).

## Not done / open

- The Professional matrix remainder is unchanged: 21 events undriven
  scripted at Pro, no Pro hold/parked legs. sf-8 no longer blocks the
  runtime legs' cost.
- The scripted driver's sf-8 pace is a driving-quality question
  (re-anchors, cp=4/8 at cap), not a physics defect.
- Vehicles carry no speed cap — the observed car spike was fragment
  blowback, now bounded at the source; other runaway paths (e.g.
  opponent recovery) are unguarded and unproven either way.
- Transient post-solve velocity spikes can still exceed the cap for
  the remainder of a single substep (the solve runs after the clamp);
  they re-clamp next substep and can no longer compound.
- Deep London/SF non-finishes, opponent skill vs retail, and any
  retail-fidelity comparison remain open (engine self-metrics only).
