# Last iteration — networked breakdown (operator report 6
# follow-up 1): a remote human's wreck in Blitz/Checkpoint now takes
# the same five-second breakdown the host's driver pays; a predicted
# client derives its own dead interval from the wire (new-run
# iteration 1)

Implementation iteration on `ralph/night` (baseline `08f0a80`).
Selection: report 6's follow-up 1 was the first named networking
item and fit one change. **Networked policy (recorded decision,
DSN-68 extension):** in a `DisabledOutcome::Breakdown` mode every
human pays the dead interval — host driver and remote driver alike.
Cruise/Circuit/Crash Course keep their outcomes; AI wrecks keep the
in-place reset.

## What landed

- `mm2_app/src/damage.rs` `resolve_disabled`: a `PlayerControl::Remote`
  arm guarded on `disabled_outcome(mode) == Breakdown` inserts
  `VehicleBreakdown` (authority-owned; `resolve_breakdown` repairs it
  after `BREAKDOWN_SECONDS`, `sync_impairment` kills the engine in the
  host's sim). Other remote wrecks fall through to the in-place arm.
- `sync_impairment`: on a predicted session in a Breakdown mode, a
  seat whose replicated damage sits at the `Disabled` bound gets
  factor 0 — the client's derivation of the dead interval, no new wire
  field. The authority's repair drops the byte and lifts it.
- `netdrive::encode_damage`: byte 255 now means *destroyed* only; a
  total that merely rounds up to it encodes 254.
- Tests: `a_remote_driver_breaks_down_like_the_host_driver_in_blitz`,
  `a_predicted_client_derives_its_breakdown_from_the_wire_damage`,
  `a_predicted_cruise_client_at_the_bound_is_not_derived_dead`; the
  existing remote-wreck test moved to Circuit (the in-place arm); the
  `encode_damage` unit test gained the 254 case.

## Evidence level

Synthetic integration only (in-process, single app). The follow-up's
third bullet — a **two-process leg** proving host + client both show
the breakdown — is NOT done; the copies' smoke follows the replicated
damage byte already, but no process-level test asserts it. Waiting
loops there must spin under a deadline (report 6 item 4).

## Gates

`cargo fmt --all -- --check` pass; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` clean; `cargo test --locked --workspace` exit 0, 0 failed (app target 761 passed incl. 20 damage::). The `encode_damage` unit test's own pass line was not separately grepped in the log. No graphics/audio/original-content/two-process evidence.

## Still open from report 6

Follow-up 2 (field races on behind Results vs. networked deferral),
follow-up 3 (F26-A replication of drawbridge/movers/parked cars/
sounds), follow-up 1's two-process leg. Status: implemented
(candidate), not independently checked.
