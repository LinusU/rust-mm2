# Drawbridges (`gizBridgeMgr`, `<city>_bridge*.pathset`)

London's Thames crossings at Tower Bridge and both Waterloo bridges,
the Tower of London gate drawbridge, and SF's Chinatown gate are not
PSDL geometry. The rooms under them (london 857, 939, 970) carry their
road behind a TextureRef of `0`, which suppresses render *and*
collision — the decks belong to `giz_*` leaves the original's
`gizBridgeMgr` places from a bridge pathset. Without the leaves a
race that crosses the river drops into it.

Everything below is **verified_original**: read from the retail
`Midtown2.exe` (`llvm-objdump`, image base `0x400000`; file offsets
are `VA − 0x400000` for `.rdata`/`.data`) and checked against the
retail data.

## Which file a session loads (`0x415410`)

The manager is created per city with the object name `bridge` and
default model `giz_bridge01_l` (`0x413244`). In a non-cruise game
type it first tries `race\<city>\<city>_bridge_<gametype><n>` — the
type format comes from the table at `0x5c46ac` (`race%d`,
`multicop`, `circuit%d`, `blitz%d`, `croam`, `crash%d`), i.e. the
event's own file stem — then falls back to `<city>_bridge`. Cruise
(`roam`) loads the default directly.

Retail files: `london_bridge` (default), `_race0`, `_circuit0`,
`_crash3`, `_crash5`, `_blitz10` (truncated, and unreachable —
London's blitz table ends at `blitz9`), `_multi` (no game type
formats to `multi`; unreachable); `sf_bridge`.

## Leaves from paths (`0x5777f0`, `0x577a90`)

Per path, a leaf is hinged at point 0 facing point 2 (point 1 on a
two-point path); paths of more than two points add a partner leaf
hinged at point 2 facing point 0, and the two link as partners. The
middle point is not read. The leaf frame is built in authored space:
`z = normalize(from − to)`, `x = up × z`, `y = z × x`, origin `from`.

The model is the path name after `PREFIX:`; when that names no
geometry the manager's default (`giz_bridge01_l`) is used.

## Pose (`0x5773a0`)

`world = frame · Rx(angle) · T(0, 0, −Size.z / 2)`, then a global
`(0, −0.3, 0)` offset (`0x6b3690`, set at `0x577170`). `Size.z` is the
leaf's `tune/banger` record (`[edi+0x30]` of the 0x154-byte record
table). The PKG mesh is drawn raw — centred, **no** `CG` offset —
so the leaf extends along local −Z from the hinge. Measured: each
leaf's `Size.z` equals its share of the path span (Tower Bridge 30 m ×
2 over 59.9 m, Waterloo 23.18 m × 2 over 46.4 m, Chinatown gate
11.83 m × 2 over 23.7 m), and the gate's centred mesh (±7.05 m)
stands on SF's road at 36.9 only from its 44.0 path point (−7.05
−0.3 → 36.65). The 0.3 m drop puts Tower Bridge's deck (+0.15) at
7.85 against its 7.8 approach.

## Modes and motion (`0x577c60`, `0x577450`, `0x577270`)

The first four characters of the path name, `stricmp`'d:

| Prefix | Mode | Behaviour |
| --- | --- | --- |
| `inac` | 0 inactive | resets closed, never moves |
| `prox` | 1 proximity | opens when a car is within 100 m (`d² < 10000`, `0x5daf8c`), triggering its partner too |
| `time` | 2 timed | **the default** (constructor `0x5771e3`): 10 s closed, open, hold 10 s, close, repeat |
| `open` | 3 open | resets raised, never moves |

Opening/closing rotates at 0.05 rad/s (`0x5daf7c`) between 0 and
0.4712389 rad ≈ 27° (`0x5daf80`); the waits are 10.0 s each
(`0x5daf84`, `0x5daf88`). A full timed cycle is ~38.8 s, and every
timed leaf starts its clock at session start, so they move together.
Retail authors no `prox`/`time` prefixes; unprefixed names (timed),
`open:`/`OPEN:` and `inactive:` occur.

The dock leaf (`giz_bridge02_l`, Tower of London gate) is `open:` in
every reachable file — a raised ramp out of the gatehouse — and the
Tower Bridge leaves are `OPEN:` for `circuit0`.

## Implementation

`mm2_game::drawbridge` holds the mode decode, hinge pairing and the
leaf state machine; `mm2_app::drawbridge` picks the file, spawns each
leaf as a kinematic body whose origin is the hinge (centre of mass
pinned there, the mesh offset baked into the vertices) and poses it
each fixed step, leaving the angular velocity that carries it to the
next step so cars on a moving leaf ride it. The session's
`DrawbridgeReport` resource records the file and leaf counts.

The motor loop and bell (`aud/ambient/drawbridge.csv`) sound while a
leaf moves — see `movers.md` § Object audio. Not reproduced: which
cars count for `prox` (the
list at `+0x24` of the manager is unidentified; every participant is
used — no retail path uses `prox`).
