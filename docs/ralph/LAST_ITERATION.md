# Last implementation iteration

- Task ID and title: F09-A.1 — parse and validate the `CAI1` BAI
  ambient-navigation container (roads, intersections, per-room culling)
  with independent fixtures and a `mm2-inspect` audit command.
- Starting commit and resulting commits: started at
  `dcb17ab1e11664691460975777f6afee9b7ea002` (clean tree, branch
  `ralph/night`, F12-C scripted-completion leg externally checked);
  result = the commit on top of it.
- Why this slice: PLAN named F09-A as a ready alternate, and it is the
  cleanest dependency-wise — both inputs (F00-B, F01-A) are externally
  `checked`, while F13-A's deps (F02-B, F11-B) are still candidates.
  It also unblocks F10 (traffic), F15 (opponents) and eventually
  route-aware driving for the bot, which is the main honest gap in the
  64-event retail matrix. F09-A is split: A.1 = BAI parser + audit
  (this iteration), A.2 = `.aimap` override parser (queued).
- Production code changed:
  - `crates/mm2_formats/src/bai.rs` (new): `Bai::parse` for the `CAI1`
    container — `roads` (id, flags, PSDL room refs, half-width, base
    speed, per-side `RoadSide` lane/tram/train/sidewalk counts +
    `ambientTypes`, per-section centre frames, `RoadEnd` junction
    records incl. vehicle rule and traffic-light pose), `intersections`
    (id, room, centre, counterclockwise road refs), `culling` (per-room
    large/small "AI bubble" road lists). Typed helpers keep raw codes
    uninterpreted (`ambient_type()`, `vehicle_rule()`,
    `is_connected()`, `FLAG_*`/`KNOWN_FLAGS`); undocumented payloads
    (`misc[40]`, fill fields) are preserved raw. Sanity caps on every
    count (retail maxima measured: 86 sections, 5 curves/side, 42
    rooms/road). Trailing bytes are an error — the format accounts for
    every byte. `Bai::validate()` reports `BaiIssue` diagnostics
    without rejecting the file: duplicate ids, unknown
    flag/ambient/rule codes, <2 sections, room-0 refs, dangling and
    back-reference-mismatched end↔intersection links, dangling
    intersection/culling road refs.
  - `crates/mm2_formats/src/lib.rs`: `pub mod bai`.
  - `tools/mm2_inspect/src/main.rs`: new `bai` subcommand
    (`[--city] [--strict]`) — expected denominator is
    `city/{london,sf}.bai`; every other discovered `city/*.bai` is
    audited as an extra (parse failures labeled `unsupported`, never
    hidden); parsed files get `validate()` issues printed plus a room
    cross-check against the same-stem `city/<stem>.psdl` (culling room
    count = rooms+1, road/intersection room refs in range).
    `.bai` added to `scan`'s recognized formats — the `_sup` files now
    appear there as honest parse failures.
- Format research (docs/research/bai.md): the R3 community doc's
  overall structure verified, with **one measured correction** — inside
  a road side, the `[lanes+sidewalks][sections]` cumulative-distance
  matrix is stored *before* the per-curve outer-edge distances, not
  after. Doc order produces a non-monotone matrix and strands the edge
  values; measured order parses all five retail files byte-exact.
  Also measured on retail: ids are sequential (refs readable as
  indices — ambiguity documented), every connected end's
  `intersectionRoadIndex` points back at its own road (757/757 SF,
  1077/1077 London), every intersection road ref is named back by the
  road, culling covers PSDL rooms+1 with list[0] empty.
- Tests added and why (`bai::tests`, 7): minimal fixture round-trip
  incl. lane-matrix/edge ordering and end wiring; bad magic; mid-file
  truncation → `UnexpectedEof`; trailing byte → `Parse`; implausible
  `nSections` → capped `InvalidValue` (no giant alloc); `validate()`
  catches a dangling intersection road ref and a dangling culling ref;
  `validate()` catches an end↔intersection back-reference mismatch.
- Commands actually run and results (this machine, macOS arm64):
  - `cargo fmt --all -- --check` — PASS.
  - `cargo clippy --locked --workspace --all-targets --all-features
    -- -D warnings` — PASS (exit 0; one collapsible-if fixed).
  - `cargo test --locked --workspace` — PASS, all groups 0 failures
    (mm2_formats lib now 67 tests incl. 7 new).
  - `mm2-inspect bai /Users/linus/coding/rust-mm2/retail` — exit 0:
    london.bai 540 roads/328 intersections/culling 1342 rooms +
    sf.bai 379/214/culling 1172 parse byte-exact, `validate()` clean,
    room refs in range vs their PSDLs; extras london_bak/sf_bak clean;
    sfai.bai (dev/test map) parses with 1 authored anomaly
    (intersection room ref 0, also out of sfai.psdl range — reported);
    london_sup/sf_sup `unsupported` (CAI1 magic but a different record
    layout under either field reading — uninvestigated, reported).
  - `mm2-inspect bai <retail> --strict` — exit 2, "0 failures, 2
    issues" (both on the sfai extra; expected files clean).
- Acceptance IDs satisfied / still open:
  - F09-AC02 advances: road ids, room refs and end/intersection/culling
    references are validated by `Bai::validate()` + the audit command,
    and unsupported records (the `_sup` files) appear in the audit.
  - F09-AC01 advances at parse level only: synthetic fixtures exercise
    legal directed structures; "legal directed routes" need F09-B
    queries — stays open.
  - F09-AC03/AC04/AC05/AC06 unchanged (queries, overlays, routing —
    F09-B scope).
- Stock data/GPU/audio/network limitations: this is parser + audit
  evidence — no runtime consumer exists yet (F09-B), so lane direction,
  light semantics and bubble behavior remain unverified (ledger UNK-12
  updated to reflect that the structure parses while semantics stay
  open; UNK-11 corrected — `.opp` parses since F11-A). The `_sup` BAI
  variant layout is genuinely unknown and reported `unsupported`, not
  guessed. `sfai.bai`'s room-0 anomaly is reported, not repaired.
  No rendered/audio/network evidence this iteration.
- Unresolved blockers or discovered regressions: `scan` now counts the
  two `_sup` BAI files as parse failures instead of unsupported-
  extension entries — a deliberate honest-reporting change, noted here
  so it is not mistaken for a regression.
- Next smallest useful action: F09-A.2 (`.aimap` parser — sections
  observed: `[Speed Limit]`, `[Exceptions]`, `[Police]`, `[Opponent]`,
  `[Ambient Types/Density]`, `[Ambients Drive On The Left]`,
  `[GoodWeatherPedName / BadWeatherPedName]`). Alternates: F13-A
  (checkpoint rules — deps F02-B/F11-B are candidates, not checked) or
  F03-A (prop audit).

This is a candidate handoff. External code-gate and separate review results live in the runner state directory and are not implied by this report.
