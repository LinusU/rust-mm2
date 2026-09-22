# Last implementation iteration

- Task ID and title: F10-B.8 — union of player interest areas for
  ambient spawn/recycle (the F10-B plan remainder's "multiplayer
  union-of-interest bubbles" line; F10 spec req 2 "use the union of
  player interest areas in multiplayer", AC04's "preserve active
  interactions near any player", the "two players far apart" edge).
- Starting commit: `c5811a82f3c27105c36e56b6488df67dfc2d9d40` on
  `ralph/night`; tree was clean, previous external review verdict
  pass (F10-B.7), so this is feature work, not a repair.

## What changed

- `mm2_game::traffic` — the spawn band is no longer a single circle
  around `player_at`. `plan_ambient`/`draw_spawn` take
  `interest: &[[f32; 3]]` — the position of every player interest
  area — and two new public predicates carry the shared semantics:
  - `in_spawn_band(position, interest, policy)`: inside *at least
    one* area's `recycle_distance` AND outside *every* area's
    `min_player_distance` — a car may live in anybody's bubble but can
    never materialise next to anybody. Empty `interest` and
    non-finite positions admit nothing.
  - `within_interest(position, interest, policy)`: the matching
    any-bubble survival test the recycler collects against.
- `mm2_app::traffic::maintain_ambient` — builds the interest set live
  each tick from every `Player` participant's `Position` (the local
  vehicle carries `Player`, so the old `PlayerVehicle`-only query is
  gone): local driver, remote drivers, AI opponents alike — designed
  composition, since each participant can hold live interactions with
  ambient cars (corridor sensing, junction-box occupancy, collision).
  A car despawns only when past `recycle_distance` of *every*
  participant; respawn draws run through the same union band. No
  participants → the population freezes rather than draining (same
  freeze the old no-`PlayerVehicle` early return gave).
- `load_ambient_traffic` — `player_at: Vec3` → `interest: &[Vec3]`;
  the session passes the local spawn alone at load (every participant
  stages on the same grid), the maintainer rebuilds the union live.
- `docs/original-rules.md` — UNK-12 gains the F10-B.8 runtime note:
  interest composition (AI areas count) is a designed choice; the
  original's interest-area model stays unverified.

## Evidence

- `cargo test -p mm2_game --test traffic` — 33 pass (+2:
  `spawn_band_is_the_union_of_player_interest_areas` — per-area
  admission, a second area's `min_player_distance` vetoes a point
  inside the first's band, empty/non-finite sets admit nothing,
  `within_interest` any-bubble legs;
  `plan_populates_through_any_interest_area` — a far-away area
  covering no lanes still populates through the second area; an
  empty interest set spawns nothing).
- `cargo test -p mm2_app --test traffic` — 27 pass (+2:
  `a_remote_players_bubble_holds_traffic_the_local_player_left` —
  remote `Player` at z=−300, local teleported to z=2000: population
  alive, `recycled=0`; control without the remote collects every car;
  `respawns_draw_inside_a_remote_players_band` — after the local
  leaves, refills materialise only inside the remote's 60–400 m band).
- `cargo fmt --all -- --check` — PASS.
- `cargo clippy --locked --workspace --all-targets
  --all-features -D warnings` — PASS.
- `cargo test --locked --workspace` — PASS, 678 tests, 0 failures.
- Retail headless smoke (install `fnv1a64:e91e6cd4b2ae30d9`):
  - `--city sf --frames 600` → `status=pass … traf=16/16 sp=23 rec=7
    dead=0 uns=0 q=0 jq=5 stuck=0 kn=1 sig=647 sigd=3` — bit-identical
    to the F10-B.7 record (one player → one area, by construction).
  - `--city london --frames 600` → `status=pass … traf=16/16 sp=20
    rec=4 dead=0 uns=0 q=0 jq=4 stuck=0 kn=1 sig=828` — bit-identical.
  - `--city sf --event checkpoint:0 --bot --frames 1200` →
    `traf=3/3 sp=5 rec=2` under a spread opponent field (the union
    holds cars near far-away opponents) — `status=fail` only on the
    pre-existing, documented "fell through the world" SF sanity check
    on this event (bot-limited, unrelated to this change).

## Review-shaped repair folded in

- `respawns_reject_space_a_participant_occupies` (F10-B.4's test)
  parked its bare `Player` participant mid-network — under the union
  that position legitimately vetoes *every* refill through the
  min-distance leg before the occupied box is ever reached, which
  would have silently re-scoped the test. The participant now stands
  150 m south — inside the union, past every lane's exclusion — so
  the widened exclusion box remains the rejecting mechanism the test
  claims to prove.

## Still open

- Interest composition is designed: `PlayerControl::Ai` opponents
  hold interest areas too (each can pen ambient cars). In a race the
  union can span the whole route, so ambient density near the local
  player dilutes across participants — bounded by `max_active`,
  unverified against the original (UNK-12).
- "Preserving relevant interactions" is still distance-only: a wreck
  or queue outside *every* bubble is collected — no interaction
  exemption (documented approximation).
- Hysteresis remains implicit (spawn outer bound = recycle radius) —
  a car hovering at the boundary is collected once, not flapped.
- No multiplayer exists yet (F24+): the union is exercised through
  `PlayerControl::Remote`/`Ai` participants in tests, not networked
  clients.
- F10-B remains active: queue-through-intersection priority,
  collision fidelity vs AC03's checklist (player-hit feel, damage),
  original junction/spawn timing (UNK-12), signal-prop model fidelity,
  F10-AC evidence vs the spec, F10-C.
