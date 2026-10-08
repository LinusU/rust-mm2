# Last iteration — report 7 item 7: hole in a London building wall (iteration 7 of the new run)

Selection: report 7 outranks everything; items 1-6 are implemented, so item 7 is next.

Which candidate (bisected with temporary env-var skips in `city.rs`, since removed): none of the four. The wall is a stack: a shop-front *INST* piece (`cw_b5_store02_20_l`, 20 x 5 m flat strip) with the PSDL `Facade` attributes above it. The Facade bottom (9.8754 at london (238.9, -838.6)) equals the INST's `y + 5.0` exactly, but `simple_transform` scaled the model's height by the heading length too (scale 0.8 here), so the shop front topped out at ~8.9 and left a ~1 m slot under the facade through which the far city shows.

Fix: a simple INST placement's heading vector stretches the model along local X only; height and depth stay 1 (`mm2_app::city::simple_transform`). Evidence (ledger WLD-34, docs/research/inst.md): over every stretched simple placement that has a Facade/Sliver piece at its vertex, the piece height equals `y + model height` in 93/119 London cases (23/40 SF) and `y + model height x scale` in 1 (0 SF); full-basis records of the same families keep |y|, |z| ~ 1 while |x| varies. The Z (depth) = 1 part is inferred from those records, not measured on a simple placement (almost all stretched pieces are flat).

Captures (london, Apple M1, local, in `/tmp/it7`, not committed): `before_5.png` / `after_5.png` at `--cam=255.1,7.4,-830.7,45,5` (the operator pose's yaw, pitched up so the facade is in frame) — the slot between the red shop band and the tan facade is closed, the shop front is now the full 5 m; `south.png` / `south_after.png` at `--cam=240.0,7.4,-826.0,0,3`. Same stretched-INST families exist in SF (40 placements), so SF shop fronts grow too.

Tests: `mm2_app::city` `simple_placement_heading_rotates_and_stretches_along_x_only` (replaces the uniform-scale test). Original-data measurement was a throwaway scratch example, deleted; not a committed audit.

Not verified: the original loader was not decompiled; depth scale; whether 24/17 stretched placements with "neither" match (London/SF) are 3D models, placements without a facade at the vertex, or another rule. The `5-wall-hole-london.webp` exact pose (`pitch -17`) was captured (`after.png`) but not eyeballed beyond the pitch 5 view.

Status: candidate; not independently checked. Next is item 8 (low-time beep every second).

Gates (foreground): fmt PASS; clippy `--locked --workspace --all-targets --all-features -D warnings` PASS; `cargo test --locked --workspace --no-fail-fast` exit 0 (2927 passed, 0 failed).
