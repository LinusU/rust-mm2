# Last implementation iteration

- Task ID and title: F03-B.3 — prop-rule stamping channel
  (PSDL `prop_rule` → `proprules.csv` → `propdefs.csv` → PKG/banger).
- Starting commit and resulting commits: started at
  `405a671a808fab23300da589739f75d95861e48c` (clean tree, branch
  `ralph/night`; F04-C.2 passed external gates + review).
- Why this slice: highest-value ready item on the plan — the second
  verified banger placement channel, blocked on UNK-21. Public sources
  (mm2hook, Open1560) only stub the prop-rule walk, so the semantics
  were recovered by measuring retail PSDL geometry with a temporary
  probe, then implemented as an inferred policy with the verified
  parts promoted to WLD-17. The probe was removed before commit.
- What landed (code):
  - `mm2_game::props` — pure `walk_prop_rules` over PSDL rooms/paths
    + rule tables. Per `RoomPath`: resolve `road_rooms` (non-room
    encoded values counted as bad refs), find the entry/exit
    crossings — `start/end_crossroads` curb pairs at path ends,
    widest neighbour-marked perimeter pair at interior room-to-road
    boundaries — then walk the two arcs between them as the sidewalk
    building lines. Side labels measured against travel direction;
    each side walked with the road on the walker's left;
    `start`/`distance`/`maxUse` scoped per def per side per room;
    placement = `lerp(curb, outer, (minLerp+maxLerp)/2)`; variant by
    deterministic hash; `PropStamp` carries room/side/def/variant/
    position/direction/ordinal. Output bounded at
    `MAX_PROP_RULE_STAMPS`; `PropWalkStats` + issue strings count
    no-rule rooms, missing rules/defs, bad refs, missing crossings,
    unreached rule rooms and cap hits — nothing dropped silently.
  - `mm2_app::city` — `stamp_prop_rules` resolves each stamp's PKG
    through the shared `PropCache`: bound names → `spawn_banger_prop`
    dormant bangers (identical to pathset stamps), unbound →
    `spawn_prop` statics; `CityReport` gains `proprule_*` counters
    and Display fields; walk issues logged with the sibling CSV path.
  - `decode_tex` clamps declared mip count to the size-supported
    maximum (`p_parkmeter_f.tex` declares 7 mips on 32×32 — a hard
    wgpu validation error once prop-rule props pulled the texture;
    now warned + clamped).
  - Tests: 4 `mm2_game::props` unit tests on a synthetic PSDL
    (single/multi-room sides + labels, start/distance/maxUse
    bounds, bad refs/undefined byte/unreached counting,
    interior-boundary marks) + one app test through `load_city` on
    a synthetic install (rule-bearing room → stamped prop entity +
    report counts).
- Retail evidence gathered (install
  `/Users/linus/coding/rust-mm2/retail`, fnv1a64:e91e6cd4b2ae30d9,
  `target/debug/mm2`):
  - `--city sf --headless` → `prop-rule props stamped rooms=345
    stamps=5002 bangers=5002 unresolved=0 no_crossing=0 bad_refs=109
    unreached=52 capped=0`.
  - `--city london --headless` → `rooms=410 stamps=5083 bangers=5083
    unresolved=0 no_crossing=0 bad_refs=0 unreached=5`.
  - `bad_refs` = encoded non-room `road_rooms` values (65 0xx range)
    on sf only; `unreached` = rule-bearing rooms no path traverses —
    both counted, not hidden.
  - Every prop-rule PKG resolves to a bound banger record on retail
    (consistent with WLD-16), so all stamps are dormant bangers.
  - Screenshots (local only): `/tmp/proprule_sf.png` — lamps line
    both sidewalks at authored ~29 m staggered spacing, banner arms
    over the road; `/tmp/proprule_sf2.png` — multi-room freeway
    parapet lamps (interior boundaries work);
    `/tmp/proprule_london2.png` — plaza phone booths, trees,
    bollards at curb edges.
- What this proves / does not prove:
  - Proves: the stamping *geometry* (WLD-17) — every rule-bearing
    room reached by a sane path resolves its crossings on both cities
    (`no_crossing=0`), multi-room interior boundaries resolve via
    neighbour marks, and retail-visual sanity holds. The channel is
    live end-to-end through `load_city` with honest counters.
  - Does not prove: the original's field semantics — UNK-21 stays
    open, narrowed to `start`/`distance`/`maxUse` scope (implemented
    per room-side), the `file1`–`file4` pick (deterministic hash,
    original seed unrecovered), `minLerp`/`maxLerp` meaning (midpoint
    lerp implemented), the left/right label convention and prop yaw
    axis, the 65 0xx `road_rooms` record kind, and the `props.csv`
    `Races` consumer. No comparison against an original-executable
    frame.
- Commands actually run and results:
  - `cargo test -p mm2_game --lib props::` — 4 pass; `cargo test -p
    mm2_app --test import_pipeline` — pass.
  - `mm2 --mm2-path <retail> --city {sf,london} --headless` — counts
    above; screenshots above.
  - Gates at the candidate commit: `cargo fmt --all -- --check`
    pass; `cargo clippy --workspace --all-targets --all-features
    -- -D warnings` pass, exit 0; `cargo test --workspace` pass —
    all suites, 0 failures.
- Acceptance IDs satisfied / still open:
  - F03-AC01/AC02 (all placement sources instantiated): the third
    verified ambient source now stamps; `props.csv` `Races` group +
    decal pathsets remain unconsumed. Open.
  - F04 (bangers): the prop-rule channel leg of the parent note is
    now covered — 5 002/5 083 retail placements bind as dormant
    bangers through the same path as pathset stamps. UNK-22
    thresholds unchanged. Open.
- Deferred deliberately: decal stamping research, `props.csv` group
  consumers, original variant-pick/lerp semantics (UNK-21),
  hull-clearance fidelity, `BirthRule`/audio/flash/decal banger
  effects, `Timer` despawn, replication (F26).
- Stock data/GPU/audio/network limitations: retail evidence is
  headless loads + local screenshots through the VFS; no
  original-executable comparison exists.
- Unresolved blockers or discovered regressions: none known.
- Next smallest useful action: decal stamping research (same
  measure-first approach), the hull-clearance fidelity question,
  F13-A or F09-C.

This is a candidate handoff. External code-gate and separate review
results live in the runner state directory and are not implied by
this report.
