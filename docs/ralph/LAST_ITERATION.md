# Last iteration — F28-A.7: cross-check the other AI-map creations (iteration 24 of the new run)

Selection: F28-B.4 (`dc0f7ba`) passed gates and review with no blocking findings, so no repair was owed. The open F28 item that retail data can close is the "cross-check of the other AIMAP-created objects" (UNK-44 tail): `AIMAP.Init` also logs "Create the subways." and the exe has `Hookman` strings.

Findings (disassembly of `Midtown2.exe`, `docs/research/specials.md` § The other AI-map creations): the subway step needs the init's `+0x44` flag and a parsed `[Subway]` line (`%s %d`, reader `0x555b38`); it builds one train of N cars per BAI train-rail terminus. Retail has no uncommented `[Subway]` (`city/london.aimap` has `#[Subway]` / `#va_ug_l 3`), so nothing is created; London trains are the pathset family already run by `mm2_app::movers` (3 cars/path — the same `va_ug_l 3`). `[Hookmen]` row = `name model x y z w flag` (writer `0x5558c1`); retail has 0 rows.

Production change:
- `mm2_formats::aimap`: `[Subway]` section parsed into `Aimap::subway: Option<SubwayRecord { model, cars, line }>` (was an unknown section); 3 parser tests (real shape, commented line, malformed/missing).
- `mm2-inspect specials`: new per-city `aimaps` audit over `city/<city>.aimap` + `race/<city>/*.aimap{,_p}` counting `[Subway]` trains and `[Hookmen]` rows; any is an unresolved actor; an unparseable aimap is a `--strict` failure. 2 new tests (quiet retail shape vs loud mod shape, `.bak` ignored; unparseable aimap fails).
- Docs: specials.md, aimap.md, original-rules WLD-31 (verified_original) + UNK-45 (hookman semantics), PLAN F28-A.7.

Evidence: `mm2-inspect specials <retail> --strict` → aimaps SF 107 + London 102 = 209 files read, 0 `[Subway]`, 0 `[Hookmen]` rows, 0 unreadable, 0 failures, 0 unresolved.

Gates (foreground): `cargo fmt --all -- --check` pass; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` pass; `cargo test --locked --workspace` exit 0, 58 `test result: ok`, none failed.

Not covered / open: no runtime for a modded `[Subway]`/`[Hookmen]` (none in retail; listed unresolved); the `+0x44`/`+0x40` init gates; hookman semantics (UNK-45); cable-car audio, networking, head-on deadlock, rail side. Status: implemented candidate; not independently checked.
