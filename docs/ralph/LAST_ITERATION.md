# Last iteration — F29-C.9: retail run of the F29-C refusals (iteration 17 of the new run)

Selection: F29-C.8 (`fb39a18`) passed gates and review with no blocking findings, so no repair was owed. Its review and F29-C.5/.7 handoffs named one open verification gap: the new refusals (`LessonSetupError::{Aimap,LeadRoute}`, `LoadCityError::Surfaces`) had only been exercised on synthetic installs. A refusal that fires on stock content would break the retail game, so the highest-value ready work was to run them against the real install rather than add more synthetic coverage. Capability check: `/Users/linus/coding/rust-mm2/retail` is present.
Production change: none. Docs-only (this file + the PLAN F29-C row).
Original-content validation (opt-in, `MM2_RETAIL=/Users/linus/coding/rust-mm2/retail`, foreground, `cargo test --locked -p mm2_app --test app`):
- `lesson_launch::every_retail_lesson_launches_at_both_difficulties`: expected 52, launched 52 — no stock Crash Course lesson hits the `Aimap` or `LeadRoute` refusal at Amateur or Professional.
- `every_retail_lesson_wired_opponent_becomes_a_lead_car_with_a_route`: wired 18, fielded 18, routed 18 (two roster notes unchanged: london `crash:10` `crash10-0.opp` and sf `crash:6` `stop-1.opp` are wired to no opponent — pre-existing, not refusals).
- `a_retried_retail_lesson_rebuilds_the_same_world`: 52/52 compared.
- Whole `app` test binary with `MM2_RETAIL` set: 1046 passed, 0 failed.
Not covered by this run: the retail `import_pipeline` tests look for `<repo>/retail` (not `MM2_RETAIL`) and mount only `mm2core.ar`, so they report "surfaces: no tables" and say nothing about the stock `materials.{mtl,csv}` pair; that pair's stock acceptance remains the F29-C.7 `--trace-deps` city run (sf/london loaded 148/152 named). No rendered or audible evidence.
Gates: code unchanged since `fb39a18` (gates green there: fmt, clippy `-D warnings`, `cargo test --locked --workspace`); docs-only commit.
Status: evidence recorded; the F29-C remainder (menu art, localization) stays open.
