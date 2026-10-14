# Cops & Robbers: per-variant acceptance status (F27-AC06)

The stock game has three variants (`CnrVariant`): free-for-all, Cops vs.
Robbers and Robbers vs. Robbers, each playable in San Francisco and
London. A chase sandbox that exercises one of them says nothing about the
others, so each cell below records the highest evidence level
(`specs/QUALITY-GATES.md`) actually reached for that variant in that city.
A cell is never marked for a level its tests do not exercise.

Levels: **code** = fmt/clippy/tests on CI; **synthetic** = the real rule
path on controlled fixtures; **original** = the retail install's site
pool and rules through production code (`MM2_RETAIL` set); **playtest** =
a person watched it.

| Variant | City | Highest level reached | Evidence |
|---|---|---|---|
| Free-for-all | sf | original | `cnr_variants::retail_every_variant_starts_and_plays_a_round_in_each_city`; three-process thief run `net_drive` (`--cnr ffa`, sf) |
| Free-for-all | london | original | the same retail test (start, pickup, load, delivery); no networked run |
| Cops vs. Robbers | sf | original | the retail test; synthetic: `net_app` replica convergence under loss/reordering, `results` match-over screen |
| Cops vs. Robbers | london | original | the retail test; no networked run |
| Robbers vs. Robbers | sf | original | the retail test; synthetic: `results` match-over screen (team verdict, leaver) |
| Robbers vs. Robbers | london | original | the retail test; no networked run |

What the **original** level covers: for each variant × city the city's
authored site pool seeds a round through `CnrHost::from_content`, the
roster is dealt over the variant's own sides, a pickup makes exactly the
carrier bear the load, the delivery at the side's own marker scores
(team totals for the team variants) and the load ends with it.

What is **not** covered for any variant: **playtest** evidence (nobody has
watched a Cops vs. Robbers or Robbers vs. Robbers match), and any
networked multi-process run other than free-for-all in sf. Those stay
open under F27-AC06 and F27-AC01; the table is the place to raise a cell
when one lands. The retail test is skipped without `MM2_RETAIL`, which is
not a pass for the *original* column.

The carrier's mass load not leaking into a later round is separate
evidence (F27-AC04): see the `cnr::tests` for a leaver, a round restart
and a session teardown followed by a new match on the same car.
