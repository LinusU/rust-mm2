# Last iteration — wire round-trip test for the networked breakdown
# derivation (new-run iteration 2)

Implementation iteration on `ralph/night` (baseline `a9633e3`).
Selection: the reviewer's verification gap that the predicted client's
Disabled-tier derivation was only exercised via `set_replicated`, not
through the encoded damage byte. Small, test-only, no behavior change.

## What landed

- `netdrive.rs` `damage_tail_encodes_the_authority_fraction` now
  decodes `encode_damage` output the way `apply_damage` does
  (`byte / 255` → `set_replicated`): a destroyed seat (255) lands back
  on `DamageTier::Disabled`; the 254 near miss does not.

## Evidence level

Synthetic unit test (encode → decode in-process). Still NOT done: a
two-process host+client leg, graphics/audio/original-content proof.

## Gates

fmt pass; clippy `-D warnings` clean; `cargo test --locked --workspace`
exit 0, no failures.

## Still open from report 6

Follow-up 2 (field races vs. networked Results deferral), follow-up 3
(F26-A replication of drawbridge/movers/parked cars/sounds), follow-up
1's two-process leg. Status: implemented (candidate), not independently
checked.
