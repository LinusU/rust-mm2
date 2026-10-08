# Last iteration — report 7 item 6: trees in a London intersection (iteration 6 of the new run)

Selection: report 7 outranks everything; items 1-5 are implemented and the previous gate/review passed with no blocking findings, so item 6 is next.

Channel: `mm2-inspect placement` (london) pointed at the `props.pathset` channel — `props.pathset:12:3/12:4 sp_tree1_s` at (214.8, 4.85, -818.9)/(215.0, -817.7), room 1207 `RoadFan`, ~5 m inside the carriageway, a metre apart. Path 12 is a two-point `LineStrip` (49.1 m, spacing 16 m) whose *end vertex* sits in the junction. Not INST, not prop_rule.

Cause: `line_strip_sites` stamped `ceil(len/spacing)` props at the raw spacing and then capped the row with the final vertex (an inference, UNK-20). The cap stamps the end vertex whatever the spacing, so every row's last two props crowd together at the row's end — here, inside a box junction.

Fix (recovered from the retail exe, not fudged): the shared strip stamper `Midtown2.exe` `0x466d30` (props loader `0x445130`/`0x4451d0`, parked-car manager `0x579906`), kind 2: per segment skip if `len < spacing`, else `step = len / floor(len/spacing)` (`0x5828a0` is the CRT `floor`; its other caller `0x443aab` is the `floor(x+0.5)` idiom) and stamp `k = floor(len/spacing)` times at stride `step` from the segment start. The end vertex is the next segment's first stamp; the final vertex and a lone vertex are never stamped. `mm2_game::props::line_strip_sites` now does exactly that (still counted arithmetically against the budget). Zero spacing keeps the designed one-per-vertex fallback (the original would divide by zero). Ledger WLD-33 (verified_original); WLD-13/UNK-20/`docs/research/pathset.md` updated.

Effect (placement audit, retail): london `props.pathset` 1188 → 1033 stamps, in-road 185 → 121, RoadFan in-road 48 → 11, Crosswalk 4 → 0, body-in-road 193 → 128; sf 925 → 778 stamps, in-road 34 → 30. Parked cars share the stamper, so their per-path counts shift the same way (spacing floored to 5 m still applies).

Capture (london, Apple M1, local, uncommitted in `/tmp/it6`): before/after at `--cam=203.0,7.7,-827.7,-125,-20` — the three/four trunks standing on the beige box junction are gone; the kerb rows of trees stay. Not fixed and out of scope: the flat beige box fill itself reads as a decal with no hatch texture there (not in the report).

Tests: `mm2_game::props` +2 (whole strides / never stamps the end incl. the retail path-12 numbers; short segment + lone vertex), 2 rewritten; `mm2_app::city` 5 rewritten to the new rule; `tests/event.rs` 3 counts (4 → 2 stamps) for its 12 m / 5 m fixture.

Not verified: the original's behaviour was read from disassembly, not run; the spacing byte's quarter-metre decode into the float at `path+0x34` was taken from the existing parse. Kind 0/1 branches unchanged (read, consistent with the port). Residual in-road pathset stamps (121) are other rows and kinds, not investigated.

Status: candidate; not independently checked. Next is item 7 (hole in a London building wall).

Gates (foreground): fmt PASS; clippy `--locked --workspace --all-targets --all-features -D warnings` PASS; `cargo test --locked --workspace --no-fail-fast` exit 0 (2927 passed, 0 failed).
