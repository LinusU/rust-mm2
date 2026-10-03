# Parked cars (`<city>_parkedcar*.pathset`)

Kerbside parked cars come from the original's parked-car manager, one
of the per-city object managers created beside the drawbridges
(`0x413230`). Read from the retail `Midtown2.exe`
(**verified_original** unless noted):

- **When**: offline sessions always; networked sessions only outside
  cruise (`roam`) and cops & robbers (`multicop`) — `0x413276`.
- **File**: the shared object lookup — `race/<city>/<city>_parkedcar_<event stem>.pathset`,
  else `<city>_parkedcar.pathset` (`0x415950`, same `%s_%s_` /
  `%s_%s` formats as the bridges, `docs/research/drawbridge.md`).
  Retail: `london_parkedcar` (91 `PATHnn` strips),
  `london_parkedcar_crash0`/`_crash3` (Crash Course), `sf_parkedcar`;
  `london_parkedcar_test` and `.bak` are unreachable.
- **Spacing** (`0x579870`): the path's spacing, raised to 5.0 m when
  smaller, handed to the generic `dgPath` stamp (`0x466d30`) — the
  same expansion `mm2_game::path_stamp_sites` implements.
- **Pick** (`0x579950`, per stamp): `rand() % 3`; 0 leaves the bay
  empty, 1/2 place `giz_pcar01_l`/`giz_pcar02_l` (format
  `giz_pcar%02d_l`, so `giz_pcar03_l` is never placed by this
  manager). A placed car then takes `rand()` again as its paint
  variant (virtual `+0x14`).
- **Pose**: the stamp matrix times `RotateY(π/2)` (`0x4bd130`: rows
  `(c,0,−s) (0,1,0) (s,0,c)`) — the stamp runs local +X along the
  path, so the car's local +Z lies along it; position is the stamp
  point. The cars are banger instances (`giz_pcar0N_l` bind
  `tune/banger` records), knockable like any other.

## Implementation

`mm2_game::parked` holds the spacing floor, the pick and an MSVC
`rand()` LCG; `mm2_app::city::spawn_parked_cars` stamps through the
shared expansion and banger path with the rolled paint. The
original's `rand()` stream is shared with the whole game and its
position unknowable, so the rolls are seeded from the session seed —
**designed**: the same cars on every peer of a session, not the same
cars as the original. Measured on retail: london 604 cars / 320 empty
bays, sf 157 / 91 (seed 0).

Banger content offset follows the shared `+CG` convention
(`docs/research/banger.md`), like every other stamped banger.
