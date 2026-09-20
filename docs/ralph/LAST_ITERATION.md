# Last implementation iteration

- Task ID and title: F03-B.4 — decal pathset channel
  (`city/<city>/decals.pathset` → textured ribbon meshes, the last
  ambient placement source named in the F03-B research).
- Starting commit and resulting commits: started at
  `de55fbb2668bd0b6564bd2cce0f3e55840895574` (clean tree, branch
  `ralph/night`; F03-B.3 passed external gates + review).
- Why this slice: the plan's named next work — the one placement
  channel still counted-but-unstamped. Public sources (mm2hook,
  Open1560) only stub decal drawing (`sdlPage16::Draw` per room), so
  the ribbon semantics were recovered by measuring retail data, same
  approach as the prop-rule slice.
- What landed (code):
  - `mm2_formats::tex` — decoding refactored through
    `decode_rgba_impl(level, honor_palette_alpha)`; existing
    `decode_rgba` unchanged (palette alpha forced opaque) and new
    `decode_rgba_honoring_alpha` keeps authored palette alpha. P8
    unit test added.
  - `mm2_app::decals` — `stamp_decals`: pairs a `LineStrip`'s
    interleaved edge points into cross-sections (even index = one
    edge, odd = other — measured, WLD-18), joins consecutive
    sections into quads, `u` 0→1 across the pair, `v` = centre-line
    distance tiled per `spacing` (quarter-metre field; 0 → 5 m
    default, inferred), normals oriented upward, 2 cm lift + −1
    depth bias, lit double-sided material. Textures resolve through
    the shared `MaterialCache::get_decal` (same `png/ktx2/tga/tex`
    order; `.tex` via `decode_tex_with(..., true)`); alpha-bearing
    textures get `AlphaMode::Blend`. Geometry merges per texture
    stem into one entity each — render-only, no colliders,
    session-owned via `CityEntity`. `MAX_DECAL_QUADS` bounds
    expansion; every path lands in a counted class (ribbons /
    `PATHnn` labels / `giz_*` animated / PKG-prop / unresolved /
    empty / degenerate / odd-tail / skipped quads / capped / missing
    textures / issues).
  - `mm2_app::city` — consumes `<dir>/<stem>/decals.pathset` beside
    the PSDL (missing file non-fatal), report gains `decals` block
    in `CityReport` + Display. `props.pathset` keeps classifying its
    texture-named paths without stamping (27 of sf's 31
    `r4i_rails_f` entries are name+points-identical `decals.pathset`
    duplicates — measured authoring leftovers; stamping them would
    double-draw the rail street).
  - Tests: `mm2_app::decals` unit tests (section pairing, odd tail,
    cumulative-v distance) + `import_pipeline` end-to-end through
    `load_city` on a synthetic install (two ribbons → one merged
    entity, quad counts, all classification counters, blended
    material, no collider).
- Retail evidence gathered (install
  `/Users/linus/coding/rust-mm2/retail`, `target/debug/mm2`):
  - `--city sf --headless` → `decals ribbons=48 quads=223 entities=2
    labels=3 empty=1 odd=1 unresolved=0 missing=0 issues=0`.
  - `--city london --headless` → `ribbons=79 quads=85 entities=3
    empty=4 degenerate=1 odd=1 unresolved=0 missing=0 issues=0`.
  - Screenshots (local only): `/tmp/decal_sf_rails.png` — paired
    cable-car rail channels down the street;
    `/tmp/decal_london_xwalk.png` — zebra crossing with correct UK
    stripe orientation; `/tmp/decal_london_zigzag2.png` — faint
    zigzag line (authored ~29% alpha);
    `/tmp/decal_london_xinter.png` — translucent yellow junction
    wash (the texture is a mostly-uniform alpha film — the wash is
    the authored content, not a rendering defect).
- What this proves / does not prove:
  - Proves: the ribbon *geometry* (WLD-18) — the even/odd edge-pair
    reading is the only one producing coherent authored widths;
    texture contents corroborate the u-across/v-along axes; the
    channel is live end-to-end through `load_city` on both cities
    with honest counters and zero unresolved names.
  - Does not prove: the original's UV policy (v tiled per `spacing`
    vs stretched — inferred), u direction (could be flipped on some
    textures), exact blend/render state (palette-alpha blending is
    inferred from authored alpha values), whether prop-channel decal
    names render (leftovers currently not stamped), or the per-point
    `attributes` word (UNK-20). No original-executable comparison
    exists.
- Commands actually run and results:
  - `cargo test -p mm2_formats --lib tex` — pass; `cargo test -p
    mm2_app` — pass (incl. new decal tests).
  - `mm2 --mm2-path <retail> --city {sf,london} --headless` —
    counts above; screenshots above via `--frames 90 --screenshot`.
  - Gates at the candidate commit: `cargo fmt --all -- --check`
    pass; `cargo clippy --workspace --all-targets --all-features
    -- -D warnings` pass, exit 0; `cargo test --workspace` pass —
    all suites, 0 failures.
- Acceptance IDs satisfied / still open:
  - F03-AC01/AC02 (all placement sources instantiated): every
    ambient-city placement source now instantiates — INST, PSDL room
    props, `props.pathset`, prop-rules, `decals.pathset`. Remaining
    unconsumed pathset families need features, not stamping rules
    (`audio_pathsets/` → F07/F08, `giz_*` object sets → animated
    objects, `props.csv` `Races` consumer unknown). Open.
  - F03-AC03–AC06: unchanged by this slice.
- Deferred deliberately: decal UV/blend verification vs original
  evidence (UNK-20), `props.csv` `Races` group consumer, prop-channel
  decal stamping if original evidence appears, hull-clearance
  fidelity, F13-A/F09-C.
- Stock data/GPU/audio/network limitations: retail evidence is
  headless loads + local screenshots through the VFS; no
  original-executable comparison exists.
- Unresolved blockers or discovered regressions: none known.
- Next smallest useful action: the hull-clearance fidelity question,
  F13-A or F09-C.

This is a candidate handoff. External code-gate and separate review
results live in the runner state directory and are not implied by
this report.
