# City special content (`mm2-inspect specials`)

F28-A coverage report. It inventories, per city, the moving and
interactive content that generic static geometry and ambient traffic do
not cover — from the installation, not from a list of remembered
landmarks — and says which of it has a runtime.

```sh
mm2-inspect specials <install> [--city london|sf] [--strict]
```

## What it measures

| Row | Source | Runtime |
| --- | --- | --- |
| drawbridge leaves | `race/<city>/<city>_bridge[_<stem>].pathset` | `mm2_app::drawbridge` (WLD-26) |
| sailboats / ferries / Underground | `…_{sailboat,ferry,train}[_<stem>].pathset` | `mm2_app::movers` (WLD-28) |
| parked-car strips | `…_parkedcar[_<stem>].pathset` | `mm2_app::city::spawn_parked_cars` |
| water / recovery rooms | `city/<city>.water` | `mm2_app::water`, `mm2_app::recovery` |
| rail curves | `city/<city>.bai` tram/train curve counts | measured only |
| cable car | `va_cablecar_f` assets + executable evidence | **none** — start sites recovered (4 in SF, 0 in London); motion unresolved (UNK-44) |

Every family file is found by name (default, `_<event stem>` overlays and
backup variants), parsed by the production `Pathset` parser, and each
path's model is resolved the way its manager resolves it (path name, else
the family default). An overlay is *reachable* when a catalogued event of
the city has that stem (`EventCatalog`); the report shows unreachable
leftovers but they cannot fail `--strict`. The expected default files are
`mm2_content::EXPECTED_SPECIAL_PATHSETS` (9 on retail); a city without
one — San Francisco has no Underground — is listed as absent, not
invented.

## Retail result (this checkout, `--strict`, exit 0)

- Expected default pathsets 9/9; 20 of 21 family files loadable.
  The one that is not is `london_bridge_blitz10` (truncated, and London's
  blitz table ends at `blitz9`, so nothing selects it — `drawbridge.md`).
  Two other overlays are unreachable but intact: `london_bridge_multi`,
  `london_parkedcar_test`.
- London: 4 bridge paths, 16 sailboat, 6 ferry (+6 per Crash Course 9/11/12),
  8 train lines, 91 parked-car strips; water level −3.8, 3 room refs.
  San Francisco: 1 bridge path, 16 sailboat (4 fall back to the default
  model), 1 ferry, 55 parked-car strips; water level −1.9, 3 room refs.
- Every path's model resolves. London's train paths are named for their
  lines, none a model, so all eight use `va_ug_l`.
- Rail curves: London 28 train curves on 28 of 540 roads, no tram; San
  Francisco 42 tram curves on 21 of 379 roads, no train.

## The one actor with no runtime: the cable car

`Midtown2.exe` creates cable cars during AI-map initialisation
(`"AIMAP.Init: Create the cable cars."` is pushed at `0x5358cf`; the
`va_cablecar_f` model name follows at `0x5359b5`; the manager's
`"Returning a NULL CableCar. Idx: %d"` accessor is at `0x534a3b`). The
model, its bound and its `aivehicledata` ship, as do the `cablecar*` and
`streetcable` audio clips, yet no pathset names a cable car and the model
is in neither city's ambient roster.

**Where the cars start (verified_original, read from the executable).**
The init walks every BAI intersection and calls `0x54a200(intersection)`,
which counts the intersection's listed roads that carry tram rails on
the side facing away from it (`0x549960`: a nonzero per-side tram-curve
pointer; the train-rail twin at `0x549990`/`0x54a290` serves the next
init step, `"AIMAP.Init: Create the subways."`). Exactly one such road makes
the intersection a **tram-line terminus** and yields `(road id, ±1
direction)`; zero or two-plus yield nothing. The init allocates one
`0x184`-byte cable-car object per terminus and constructs it
(`0x53f7c0`) from the model name, its index, that road id (`+0xd0`) and
the direction (`+0xd2`). `Bai::tram_termini` implements the selection.
Retail: San Francisco has **4** termini (21 tram roads, none one-sided),
London **0** — so London has no cable cars. The step only runs when a
flag in the init's parameter block (`+0x40`) is set; what sets it (per
city, per game type, or a detail option) was not traced, so whether
every San Francisco session spawns the four is **unknown**.

Still unrecovered: the cars' motion along the road chain (speed, stops,
turnaround, bell/start/stop audio triggers) and which in-memory side
`±1` names (immaterial on retail, where every tram road carries rails on
both sides).

Out of scope here: the object-audio tables (`drawbridge`, `ferry`,
`subwaycar`, `trolleycable`, …) are sound emitters covered by
`movers.md` § Object audio, and the static `giz_*` landmark models are
ordinary placed props.
