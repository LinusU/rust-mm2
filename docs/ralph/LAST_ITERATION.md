# Last implementation iteration

- Task ID and title: F14-B.1 — live running order / place
  indicator. First slice of F14-B's "participant ranking" leg; it
  is also the remaining rank-presentation leg of F13-B, named by
  the selection policy after F13-B.1's standings landed.
- Starting commit and resulting commits: started at
  `9728bb440214d2b2f950cd03b0b0205b7fed0a59` (operator's docs-only
  commit on the externally checked `7756026` F13-B.1 handoff;
  branch `ralph/night`, clean tree).
- Why this slice: HUD-2 documents a place indicator, F13-B.1
  already delivered the *terminal* standings, and no live
  participant ranking existed while a race runs. The other
  candidates were blocked — opponent hooks need F15 — or mostly
  satisfied — F11-C's audit legs already exist.
- Retail install: `/Users/linus/coding/rust-mm2/retail`
  (`fnv1a64:e91e6cd4b2ae30d9`), all runs `--headless` on this
  machine's dev-profile binary.

## What changed

- **`mm2_game::race::live_order`** — a contract function that
  takes `(PlayerId, &RaceProgress, Vec3)` rows and returns ids
  best→worst (DSN-13, designed — HUD-2 names the instrument; no
  verified original rule describes its ordering):
  `Finished` participants lead ordered by their recorded
  `race_ticks` (the same key DSN-12's standings use, so the live
  order converges to the standings as everyone resolves); active
  participants (`Racing`/`AwaitingStart`) sort by progress —
  `Ordered`: `(lap, next)`; `AnyOrder`: cleared-gate count;
  progress ties sort toward whoever is closer (straight-line XZ)
  to their *own* current objective — `checkpoints[next]`, or
  `navigation_target`'s nearest remaining gate / armed finish for
  `AnyOrder`; `TimedOut` participants trail ordered by
  `race_ticks`; `PlayerId` breaks all remaining ties. The
  distance tie-break is a presentation heuristic, not course
  distance — results never consume this order (the ledger owns
  them).
- **`mm2_app::main` (HUD): live place indicator.** While the race
  is `Countdown`/`Running` the HUD shows `{ord} of {n}` for the
  local participant whenever ≥2 participants have a standing —
  ordinal-only for a lone driver (a place indicator is a
  competitive instrument). The terminal `FINISHED {ord}[ of {n}]`
  Results line from F13-B.1 is unchanged.
- **`mm2_app::smoke`: `pos={i}/{n}` field.** The headless record
  reports the local participant's live standing whenever one
  exists — distinct from `place=`, which is the ledger's terminal
  standings placing on a recorded result.
- **`docs/original-rules.md`:** DSN-13 added (live-order policy,
  designed).

## Tests

- `mm2_game/tests/contracts.rs` (+2):
  - `live_order_ranks_ordered_participants` — finished-first by
    ticks, progress score `(lap, next)`, distance tie-break to the
    next authored gate, `TimedOut` trailing, `PlayerId` fallback,
    input-order independence.
  - `live_order_any_order_ranks_by_objective` — cleared-gate
    count outranks distance; equal counts sort by distance to each
    participant's *own* nearest remaining gate (`navigation_target`
    reuse); armed finish used as objective once all gates clear.
- `mm2_app/tests/race.rs` (+1):
  `live_order_tracks_progress_and_locks_finished_places` — two
  participants through a 2-lap Ordered course via the production
  `advance_race` path: a remote's extra gate outranks the local's
  empty progress; equal progress ranks the nearer-to-gate driver
  first; the remote's lap-1 wrap outranks the local's proximity;
  the remote's finish locks 1st while the session stays `Playing`;
  the local's later finish resolves the live order into the
  ledger's standings `[remote, local]`.
  - Harness note: each `set_position` write also produces ghost
    segments back toward the previous `GlobalTransform` on the
    next fixed step (avian's `transform_to_position` copies
    `GlobalTransform → Position` whenever `Position` wasn't
    changed that tick — both sync directions are on by default).
    The test's waypoints therefore never park inside an un-cleared
    trigger radius, so every park-to-park path (real or ghost)
    sweeps only the gates it means to. This quirk affects all
    `set_position`-driven tests equally; the new waypoints are the
    fix, not a special-casing.

## Commands actually run and results

- `cargo test -p mm2_game --test contracts` — 12/12 pass (incl.
  both new `live_order` tests).
- `cargo test -p mm2_app --test race` — 33/33 pass (incl. the new
  production-path test).
- `cargo fmt --all -- --check` PASS; `cargo clippy --locked
  --workspace --all-targets --all-features -- -D warnings` PASS;
  `cargo test --locked --workspace` PASS (all groups incl.
  doc-tests, 0 failures).
- `mm2 --mm2-path <retail> --city london --event blitz:0
  --headless --bot --frames 2500` → `status=pass`,
  `phase=results race=Complete cp=3/3 results=1 tl=7.6s pos=1/1
  outcome=finished place=1` — the new `pos=` field end-to-end on
  real authored content (solo run → `pos=1/1`; a multi-place
  retail record needs F15 opponents, which do not exist yet).

## What this proves / does not prove

- Proves: a deterministic live participant ordering exists as a
  contract property for both progress rules (F14 req.: explicit
  start-lap/tie policy); the production driver keeps independent
  progress and the live order resolves into the authoritative
  standings (F14-AC03 ordering leg, test level); the HUD presents
  the local participant's place while racing and the smoke record
  carries `pos=` on real authored content.
- Does not prove: opponent-driven live ordering (F15 — exercised
  with synthetic remote participants; no AI exists); any original
  live-placing rule — the ordering is designed (DSN-13), not
  verified_original; the distance heuristic vs. the original's
  (unknown) placement metric — straight-line XZ is an explicit
  approximation; a full results screen (F17); HUD legibility
  in-game (no screenshot evidence taken — headless run only).
- Acceptance IDs: advances F13-AC03/F14-AC03's ordering legs and
  the HUD-2 place-indicator instrument; F13-AC04 needs F15
  opponents, F14-AC04/AC05/AC06 stay open.

This is a candidate handoff. External code-gate and separate review
results live in the runner state directory and are not implied by
this report.
