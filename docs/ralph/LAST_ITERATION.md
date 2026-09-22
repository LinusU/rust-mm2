# Last iteration — F18-A.1 weather/environment preset inventory

Iteration 42 on `ralph/night`, continuing from `881feee` (the F02-C.1
roster controls matrix + reverse-band repair — external review verdict
**pass**). This iteration picks up F18-A.1, the plan's listed
weather/lighting preset-inventory slice: F01-B and F06-B deps are
landed, and every stock environment file format is now parsed with an
original-content audit behind it.

## What changed

Six new pure parsers in `crates/mm2_formats` plus a new audit command:

- `sky.rs` — `.sky` one-line dome record (`<model> <hatY> <yMul>
  <rot>`); field names are R4-recovered `lvlSky` members, exact
  4-token arity, finite-float validation.
- `lighting.rs` — `.ltNN` presets on the shared `tune` grammar.
  `LightingPreset` decodes `Key`/`Fill1`/`Fill2`
  (`Heading`/`Pitch`/`Color`) + packed `Ambient`, classifies the
  `<weather>-<tod>` block name, and maps it to the measured
  `NN = tod×4 + weather` grid (`WeatherKind`, `TimeOfDay`,
  `preset_index`, `LIGHTING_PRESET_COUNT`). Unknown fields and
  off-grid names surface as validation issues.
- `ldef.rs` — `.ldef` bake-source path + integer rows, preserved
  verbatim (`texture_stem()` helper); semantics unrecovered.
- `cpvs.rs` — `PVS0` room-visibility tables. Corrected against
  reference code + retail bytes: `index_count − 1` lists, stored
  indices are *compressed* end offsets (index 0 implicit), fill runs
  count raw, literal runs count `control − 0x7F`. 2-bit-per-room
  visibility uses the mm2hook `IsRoomVisible` layout (`byte r>>2`,
  bits `2*(r&3)`) — verified 1340/1341 London rooms self-visible
  where the doc-derived offset reading yielded 888. Bounded decode
  (`MAX_LIST_BYTES = 8192`, `MAX_INDEX_COUNT = 1<<20`), `code`/
  `is_visible`/`visible_rooms`, `validate()` → `CpvsIssue`.
  `PvsHist` parses the `.pvshist` `from to weight` text table.
- `water.rs` — `.water` level + integer room refs.
- `lmap.rs` — `LMP0` + count + i32 entries, exact-length check.

`tools/mm2_inspect` gains `weather <install> [--city] [--strict]`:
census of all 102 discovered environment files (denominator never
filtered — the 38 numbered `.cpvs` variants, named `.ldef`s,
`city/phys/j01.sky` and `sf082100.*` are audited extras), per-file
parse with measured stats, and cross-checks: `.sky` dome →
`geometry/*.pkg`, `amb_<grid>.ldef` ↔ `texture/sky_<grid>.tex`
(measured 32/32 name alignment — flagged inferred), `.ltNN` name ↔
file slot + 16/16 coverage, and `cpvs`/`lmap`/`pvshist`/`water` room
references against `city/<stem>.psdl` (`lists == rooms + 1` on both
base tables). Authored anomalies are notes/findings, not issues —
stock retail `--strict` exits 0.

## Measured retail facts now on record

- `ltNN` grid resolves the `Weather`/`TimeofDay` column-order half of
  UNK-1: tod = morning/noon/evening/night, weather =
  clear/cloudy/foggy/rainy; every record's block name classifies to
  its own file slot on all 32 files.
- `.pvshist` is a `from to weight` text table (95,112 london /
  105,022 sf rows, weights saturating at 255). The archive entry is
  DAVE-compressed with ~1 MB trailing padding — storage detail
  `inflate_entry` already handles; the parser takes text. First
  attempt fed `vfs.read` output to a second `DeflateDecoder` and
  failed "corrupt deflate stream" — root-caused to the double-inflate,
  not the format.
- `sf.lmap` authors 1125 entries vs 1171 PSDL rooms (authored
  shortfall — note, not issue); entry 0 is a `0xCDCDCDCD` sentinel on
  both cities, preserved verbatim.
- `sf082100.{cpvs,pvshist}` is a near-duplicate variant (105,021 vs
  105,022 rows); `london_254`/`sf_00`/`sf_254` author fewer CPVS lists
  than `rooms + 1`.
- `.water`: london −3.8 @ rooms 345/351/356; sf −1.9 @ 228/399/401.
- `.ldef` integer pairs: only two signatures ship (`-2300 2600 /
  500 -1800` on `_f` and named files; `-1500 1250 / 1500 -1250` on
  `_l`). The dev `.tif` paths are provenance only — never resolve, not
  redistributable.

## Tests

+9 unit tests across the six modules: retail-shape parses, wrong
arity/empty/non-numeric rejects, bad magic, oversized/truncated index
tables, non-monotonic indices, truncated RLE runs, decompressed-output
bound, unknown 2-bit codes + self-invisible detection, pvshist row
validation, non-finite level/angle and negative colour validation.

## Gates

- `cargo fmt --all -- --check` — pass.
- `cargo clippy --locked --workspace --all-targets --all-features --
  -D warnings` — pass (one `collapsible_if` fix).
- `cargo test --locked --workspace` — pass, 0 failures
  (136 `mm2_formats` lib tests incl. new).

## Evidence (retail `fnv1a64:e91e6cd4b2ae30d9`, 2026-09-22)

- `mm2-inspect weather /Users/linus/coding/rust-mm2/retail`:
  102/102 parsed, 0 unsupported, 0 failures, 0 issues; all 16/16 ltNN
  slots per city; `psdl` cross-checks london 1341 / sf 1171 rooms;
  `--strict` exit 0; `--city london --strict` exit 0.

## Remaining gaps (open — F18-A parent stays active)

- No runtime consumer: parsing is not rendering. `mm2_app` does not
  yet bind `.ltNN`/`.sky`/`.cpvs` to lights/sky/PVS (UNK-24).
- `.ldef` integer-pair semantics, `.lmap` value semantics, `.pvshist`
  weight consumers, `.water` ref kind, `amb_*`/`sky_*` letter-grid
  meanings all unverified (UNK-24).
- Which numbered `.cpvs` variant a weather/fog setting selects is a
  measured hypothesis, not a recovered rule.
- F18 reqs 1–7 remain open: preset→Bevy mapping, precipitation,
  surface wetness, session determinism, CLI/menu selection,
  unsupported-combination policy.

Files: `crates/mm2_formats/src/{lib.rs,sky.rs,lighting.rs,ldef.rs,
cpvs.rs,water.rs,lmap.rs}`, `tools/mm2_inspect/src/main.rs`,
`docs/research/environment.md`, `docs/original-rules.md`,
`docs/ralph/{PLAN.md,LAST_ITERATION.md}`.
