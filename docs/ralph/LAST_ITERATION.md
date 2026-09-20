# Last implementation iteration

- Task ID and title: F03-B.5 — placement-height repair for the
  operator's play-test report ("all of the stuff is spawned in at the
  wrong height"; a sawhorse struck at 100+ km/h did not move).
- Starting commit and resulting commits: started at
  `ff7ab7effc224b2ade61c90c54e22d21ab353491` (the operator-report
  commit; branch `ralph/night`, clean tree).
- Why this slice: the operator report is a PRIORITY entry in PLAN.md
  and outranks every queued feature candidate — it is direct
  observation of rendered gameplay on retail content, and two
  recorded hypotheses (slope-tumble, hull clearance) were built on
  the misplaced geometry it describes.
- Diagnosis (measured, not inferred): a temporary `mm2_inspect`
  probe dumped `dgBangerData` `Size`/`CG` against PKG vertex AABBs on
  the retail install. Prop meshes are authored **centred at the bound
  centre** (e.g. `sp_sawhrslt_f` y ∈ [−0.727, +0.978]); `Size` is the
  bound's **full** extents, `CG` is the bound centre, and `CG.y =
  Size.y/2` on every measured record (cone 0.425/0.85, sawhorse
  0.727/1.453, tree 3.5/7.0, streetlamp 3.862/7.702, tptpole
  6.151/12.309, `ghirardelli` sign and BREAK fragments too). The
  authored stamp point is where the bound's *base* rests — so `mesh +
  CG` lands inside `CG ± Size/2` with its base at y=0. `city.rs`
  stamped the mesh centre at the point: every stamped prop sank by
  ~half its height, matching the report exactly (the sawhorse's bound
  was already under the road, hence immovable).
- What landed (code):
  - `mm2_app::city::PropOffset` — `Verbatim` (INST), `Bound(Vec3)`
    (`+CG` for stamped bound names), `Ground` (`−min_y`, lift-only,
    for unbound stamped names). Offset is baked into render vertices
    and collision accumulation in `emit_strip`, and into BREAK
    fragment pieces via the new `fragment_pieces` resolver (parent
    content offset applied to fragment chunks). `PropCache` keys now
    carry the offset class — one pkg name can reach all three
    channels with different offsets.
  - `stamp_pathset` resolves the banger record *before* fetching the
    model (so the `CG` is known); `stamp_prop_rules` does the same;
    INST stamping passes `Verbatim`.
  - `mm2_app::banger`: `mirrored_cg` is `pub(crate)` for city.rs;
    the bound-strike path measures reach as half the max `Size` axis
    (`Size` is full extents, not half) and aims at the bound centre
    (`position + rot·cg`); the spin-kick lever is measured from the
    centre of mass rather than the body origin.
  - `mm2_game::banger` + `mm2_formats::banger`: `Size`/`CG` docs
    corrected to the measured convention (full extents / bound
    centre); `angular_kick` treats `size` as full extents (inertia
    dims were doubled — now corrected, kick is ~4× stronger).
  - Docs: `docs/research/banger.md` records the measured convention
    with the retail table and the operator-visible defect;
    `pathset.md`/`proprules.md` note the base-on-point convention;
    `PLAN.md` marks F04-C.2/-C.3 retail evidence for re-take.
- Tests added (in `crates/mm2_app/tests/banger.rs`):
  - `stamped_props_rest_their_bounds_on_the_path_point` — synthetic
    city through `load_city` + real Avian: bound pathset prop's
    `ColliderAabb` base rests on the authored path point
    (centred-fixture pkg), unbound prop's lowest vertex rests on the
    point, INST prop stays verbatim (fixture authored base-at-origin
    — a `Bound` offset would have lifted it).
  - Two existing settle-window tests widened (10 s → 30 s budget;
    they settle in ~0.9 s) because the corrected kick is ~4×
    stronger — physically right, not a regression.
- Retail evidence (install `/Users/linus/coding/rust-mm2/retail`):
  - `mm2 --mm2-path <retail> --city london --headless` — `status=pass`,
    counts unchanged (1997 INST / 1188 pathset props+bangers /
    5083 prop-rule stamps+bangers, 0 unresolved, 0 decode failures).
  - `mm2 --city london --cam=<park view> --frames 90 --screenshot`
    → `/tmp/props-fixed-london-trees.png` — trees rooted on grass,
    bench on path, lamps upright, post boxes standing (before:
    `screenshots/pathset-london-trees.png` showed buried shrubs).
  - `mm2 --city sf --cam=<lamp view> --frames 90 --screenshot`
    → `/tmp/props-fixed-sf-lamps.png` — lamps full height with arms
    over the road, hill trees full (before:
    `screenshots/pathset-sf-lamps.png` showed poles buried to their
    arms, stump trees).
- What this proves / does not prove:
  - Proves: stamped-prop heights now match the authored bound
    convention on all three channels; rendered evidence on both stock
    cities; collider AABBs (not just render meshes) rest on the
    authored points; INST untouched.
  - Does not prove: the original used the identical convention for
    every prop (measured on ~a dozen records incl. fragments, not all
    994); whether prop-rule stamp Y is curb-top or lerped (cm-class,
    still open); the F04 activation/settle evidence — measured on
    sunk geometry and owed a re-take.
- Classification: `Size`-full-extents / `CG`-bound-centre /
  base-on-point is a verified_original measurement of authored data
  (WLD-15 extended in banger.md); `PropOffset`, the `Ground` lift
  policy for unbound names, and the CoM-relative kick lever are
  implementation choices.
- Commands actually run and results:
  - `cargo test -p mm2_app --test banger` — 17/17 pass.
  - `cargo fmt --all -- --check` PASS; `cargo clippy --workspace
    --all-targets --all-features -- -D warnings` PASS (after deleting
    the temporary `probe_height.rs`, which tripped lints and was
    measurement-only); `cargo test --workspace` PASS.
  - Retail runs as recorded above; physics smoke `status=fail
    never grounded` on london — pre-existing, unrelated to this fix
    (counts/visuals clean); recorded, not attributed.
- Acceptance IDs satisfied / still open: repairs the F03-B stamped-
  placement contract on real data; F04 stays `implemented` — C.2's
  settle/pool/repeat evidence and C.3's ghosted-prop evidence must be
  re-run on corrected geometry before any rollup.
- Deferred deliberately: F04-C.2/-C.3 retail re-take (next
  iteration's named action); per-prop verification of the remaining
  ~980 records (audit-scale, not needed to land the fix).
- Stock data/GPU/audio/network limitations: screenshots are local
  captures of our renderer on retail content — no original-executable
  comparison; no GPU break capture; no audio/network coverage.
- Unresolved blockers or discovered regressions: none known from
  this change; the london headless physics smoke's `never grounded`
  predates it.
- Next smallest useful action: re-run the F04-C.2 `--spawn` evidence
  commands on the fixed placement (London bollard row, SF wood
  barricade, cone cluster); then return to queued work (F13-A
  remainder / F14-A / F11-C).

This is a candidate handoff. External code-gate and separate review
results live in the runner state directory and are not implied by
this report.
