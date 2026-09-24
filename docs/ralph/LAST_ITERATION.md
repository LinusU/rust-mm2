# Last iteration — F07-A.1: audio format discovery (waves + cardata)

New-task iteration on `ralph/night` (baseline `13ef019`, the reviewed
F10-B.11 repair). Selected F07-A.1 — the smallest ready leg of the
highest-priority queued task with all deps landed: every `aud/**` file
was unverified (`"no audio decoder exists"`), and audio is a required
product surface (engines, horns, impacts, surfaces, ambience, speech,
music). This slice lands the format/discovery layer only — **no
playback** (`bevy_audio` is still not enabled; voice lifecycle, mixing
and runtime semantics are F07-B+ and UNK-25).

## What landed

- `mm2_formats::wav` — bounded RIFF/WAVE parser: chunk walk with
  even-byte padding, `fmt `/`data` decode (PCM + tag reporting), unknown
  chunks preserved with order/offsets, `riff_form_type` exposing
  non-WAVE RIFF form words (DirectMusic classification without
  pretending to decode it), size-mismatch/duplicate/missing/trailing
  issue reporting, duration/frame/16-bit-sample helpers.
- `mm2_formats::cardata` — 12 path-dispatched grammars covering every
  authored `aud/cardata/**` + `aud/ambient/**` table:
  - car audio (`{player,opponent}/{vp*,default}.csv` + `copy of`/`*.wrk`
    work copies) — horn/clutch row + `Engine wave name` table; **three**
    authored column layouts ship on retail (10-col fade-window,
    9-col `Pitch divisor`, 7-col `Volume divisor`/`vol inverse RPM`),
    so rows are stored positionally with the header preserved verbatim
    and `EngineSample::column` resolves normalized names;
  - `engineparams{play,opp}.csv` — 83–97-byte MSVC `0xCD`/`0xDD` binary
    name prefixes preserved raw + nine headerless floats;
  - `default_impacts.csv` — `***`/`""""` separators, banger-name
    categories, `ENDOFDATA` with trailing-line counting;
  - `default_surface{dry,ice,wet}.csv` — tunnel index + positional
    surface rows (9-col opponent vs 12-col divisor/`for tunnels` player
    schema; skid bands claim *slippage* vs *speed* units per schema);
  - `*policesiren.csv` — `play time,next index` step chains (headers
    repeated per step), optional `Explosion wave name`;
  - ambient engine/horn speed-band tables, object audio +
    `VECTORPOINTS` emitter lists, `*ambientcontainer.csv` `file names`
    member lists, `shared/semidata`/`vehtypes`, suspension/tirewobble
    band tables.
  - Sentinels (`NOSOUND`/`NOTHING`/`ENDOFDATA`/`FALSE`/`NONE`) treated
    as no-wave references; `TableDiagnostic`s preserve malformed rows;
    `validate()` returns semantic issues separately.
- `mm2-inspect audio <dir> [--strict]` — full `aud/**` census over the
  production VFS: per-table summaries, PCM census by rate/channels/bits,
  per-directory wave counts, DirectMusic RIFF-form classification,
  deferred-text/binary CSV split for the F08 families, extras, a
  sample-reference→wave-stem cross-check, cardata↔`tune/*.info` roster
  coverage, and a failures/issues/quirks/findings split — `--strict`
  fails only on failures + issues (authored quirks like binary prefixes
  and declared-count drift are findings, not regressions).
- Inventory audio note updated (was "no audio decoder exists");
  `docs/research/audio.md` added; `docs/original-rules.md` gained
  AUD-1…8 + UNK-25.

## Retail evidence (`mm2-inspect audio <retail>`)

```
3256 files (37 dir entries), 2503 waves decoded, 103 tables parsed,
552 deferred text csv (spchdata 526 / creaturedata 21 / dmusic 5),
0 binary csv, 3 extras (.bat), 0 failures, 0 issues,
35 authored quirks, 1 finding, 1 dead ref
```

- Waves: all PCM — 2112 @ 11025 mono, 375 @ 22050 mono, 13 @ 48000
  mono, 3 stereo/other-rate; 240 902 980 B ≈ 10 235 s.
- DirectMusic: `DMSG`×62, `DMST`×17, `DLS `×14, `DMBD`×2 — all form
  words match their extensions.
- Sample refs: 159/160 resolve to wave stems; `skidflagstone` is a
  genuine dead authored ref (dry/wet surface tables).
- Coverage: every `tune/vp*.info` id has player+opponent cardata;
  `vpcaddie59` cardata is an orphan (no tune entry) — finding.
- Quirks: engineparams binary prefixes; ~30 `Num Engine Samples`
  declared-vs-authored drift (players declare 2 author 4; opponents
  declare up to 4 author 1).
- `--strict` exits 0 on retail → usable as a no-regression gate.

## Tests

- `mm2_formats` — 24 cardata tests (retail-verbatim fixtures per
  grammar, all three engine column layouts, quoted separators,
  ENDOFDATA trailing content, container lists) + wav tests (padding,
  truncation, dup/missing chunks, non-PCM, size mismatch, trailing).
- `mm2_inspect` — 4 synthetic-install audit tests (stem/rate-suffix
  rules, failure+mismatch reporting, roster coverage).

## Gates

`cargo fmt --all -- --check` clean; `cargo clippy --locked --workspace
--all-targets --all-features -- -D warnings` clean; `cargo test
--locked --workspace` green — 67 suites, 1025 tests, 0 failures.

## Not done / open

- No audio playback, voice lifecycle or `bevy_audio` — F07-A remains
  `active`; AC01–AC06 are playback/runtime ACs this slice does not
  claim. F07-B (RPM/slip/impact event wiring) is next under F07.
- `spchdata`/`creaturedata` cue grammars and DirectMusic decode are F08
  scope — classified but unparsed.
- All runtime semantics open under UNK-25 (fade-window driver quantity,
  divisor schemas, tunnel index, skid-band unit disagreement,
  impact category binding with `AudioId`=0).
- Parse-level evidence only — no audible/original-behavior proof
  exists or is claimed for this slice.
