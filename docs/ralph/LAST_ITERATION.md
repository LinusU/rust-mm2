# Last implementation iteration

- Task ID and title: F00-C — reusable synthetic / original-data /
  graphical evidence commands with explicit missing-capability outcomes
  (completes the F00 evidence-harness scope; selected per the reconciled
  plan after F00-B's children passed external check+review).
- Starting commit and resulting commit: started at
  `ab32a705e08e399b4b2fc20e7040d9837658d6fa` (clean tree, branch
  `ralph/night`, external check+review had just passed on F00-B.2);
  result = this commit.
- Production code changed:
  - `crates/mm2_app/src/smoke.rs` (new): `SmokeRecord`/`SmokeStatus`
    report (`smoke=<kind> world=<w> status=<pass|fail|unavailable>
    <metrics>`), kinds `headless-physics` vs `visual`, exit codes
    0/3/4 (2 stays usage-error), `header()` with the build-embedded
    engine commit, and `headless_smoke()` — the MinimalPlugins +
    Avian runner (same pattern as `tests/drive.rs` /
    `examples/drive_probe.rs`) that spawns the dev world or a real
    city through the VFS, settles, then holds full throttle and checks
    finite/grounded (+drove, for the synthetic world).
  - `crates/mm2_app/build.rs` (new): embeds `MM2_BUILD_COMMIT` — same
    mechanism as `mm2-inspect` so smoke reports are versioned by the
    code that produced them.
  - `crates/mm2_app/src/main.rs`: `--headless` flag (conflicts with
    `--screenshot`/`--cam`); `--city` is now `Option<String>` and a
    specifically requested city always means City mode — a VFS miss is
    a hard failure or `unavailable`, never a silent dev world; smoke
    records printed for the `--car` load failure and the two
    capability checks (requested city with no data source →
    `unavailable` exit 4; visual run with no display → `unavailable`);
    `smoke_test` now ends early with `status=fail` on
    `WorldState::Failed` and awaits the screenshot file on disk before
    reporting `pass` (no more fixed 5-frame hopeful delay); `AppExit`
    codes propagate to the process exit code.
  - `crates/mm2_app/src/lib.rs`: `pub mod smoke`.
- Documentation changed:
  - `README.md`: "Smoke tests (evidence commands)" section — commands,
    record format, status table, exit codes.
- Key behavior verified on this machine (macOS arm64, retail install):
  - `mm2 --dev-world --headless` → `status=pass` (600 updates, peak
    27.9 m/s, moved 157 m, 4/4 wheels grounded), exit 0 — dev world
    starts with no MM2 data and the car actually drives (AC04).
  - `mm2 --mm2-path <retail> --city sf --headless --frames 300` →
    `status=pass` — 1171 rooms / 3763 props imported via VFS, stock
    `vpbug` drove 30 m on real city collision, exit 0.
  - `mm2 --dev-world --frames 90 --screenshot /tmp/x.png` →
    `status=pass`, screenshot awaited: report printed only after the
    2.9 MB PNG existed on disk; image visually verified (dev car on
    textured road, live HUD). First recorded GPU/render evidence of the
    run (AC05).
  - `mm2 --headless --city bogus` (no install/mods) →
    `status=unavailable`, exit 4 — missing data is not a failure.
  - `mm2 --mm2-path <retail> --city bogus --headless` → `status=fail`,
    exit 3, record names `city/bogus.psdl` + reason — explicit failure
    for a requested missing city (AC04).
  - `mm2 --mm2-path <retail> --city bogus --frames 60` (windowed) →
    `status=fail`, exit 3 via `WorldState::Failed`.
  - `mm2 --headless --dev-world --car nosuchcar` → `status=fail` record
    + exit 2 (usage-level explicit failure for a requested missing
    vehicle).
  - `--headless --screenshot` → clap conflict, exit 2.
- Tests added/changed and why: `crates/mm2_app/tests/smoke.rs` — 3
  integration tests driving the production `headless_smoke`: dev-world
  pass on an empty VFS, requested-missing-city → explicit `fail` naming
  the logical path, and record-line kind/status distinguishability
  (AC05) + exit codes.
- Commands actually run and results:
  - `cargo fmt --all -- --check` — PASS.
  - `cargo clippy --locked --workspace --all-targets --all-features --
    -D warnings` — PASS.
  - `cargo test --locked --workspace` — PASS, 18 test binaries/doc-test
    groups, 0 failures (incl. the 3 new smoke tests).
- Acceptance IDs satisfied / still open:
  - F00-AC04: satisfied — dev world starts without original data
    (headless + visual evidence); a specifically requested missing
    city/vehicle exits or reports an explicit failure
    (`status=fail`/`unavailable`, non-zero exits).
  - F00-AC05: satisfied — `smoke=headless-physics` and `smoke=visual`
    records are independently distinguishable; both were run and their
    records observed. Candidate, pending external check.
  - F00-AC01: gates re-run and pass.
  - F00-AC02/AC03/AC06: unchanged — carried by F00-B.1/B.2, still
    externally checked.
- Evidence files: none committed; the verification screenshot lives at
  `/tmp/mm2-smoke-devworld.png` (local only, synthetic dev world — no
  original content).
- Stock data/GPU/audio/network limitations: GPU/render evidence now
  recorded on this machine for the synthetic dev world only — no
  original-content frame has been captured this run (the city visual
  smoke is runnable but was not captured; London/SF visual evidence
  still open). Audio unexercised (no audio code / no `bevy_audio`).
  Network unexercised (no networking code).
- Unresolved blockers or discovered regressions: none. The visual
  smoke's display check is heuristic (Linux DISPLAY/WAYLAND_DISPLAY);
  on a genuinely GPU-less but display-present session wgpu would fail
  noisily rather than report `unavailable` — honest crash, not a false
  pass.
- Next smallest useful action: F01-A — typed SessionConfig + explicit
  session lifecycle in `mm2_game` (plan's next foundation slice).

This is a candidate handoff. External code-gate and separate review results live in the runner state directory and are not implied by this report.
