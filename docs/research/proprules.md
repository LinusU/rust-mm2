# Roadside-prop rule tables (`propdefs.csv`, `proprules.csv`, `props.csv`)

The third placement mechanism after INST and pathsets: each PSDL room
carries a `prop_rule` byte (parsed as `Psdl::prop_rules`), and these
CSV tables turn that byte into the props stamped along the room's
perimeter — street lights, trees, parking meters, mailboxes, benches,
trash cans, phone booths. This is the sidewalk dressing that lines
every street block.

Measured on all 16 retail files (2026-09-20):

- `city/<city>/propdefs.csv` — named prop prototypes (london 16, sf 33).
- `city/<city>/proprules.csv` — `n{NN}left`/`n{NN}right` rows listing
  propdef names (london 16 numbers × 2 sides, sf 20 × 2).
- `city/<city>/props.csv` — `Group,Name` membership list; retail uses
  only the `Races` group (london 19, sf 16 props).
- `city/props.csv`, `city/london/props.csv.txt`,
  `city/sf/propdefs02.csv.txt`, `city/phys/*`, `city/sf/bak/*` —
  top-level/dev/snapshot copies audited the same way.
- `geometry/props.csv` — a *different* table sharing the basename:
  per-PKG LOD triangle counts (`Name,H Tris,M Tris,L Tris,VL Tris`, 8
  `va_*` traffic vehicles on retail).

## Grammar (verified against every retail file)

`propdefs.csv`: header `name,start,distance,maxUse,minLerp,maxLerp,`
`file1,file2,file3,file4` (trailing empty columns common), then rows
`name,int,int,int,float,float[,pkg…]`. The file columns hold 1–4 PKG
basenames; empty cells are dropped.

`proprules.csv`: header `rulename,prop1,…,prop8`, then rows
`rule,propdef…`. Every retail rule name fits `n{NN}left`/`n{NN}right`.

`props.csv`: header `Group,Name`, then `group,pkg-basename` rows. The
`city/phys/` dev copy mislabels the header `name,start` while writing
the same `group,name` rows — the parser tolerates any ≥2-column header
and preserves the labels.

`geometry/props.csv`: `Name,H Tris,M Tris,L Tris,VL Tris`, PKG
filenames *with* the `.pkg` extension. The four counts are not ordered
by magnitude on retail (`va_bus_f.pkg` = 68/178/78/12), so they are
preserved as authored columns, not reinterpreted as sorted LODs.

## The `prop_rule` byte ↔ rule-number link

`Psdl::prop_rules` distributes exactly over the defined rule numbers:

- london: bytes 1–16 used (415 rule-bearing rooms of 1342; 927 rooms
  carry 0 = no roadside props) ↔ rules `n01`–`n16` all defined.
- sf: bytes 1–20 (397 rule-bearing of 1172) ↔ `n01`–`n20`; `n14` is
  defined but referenced by no room.
- Both cities have **one room carrying byte 205** with no `n205` rules
  — an authored anomaly, reported as an audit issue (same class as
  `sfai.bai`'s room-ref-0).

So byte `N` selects the `n{NN}left`/`n{NN}right` row pair; which side
of the room each applies to and where on the perimeter props land is
the unverified part (UNK-21).

## Field semantics — inferred, not verified (UNK-21)

| Column | Retail values | Status |
| --- | --- | --- |
| `start` | 1–20 | inferred perimeter offset (m) before this prop may be placed |
| `distance` | 1–90 | inferred spacing (m) between successive placements |
| `maxUse` | 1–9999 | inferred per-room placement cap (`9999` ≈ unlimited) |
| `minLerp`/`maxLerp` | 0.1–0.5, always equal | unknown — a lerp range that never varies suggests scale/position jitter, but nothing verifies it |
| `file1`–`file4` | 1–4 PKG names | inferred variant pick list (`streetree` lists `sp_tree1_s` four times on both cities — either weighting or a dev stub) |

`props.csv` group meaning is likewise unverified: `Races` groups the
props race events place (barricades, cones, parked cars) plus some
generic dressing; whether the original uses the table for anything
beyond bookkeeping is unknown.

## Audit results (`mm2-inspect proprules`, retail 2026-09-20)

- **16/16 files parse** (6 expected + 10 extras), zero unsupported.
- Rule→def references: every `proprules.csv` prop ref resolves to a
  sibling `propdefs.csv` entry — including `city/sf/bak/`'s `speed`
  def, which exists only in the `bak/` table.
- Dead PKG refs (authored anomalies, reported): london `props.csv`
  `sp_bollard_pedsafe_l` (a `sp_bollard_pedsafe.dgbangerdata` exists,
  the PKG does not); `geometry/props.csv` LOD row `va_garbagetruck.pkg`;
  all 41 `city/phys/` `*_m` file refs plus `sp_boxfruit_f`,
  `sp_fruitcard_l`, `sp_plantcard_l` in `phys/props.csv` — the same
  dead names the pathset audit reports for `city/phys/props.pathset`.
- `--strict` exits 2 (48 issues: 41 phys `*_m` file refs + 3 phys
  group entries, `sp_bollard_pedsafe_l`, `va_garbagetruck.pkg`, 2 ×
  rule-205). `--city london` → 4 files / 2 issues; `--city sf` →
  7 / 1.

## Runtime consumption

None yet. The mechanism explains what `Psdl::prop_rules` *means*; how
the original walks a room's perimeter (which edges get `left` vs
`right`, whether `start` measures from a fixed corner or each edge,
what `minLerp`/`maxLerp` do, how `maxUse` budgets are spent across the
rule's prop list) is unverified (UNK-21). Stamping ambient props from
these tables belongs to a later F03 slice once those semantics are
measured — against retail room geometry, not guessed.
