# Last iteration — report 7 item 2: light-bar flares (iteration 2 of the new run)

Selection: report 7 outranks everything; item 1 is implemented, so item 2 is next. The previous gate/review passed with no blocking findings, so there was nothing to repair first.

Cause (measured on retail with the new `mm2-inspect pkg <install> geometry/vpcop.pkg --shaders --verts`): `SRNn` is a 0.5 m flat quad whose shader (`vpcop` 21/22/23, solid blue/red/cream) names no texture and whose UVs are uninitialised memory; it was drawn as an opaque unlit box. `HEADLIGHTn` is the same quad with UVs (0,0). The `HLIGHT`/`TLIGHT`/`RLIGHT`/`BLIGHT` shaders author a black alpha-0 diffuse with the colour in `emissive` over `fxltglow*` (white radial falloff); the old unlit+blend material turned those into flat grey rectangles too — so `HLIGHT`/`BLIGHT` DO share the defect (seen on a forced-lit Beetle).

Change (ledger DSN-100, COP-10 pointer):
- `car_visual`: `SRNn` and `HEADLIGHTn` flares are camera-facing sprite quads (`FlareSprite`, `face_flares`, scheduled in its own `Update` slot), UVs synthesised corner to corner, material = glow texture (`fxltglow` when the shader has none) tinted by diffuse (emissive when the diffuse is black/transparent), `AlphaMode::Add`, unlit, two-sided, no shadow, scaled `FLARE_SCALE` 3x (enhanced policy; retail sprite size unrecovered). Other lamp quads keep their geometry and take the emissive-tint additive material (`group_material`). `SIRENn` housing unchanged.
- `mm2-inspect pkg --shaders` dumps shader records.
- Tests (`tests/light_bar.rs`, 3 new): additive/unlit/tint/UV/scale on all four `SRN` flares, billboard faces the camera under a rotated car, black-diffuse lamp shaders tint by emissive.

Captures (local, not committed; `/tmp/rm2cap`, sf `--spawn=59,1.5,95,0`, lights forced lit by a temporary local edit because a chasing cop could not be framed reliably, edit reverted): before `before_forced.png` reproduces the operator's opaque red/blue boxes at `--cam=60,4,79,0,-14`; after `cop3_after.png` shows a soft red/blue glow, plus lit head lamps. Beetle `--cam=59,2.2,88,180,-6`: before grey boxes, after glow halos. Not seen: the alternating flash on a genuinely pursuing cop in a windowed run; headlights at the shipped (off) default were not changed.

Gates: see end of this file.

Gates (foreground): fmt PASS; clippy `--locked --workspace --all-targets --all-features -D warnings` PASS; `cargo test --locked --workspace --no-fail-fast` exit 0 (73 `test result` lines, none failed).

Status: candidate; not independently checked. Report 7 items 3-11 untouched; next is item 3 (cable cars hover above the tracks).
