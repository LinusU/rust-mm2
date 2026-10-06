# Cops & Robbers (multiplayer) — what the original ships and what it hard-codes

F27-A's rule matrix, split by *where the original keeps each fact*. Reading
method: `llvm`-style disassembly of the retail `Midtown2.exe` (image base
`0x400000`, `.rdata`/`.data` file offset = VA − `0x400000`, all addresses
below are VAs), string tables from the retail `mmlang.dll` (PE `RT_STRING`),
and the retail `race/`, `aud/spchdata/`, `geometry/`, `tune/banger/` and
`texture/` files through the VFS. `mm2-inspect cnr <install> --strict`
audits the data half (18 files + 14 cue families per city; retail passes for
both cities). Classes follow `docs/original-rules.md`: **verified** = read
directly from code/data, **inferred** = a reading of what was read,
**unknown** = not recovered.

The mode is the executable's mode-name table entry 2, `multicop`
(`0x5c46ac` table → `0x5c46d8`; entry 0 is `roam`, the multiplayer Cruise).
It is the same stem the parked-car and drawbridge notes already name.

## 1. Data the original ships (verified; audited by `mm2-inspect cnr`)

| File | Role |
| --- | --- |
| `race/<city>/multicopwaypoints.csv` | The pool gold, hideout and bank positions are drawn from. Ordinary headered waypoint CSV (`x,y,z,a,poly count,…`), **44 rows (sf), 46 rows (london)**. The executable builds the name as `%srace\%s\%swaypoints.csv` (`0x5c34c8`, loader `0x424870`) with the mode stem. |
| `geometry/wpobj_gold.pkg`, `pt_hideout`, `pt_bank`, `pt_red`, `pt_blue` | The five marker models. The names are literals in the executable (`0x5c3440…0x5c347c`, `0x5c3f08` beside the string `geometry`). |
| `tune/banger/<same five>.dgbangerdata` | A banger record per marker. What the record does for a marker (collision? pickup trigger?) is **unknown**. |
| `texture/{gold,hideout,bank,red,blue}_dot.tga` (+ `_ni`) | Map/HUD dots. Binding of dot to role is by name only (**inferred**). |
| `aud/spchdata/cnrsf.csv`, `cnrlondon.csv` | Commentary cue tables (grammar: `docs/research/audio.md`). Both author the same 14 cue families — see §4. |
| `jpg/<city>_multicop.jpg` | The mode's loading image. |

There is **no** `multicopsets.csv` on retail. The executable tries
`race\<city>\<stem>sets.csv` first (`0x42466f`: groups of three 16-byte
vectors, read in the order bank, gold, hideout, placed by `0x424e30`) and
falls back to the waypoint pool when that yields no rows
(`0x423cea`). So the retail game always takes the pool path, and a mod can
supply `multicopsets.csv` to author fixed gold/bank/hideout triples — **the
sets path is read from code, not observed on any shipped file**, and this
repository does not implement it.

### How a round picks its sites (verified from code, *not* reproduced)

`0x424be0` draws three positions from the pool with the engine's `rand()`
(`0x424cc0`: `index = rand() % (rows − 1)`), one each for the gold
(`+0xb20c`), the bank marker (`+0xb204`) and the hideout marker
(`+0xb200`). A re-draw until the three differ only runs when a byte at
object `+0x276` is set; what sets it is **unknown**. A pool with fewer than
three rows disables the pool (`0x4248c4`) and the original falls back to an
arbitrary world position (`0x413b80`). Whether the `rows` in the modulus
counts the CSV header is **not established**, so "the last row is never
drawn" is not claimed. The new `CnrContent` loader refuses a pool under
three rows instead of reproducing the fallback; the draw itself (seeded,
host-authoritative, distinct) is F27-B and an enhanced policy.

## 2. Settings the host chooses (verified)

The original packs the host's Cops & Robbers choices into one integer
(encode `0x5014c1`, decode `0x501510`):

| Bits | Field | Values |
| --- | --- | --- |
| `>> 6` | variant | 0 / 1 / 2 (see below) |
| `>> 4 & 3` | gold mass index | `0x5d0570`: **0, 100, 200** |
| `>> 2 & 3` | match-limit type | 0 none, 1 time, 2 points |
| `& 3` | limit value index | time `0x5d0550`: **5, 10, 20, 30** (minutes); points `0x5d0560`: **100, 250, 500, 1000** |

`mmlang.dll` agrees: `Weightless`/`Quarter Ton`/`Half Ton` (ids
0x14c–0x14e), `5/10/20/30 minutes` (0x150–0x153), `100/250/500/1,000 pts`
(0x154–0x157), `Gold Mass` (0x14f). Defaults at `0x523484…0x5234a0`: variant
0, no limit, gold mass 0, time value 5.0.

* **Gold mass units** — the table holds 0/100/200 in the engine's own mass
  unit. The help says ¼ and ½ ton; 100:200 matches that ratio, but nothing
  recovered fixes the unit, so `GoldMass::engine_units` is exposed
  separately from `GoldMass::added_mass_kg` (the *documented* reading,
  250/500 kg, a design reading of a label). Class: values **verified**,
  kilograms **inferred**.
* **Variant numbering** — **inferred**. Read from code: variant 0 compares
  the player's own points against the point limit (`0x425db7`), variants ≠ 0
  compare two team totals (`0x425e63`, `+0xb220`/`+0xb224`); variant 2
  constructs the `pt_red`/`pt_blue` pair (`0x423e5a`), variants 0 and 1 the
  `pt_hideout`/`pt_bank` pair. The help names the three variants as
  Free-for-all, Cops vs. Robbers, Robbers vs. Robbers; 0 and 2 are pinned by
  the code, so 1 is Cops vs. Robbers by elimination. Variant 0 builds a bank
  marker too; what a free-for-all player does with it is **unknown**.
* **Match-limit end** — verified for points in free-for-all: the local score
  reaching the limit announces `Point limit reached` (`mmlang` 0x77; team
  variants 0x79) and ends the round (`0x425dc7`). Time-limit end and the
  `20/15/10 minutes remaining` warnings (0x8a–0x8c) exist as strings; their
  trigger code was not traced.

## 3. Rules the code carries (verified call-site constants, open semantics)

* **Delivery** — a carrier standing at its hideout/bank marker delivers:
  +**100** points (`0x427290(0x64)` at `0x425b30`/`0x425c98`), the gold's
  mass is removed, the carrier cleared, and the host draws the next round's
  sites (`0x424ac0`, only when the authority flag `[0x6b2994]` is set). The
  marker's radius field is **12.0** (`0x423f46`); the test compares the
  carrier's squared distance with the marker radius (`0x425a65`), direction
  **inferred** from x87 flag tests. The same test requires the carrier to be
  in the marker's room or on the marker's room flags (`0x425a76…0x425a94`) —
  room semantics **unknown**.
* **Carrier mass, once** (F27-AC04's original behaviour) — pickup adds the
  configured mass to the carrier's rigid body; every way the gold leaves a
  carrier (delivery, drop, being robbed, `0x4267e9`/`0x4268c8`) calls the
  same routine with the negated amount (`0x4249f0`). No code path adds mass
  without a matching removal in the paths read.
* **Handling scalar** — the same routine, for the *local* car only, writes a
  scalar to `carsim+0x40c` on pickup — **1.0, 0.9, 0.81** for 0/100/200 units
  (`0x5c341c`) — and resets it to 1.0 on removal. It is consumed in the car
  force code at `0x4d5c22`/`0x4d5d00`; what force that is was **not
  identified**, so the number is sourced and its physical meaning is open.
* **Pickup award** — the local-pickup handler (message `0x25a`) adds **25**
  points (`0x4266a0`). Whether that is the first pickup, any pickup or a
  steal is **unknown**.
* **Authority** — gold is arbitrated by the host. A client's pickup request
  (message `0x25e`, `0x426560`) is honoured only when the host sees no
  current carrier (`[0xb21c] == 0`); the host then names the carrier to
  everyone. This is the original's answer to F27-AC02 (one carrier, ever).
  The DirectPlay message ids that matter: `0x1fc` (sets a started flag, `0x426373`; **inferred** match start), `0x204`
  match-end banner, `0x258` gold delivered/lost by a remote player, `0x259`
  a remote player dropped the gold (it lands at a position in the message),
  `0x25a` carrier changed, `0x25e` pickup request, `0x261` new gold sites,
  `0x25c`/`0x25d` team assignment (**inferred**). Payload layouts are not
  recovered, and rust-mm2's wire protocol is its own (compatibility with
  original DirectPlay clients is out of scope).
* **Announcements** — `mmlang.dll` ids 0x70 `You dropped the gold!`, 0x73/0x86
  `You have the Gold!`, 0x74/0x75 `Gold delivered!` (hideout/bank), 0x85/0x87
  `<name> has the Gold!`, 0x88 `<name> dropped the Gold!`, 0x89 `<name>
  delivered the Gold!`, and team labels `COPS`/`ROBBERS`/`RED`/`BLUE`
  (0x7a–0x81, 0x108–0x10b).

## 4. Commentary vocabulary (verified names, unverified triggers)

Both cue tables author: `BLUETEAMHASGOLD`, `REDTEAMHASGOLD`,
`BLUETEAMSTASHEDGOLD`, `REDTEAMSTASHEDGOLD`, `BLUETEAMDROPPEDGOLD`,
`REDTEAMDROPPEDGOLD`, and `ROB`/`COP` × `GETLOOT`, `DROPLOOT`, `STASHLOOT`,
`RECOVERLOOT`. The original therefore distinguishes four events per role —
**get, drop, stash (deliver), recover** — and announces team state in the
team variant. `RECOVER` implies picking up *dropped* gold is a distinct
event, but nothing here says who may recover or what it scores.

## 5. Still unknown (stays open against F27-A / UNK-10)

* What knocks gold out of a carrier — impact threshold, which collisions,
  whether cops differ from robbers, any drop invulnerability. The drop
  handler places the gold at a position carried in the message; the *cause*
  lives in code not traced.
* Gold pickup radius (the gold marker's constructor receives 5.0, `0x423d12`;
  its use as a pickup radius is not read).
* Cops-vs-robbers specifics: whether cops score for recovering, what the
  cops' bank delivery scores, vehicle assignment (the help says Mustang
  Cruiser / Mustang GT; the roster binding was not traced here).
* Respawn timing after delivery, behaviour when the carrier disconnects, and
  out-of-bounds gold (F27-AC03). A leaver "disappears for everyone" (MP-5);
  gold handling at that moment is unrecovered.
* Time-limit end and countdown warnings; round restart/rematch; whether the
  host's gold-mass choice applies to the next round or live.
* The `+0x276` re-draw flag, the `rows − 1` modulus's relation to the header,
  and the unit behind 100/200.

## The authoritative gold state machine (F27-B.1)

`mm2_game::gold::GoldMatch` is the host-side rule core: one gold object with
one ownership state (`Resting` → `Carried` ⇄ `Dropped`), the participants and
their scores, and the match end. It is pure state — the host feeds it
positions its own simulation holds, never a client's claim — and it emits
`GoldEvent`s for the wire, HUD and commentary. `mm2_content::cnr::CnrSettings
::rules` builds its `GoldRules` from the tables above. Where each rule comes
from:

| Rule | Class |
| --- | --- |
| delivery = 100 points within a 12.0 m marker radius, carrier cleared, new sites drawn | original (constants; radius direction inferred) |
| gold mass option 0 / 100 / 200 → 0 / 250 / 500 kg and handling 1.0 / 0.9 / 0.81 for the carrier only | original values; kg and the scalar's effect **provisional** |
| time 5/10/20/30 min, points 100/250/500/1,000; points end compares the individual (FFA) or the team total | original |
| a pickup awards 25 points | original constant; applied to *every* grant — implementation choice (trigger unknown) |
| exactly one carrier; a pickup is honoured only with no carrier | original (host arbitration, `0x426560`) |
| contested pickup: nearest host-measured car wins, ties to the lower player id, whatever the arrival order | implementation choice (the original's arbitration order is the host's message order) |
| pickup radius 5.0 m | enhanced policy |
| the dropper cannot retake the gold for 1 s | enhanced policy |
| anyone may recover dropped gold (a distinct `Picked { recovered }` event, the original's `RECOVERLOOT`) | documented (help: "anyone can pick it up") |
| a carrier who disconnects drops the gold where their car was | enhanced policy (original unrecovered) |
| gold outside the bounds is re-placed at a fresh pool site, no score, round advanced | enhanced policy (original unrecovered) |
| delivery targets: FFA and robbers → hideout, cops → bank; red → hideout draw, blue → bank draw | documented for FFA/robbers/cops; the red/blue mapping is an implementation choice |
| seeded, always-distinct draws of the three sites | enhanced policy (CNR-6: original used `rand()`) |
| a request carries the round it was made against; a delivery or re-placement advances the round, so late messages are refused `Stale` | implementation choice |
| tie at the end (level top scores or team totals) → `Winner::Tie` | implementation choice (original unrecovered) |

The carrier's handling load is *derived* (`GoldMatch::load_for`) rather than
stored on the car: only the current carrier has one, a stash/drop/leave ends
it in the same call that changes the state, and a new match starts with none.
This is how F27-AC04 ("applied and removed exactly once, no leak into a later
race") is made structurally true; the Bevy-side component that reconciles a
vehicle against it is F27-B.2.

What this slice does **not** do: no wire messages, no lobby/HUD, no vehicle
mutation, no rematch flow, no knock-loose trigger (the host will call
`dislodge` from its impact/damage systems with its own threshold — the
original's is unrecovered). So F27-AC01..06 remain open; the unit tests are
synthetic evidence for the rule core only.

## Consequences for the implementation

* `mm2_content::cnr` carries the verified tables (limits, mass options, the
  delivery constants), `CnrSettings` (the host's three choices → the rules a
  match enforces), and `CnrContent::load`, which resolves the data half
  through the VFS and counts every miss. `mm2_game::gold` holds the variant
  enum and the state machine itself.
* Everything marked *inferred* or *unknown* above must stay out of any
  "original-verified" claim for the mode. F27-B's rules for those points are
  **Enhanced policy / Implementation choice** and are labelled as such where
  they land.
