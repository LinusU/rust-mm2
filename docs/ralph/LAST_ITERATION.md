# Last iteration — report 7 item 3: cable cars hover (iteration 3 of the new run)

Selection: report 7 outranks everything; items 1-2 are implemented, so item 3 is next. The previous gate/review passed with no blocking findings.

Cause (measured on retail): `spawn_cable_cars` lifted each body by the bound record's `CG.y` (1.645 m). `va_cablecar_f` is authored with its origin at the floor (PKG `BODY_H` y 0.0014..3.29; `.bnd` y -0.002..3.32), so `CG.y` is just the centre of mass, and the lift floated the tram 1.6 m up. The route datum is fine: a temporary probe (not committed) raycast down from 3 m above the curve every 10 m on both SF circuits (495 samples) and found the carriageway at the curve's height (median gap 0.0 m).

Change: `cablecar::rest_lift` rides the body so the bound's base sits on the curve (about 0 on retail; 0 with no bound; garbage clamped/ignored). Ledger UNK-44 and `docs/research/specials.md` record the measurement. Tests: unit `the_car_rides_so_the_base_of_its_bound_rests_on_the_rail`; the retail production-spawn test now asserts `|lift| < 0.05` (MM2_RETAIL run: all 13 `cablecar` tests pass).

Captures (local, uncommitted, `/tmp/rm2cap`): same pose `--cam=-1280,71.9,168,90,0` at a start terminus, before `cc_b3.png` (wheels clear of the road line) and after `cc_after_b.png` (car dropped ~130 px, wheels on the road plane). The operator's exact pose shows no car at frame 120 (the cars have moved), so the window was reproduced at a start site. Not seen: a car running a flat stretch from the side; the exact operator frame.

Not done / open: the ferry mover applies the same `CG.y` lift (`movers.rs`) and was not re-measured; the original `+0x4c` offset's sign remains unrecovered.

Gates: see below.

Status: candidate; not independently checked. Next is item 4 (tram rails drawn on the outer lane).

Gates (foreground): fmt PASS; clippy `--locked --workspace --all-targets --all-features -D warnings` PASS; `cargo test --locked --workspace --no-fail-fast` exit 0 (73 `test result` lines, none failed).
