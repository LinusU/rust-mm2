# Last iteration — F18-A.7: SDL deadly-water marking + environment research closure

Feature iteration on `ralph/night` (baseline 3cb3453, external review
of the F18-A.6 repair passed). Selected the F18-A remainder: the last
open research questions (`.cpvs` variant selection, the `.water`
"[from SDL]" source) and the one missing consumer they implied.

## Evidence gathered (retail Midtown2.exe + data)

- **`.cpvs` variants are bake sweeps, not runtime files.**
  `cityLevel::Load` opens exactly one PVS stream —
  `datAssetManager::Open("city", stem, "cpvs")` at VA 0x4440BE; the
  `"cpvs"` string at 0x5C485C is a bare extension and no `%s_%d`
  numbered-name construction exists in the loader. Measured on the
  data: `sf_8`/`london_8`/`london_bad` are byte-identical to the base
  tables, and the numbered files nest
  `_0 ⊋ _2 ⊋ _4 ⊋ _8=base ⊋ _16 ⊋ _32 ⊋ _64 ⊋ _128 ⊋ _255` (per-list
  visible-room sets; larger N = stricter occlusion, far-room
  visibility survives — not distance cuts). `_00`/`_254` are
  independent tables, `sf082100` a strict subset (earlier bake). The
  shipped table is the N=8 bake.
- **`.pvshist` is bake-tool data** — no `pvshist` reference exists in
  the exe's strings or loader (negative evidence, string-search only).
- **`.lmap` is runtime-loaded** — `wrong lightmap version` /
  `room count mismatch` diagnostics sit in the city loader. The i32
  values' semantics stay open (UNK-24); still unconsumed.
- **Deadly water has two exe-verified sources.** `cityLevel::Load`
  logs `"Room %d has Water of Death(tm)"` from `[from .water file]`
  (each ref bounds-checks `0 < ref < nRooms` and sets the room's
  runtime water flag — mm2hook `RoomFlags::Water` = 0x4; the six
  retail ref rooms already carry 0x4 in stored `room_flags`) and
  `[from SDL]` (a room whose FIRST attribute is a `TextureRef` to a
  liquid-class texture is marked). `GetWaterLevel` returns the one
  authored level; the kill is `flagged && pos.y < level`. The
  per-texture class flag is populated at material bind and its exact
  derivation is unrecovered — measured on retail it lands exactly on
  `deepwater`-mapped surfaces (42 sf `s_ocean` rooms, 20 london
  `s_thames`); `water`-mapped `s_water`/`s_pond` (drag 0.119) never
  mark.

## What changed

- `mm2_formats::psdl`: `texture_ref_index` (the shared
  `data + 256*subtype − 1` TextureRef decode — `city.rs`'s
  `decode_texture_ref` now delegates) and the recovered `RoomFlags`
  bit names documented on `Psdl::room_flags`.
- `mm2_content::surface`: `SurfaceTables::is_deadly_surface` — a
  texture resolves to a drowning-class material when its authored
  `drag` meets `RecoveryPolicy::water_min_drag` (0.3): retail
  `deepwater` 0.5 qualifies, `water` 0.119 stays wadeable — the same
  boundary the wheel classifier applies, and it reproduces the
  retail mark set exactly. Documented as inference (the class-flag
  derivation is unrecovered), not a claimed original test.
- `mm2_app::water`: `CityWater::build` gains a `surfaces` parameter
  and appends SDL marks — rooms whose first attribute is a
  `TextureRef` to `is_deadly_surface`, bound at the authored `level`
  verbatim (the verified global bound; refs keep their designed
  `max(level, room-top)`). A room already ref'd isn't double-marked.
  `sdl_rooms()` reports the split.
- `mm2_app::city`: `load_city` passes `surfaces.as_ref()` into
  `CityWater::build`; the info log gains `sdl=`; the `.cpvs` comment
  now records the verified single-file load.
- `mm2_app::smoke`: `wtr=<level>/<refs>r(+<n>sdl)(+<n>s)` — the
  ref/SDL split is recorded like the exe logs it.
- `mm2-inspect weather`: each non-base `.cpvs` is classified against
  the base table's decoded visibility (identical / strict subset /
  strict superset / independent — note lines, never issues), and each
  city reports its SDL-marked room count.
- Docs: `environment.md` records all four findings + the audit's new
  checks; `original-rules.md` WLD-23 rewritten with the exe-verified
  marking/load facts, DSN-34 extended (marking verified; exposure
  shape and the elevated-ref bound stay designed), UNK-24 narrowed
  (`.lmap` values, `.ldef` pairs, letter grid, dome fogging and
  `sf_fog_orig.csv` remain open).

## Tests

+5 in `water.rs`: first-attribute `TextureRef`→deepwater marks at the
level bound; shallow `water` never marks; a non-first `TextureRef` is
ignored; a ref+SDL room keeps its elevated ref bound (dedup); absent
surface tables skip the SDL pass while refs still apply. Existing
`build` callers pass `None` (unchanged behaviour).

## Verification (this tree)

- `cargo test -p mm2_app water` — 9/9 pass, incl. all five new SDL
  tests; `import_pipeline` water tests still pass.
- Retail headless (fingerprinted install): sf → `wtr=-1.9/3r+42sdl`,
  london → `wtr=-3.8/3r+20sdl`; `deadly-water record loaded … sdl=42`
  / `sdl=20` in the load logs.
- `mm2-inspect weather <retail>` — 105/105 parsed, 0 issues; all 23
  variant relations printed (matching the measured chain above) and
  `water: sf — 42 SDL-marked room(s)` / `london — 20`.
- Gates (2026-09-23, this tree): `cargo fmt --all -- --check` clean;
  `cargo clippy --workspace --all-targets --all-features --
  -D warnings` clean; `cargo test --workspace` — 67 suites, 0
  failures.

## Not done / open

- The per-texture liquid-class derivation the exe's SDL pass reads is
  unrecovered; `is_deadly_surface`'s drag threshold reproduces the
  retail set but is inference. UNK-24 keeps `.lmap` value semantics,
  `.ldef` pairs, the `amb_*`/`sky_*` letter grid, dome fogging and
  `sf_fog_orig.csv`.
- `.water`-absent cities get no `CityWater` even though the original
  would still SDL-mark rooms — documented design choice (the authored
  level is the bound; no record, no level to bind).
- The room-scoped exposure overlay (point-in-perimeter + bound) and
  the `max(level, room-top)` elevated-ref bound remain designed
  policy (DSN-34); the marking set and level bound are verified.
- The exploratory `pvs_probe.rs` measurement file is deleted; its
  findings live in the audit + docs instead.
