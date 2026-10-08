# Last iteration — report 7 item 11: the three-process collision test waits on its condition (iteration 5 of the new run)

Selection: iteration 4's commit (`cd74a45`) passed gates and review with no blocking findings; item 11 is the last of report 7.

Cause: `a_driven_collision_replicates_across_three_processes` ended its clients on frame budgets (raised to 2800/2500 by `70d53a5`). A frame budget is not a clock: under load the ram's first strike lands late in host ticks and the uninvolved client can exit before the impact row reaches it.

Change:
- Headless smoke gains `smoke::StopWhen` / `RunBudget` (`headless_lobby`/`headless_host` take a `RunBudget` in place of `frames`; `headless_smoke` unchanged). `--frames` stays the ceiling. CLI (all `requires = headless`): `--until-impacts <n>` (applied replicated impact rows), `--until-peer-left` with `--with-impacts <n>` (a held remote copy is gone and this process has emitted `n` impacts of its own), `--deadline <secs>` (wall-clock bound). The record prints `stop=impacts|peer-left|deadline` and counts the updates actually run in `updates=`; records without an armed condition are bit-identical.
- The driven run: bob `--until-impacts 1 --deadline 120`, alice `--until-peer-left --with-impacts 1 --deadline 150`, frames 100000 as ceiling. The test asserts `stop=impacts` / `stop=peer-left`, so a deadline fallback fails loudly. Exact-count assertions are unchanged. The control run (nothing may collide, no condition to wait for) keeps its fixed 2800/2500 frames.
- Alice's `--with-impacts 1` came from a first parallel stress run: 3 of 10 copies failed because bob left (and alice with him) before alice's own predicted sim had registered the contact (`impacts=0`).

Tests: 3 unit (`mm2_app` `smoke::tests`: stop fires on its event and not before, peer-left needs a held peer first, deadline). Process level: the collision test alone ~10 s; 12 and then 16 copies in parallel all pass (before the `--with-impacts` fix: 7/10).

Gates: see the commit; fmt, clippy -D warnings, `cargo test --locked --workspace` run in the foreground.

Not verified: no windowed or retail run (networking test harness only). Residual from iteration 4 stands (rate/growth limit not applied to race rows).

Status: candidate; not independently checked. Report 7 items 1–11 are all implemented candidates; the next iteration returns to the F25-B/F25-C remainders in the plan.
