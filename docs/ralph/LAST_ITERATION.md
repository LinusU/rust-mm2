# Last iteration — report 7 item 4: tram rails in the outer lanes (iteration 4 of the new run)

Selection: report 7 outranks everything; items 1-3 are implemented and the previous gate/review passed with no blocking findings, so item 4 is next.

Cause (measured on retail): not the tram runtime and not a flipped section — every road's texture v was mapped backwards. `emit_road_surface` and the divided-road carriageways put the centre line at v = 0 and the kerb at v = 1. The data says the reverse: SF room 689 carries `r4_track_f` on a 20 m road (centre z = 163.19). The texture's rail band is centred on row 190/256, so v = 0.74 from the kerb, 0.26 from the centre: 2.6 m off the centre line. The authored `r4i_rails_f` decals in the junction beside it sit at z = 160.62 / 165.75, 2.57 m either side of the same centre. Only centre-at-v=1 lines the baked rails up with the decals (the old mapping put them 7.4 m out and flipped them back at each junction). `r2_f` agrees (white edge line inset at v~0.03, yellow at v = 1).

Change: `city.rs` gains `carriageway_uv` (kerb → centre/median, v 0 → 1), used by `emit_road_surface` and both divided carriageways. This is the global road mapping, so every road texture changes with it: SF's double yellow now sits on the centre line and the white edge lines at the kerbs, as seen in the captures. Ledger WLD-32 and `docs/research/psdl.md` record the measurement. Tests: `road_surface_mirrors_texture_about_the_centre_line` now calls the production helper (centre v = 1, kerb v = 0); new `a_divided_carriageway_runs_kerb_to_median`.

Captures (local, uncommitted, `/tmp/rm4`): `--cam=-1027.6,91.0,165.5,-90,-27` on retail SF, before `before.png` (rails against both kerbs, double white centre) and after `after.png` (rails 2.6 m either side of a double yellow centre, lining up with the junction's rails). London `--cam=203.0,7.7,-827.7,-125,-20` after only (`london_after.png`): dashed white centre, no visible breakage; not compared against a before.

Not done / open: divided roads and London were checked by data reasoning (US yellow-on-the-median-edge; UK dashed centre), not by a before/after capture. Whether the original loader mirrors per section the same way is unrecovered; the evidence is the data alone.

Status: candidate; not independently checked. Next is item 5 (Cops & Robbers navigation arrow and map markers).

Gates: see below.
Gates (foreground): fmt PASS; clippy `--locked --workspace --all-targets --all-features -D warnings` PASS; `cargo test --locked --workspace --no-fail-fast` exit 0 (73 `test result` lines, none failed).
