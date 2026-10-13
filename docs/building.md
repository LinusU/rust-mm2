# Building, running and packaging

How to build rust-mm2 from a clean checkout, where it looks for files and
where it writes them. Every claim below is either backed by something that
runs in this repository or labelled *untested*. `scripts/package.sh`
assembles the distributable install layouts — a portable folder, a macOS
`.app` bundle and a Unix prefix, each with an archive beside it (a
`.tar.gz`, or a `.zip` on Windows); there is no installer or auto-updater.

## Platforms

| Platform | What has actually been run | Status |
|---|---|---|
| macOS, Apple Silicon (arm64) | The development machine: fmt/clippy/test gates, windowed Metal runs, `scripts/release-build.sh` | **tested** |
| Linux, x86_64 | GitHub Actions `ubuntu-latest` runs fmt, clippy and the workspace tests (headless: no window, GPU or audio device) | build and tests **tested in CI only**; running the game windowed is *untested* |
| Windows (MSVC) | The non-gating `build-check` job's `cargo check --workspace --all-targets`, green on `windows-latest` (commit `895e89a`); never built or run by a maintainer. The Windows `.zip` layout is assembled and checked in `tests/package.rs` from fake `*.exe` binaries (see [Packaging](#packaging)) | compile check **tested in CI only**; building or running the game, `scripts/package.sh` itself and the `%APPDATA%` user-data path are *untested* |
| macOS, Intel | nothing (the `build-check` job's `macos-latest` is an arm64 runner) | *untested* |
| Linux, arm64 | nothing | *untested* |

A hosted CI pass is a code check. It does not show that a window opens, a
sound plays or an original installation loads — those need a person on the
platform.

## Toolchain

- Rust **1.95 or newer** (`rust-version` in `Cargo.toml`; edition 2024).
- `rust-toolchain.toml` selects the `stable` channel plus `rustfmt` and
  `clippy`, so the toolchain floats with stable releases and a new release
  can add lints that the `-D warnings` gate then enforces.
- Build with the committed `Cargo.lock` and `--locked`, so a dependency
  never moves under you. Bevy is 0.19 and Avian 0.7; do not upgrade
  either to make a build pass.

## Native dependencies

| Platform | Needed beyond Rust |
|---|---|
| macOS | The Xcode command line tools (`xcode-select --install`) for the linker. Nothing else; Metal ships with the OS. |
| Linux (Debian/Ubuntu) | `g++ pkg-config libx11-dev libasound2-dev libudev-dev libxkbcommon-dev libwayland-dev libxkbcommon-x11-dev` — exactly the packages CI installs. Other distributions need the equivalents of these (X11/Wayland windowing, ALSA audio, udev for gamepads); see [Bevy's Linux dependency list](https://github.com/bevyengine/bevy/blob/main/docs/linux_dependencies.md). |
| Windows | The Visual Studio C++ build tools (MSVC toolchain). *Untested.* |

There is no system dependency on any original-game library, and no network
access at build time beyond fetching the crates the lockfile names.

## Build and check

```sh
cargo build --locked                  # debug build of the game (mm2)
cargo build --locked --workspace      # every crate and tool

cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace
```

The last three are the gates in `AGENTS.md` and `.github/workflows/ci.yml`.
The first build compiles Bevy and takes several minutes; the `dev` profile
optimises dependencies (`opt-level = 3`) so a debug build is playable.
The tests need no original game data. The ones that do read
`MM2_RETAIL=<install dir>` and print `skipped: MM2_RETAIL is not set`
without it — a skip, not evidence about original content.

## Binaries

| Binary | Crate | Purpose |
|---|---|---|
| `mm2` | `mm2_app` | The game: window, menu, sessions, `--headless` smoke runs |
| `mm2-host` | `mm2_app` | Headless dedicated lobby host |
| `mm2-join` | `mm2_app` | Headless lobby client |
| `mm2-inspect` | `mm2_inspect` | Read-only inspector for an MM2 installation and its parsed content |

`cargo run` (workspace default member) starts `mm2`. A debug build lands in
`target/debug/`, a release build in `target/release/`.

## Release build without private paths

`cargo build --release` embeds the builder's absolute paths in the
binaries: the registry copy of every dependency, the toolchain's sources,
the checkout. A debug build of this repository contains thousands of
`/Users/<name>/…` strings. A release artifact should not.

```sh
scripts/release-build.sh
```

This runs `cargo build --locked --release` for the four binaries with
`--remap-path-prefix` for the checkout, `$CARGO_HOME` and `$RUSTUP_HOME`
(Cargo's `trim-paths` is still nightly-only), strips debug information
(the linker's object-file debug map names the toolchain's `libstd` under
your home even after remapping) and re-signs ad hoc on macOS, then runs
`scripts/scan-private-paths.sh` over the four binaries. The script exits
non-zero if the scan finds `$HOME`, `$CARGO_HOME`, `$RUSTUP_HOME` or the
checkout path in any of them. Run the scanner on its own against any file:

```sh
scripts/scan-private-paths.sh path/to/binary [more files...]
scripts/scan-private-paths.sh --forbid /some/private/prefix path/to/binary
```

Verified on macOS arm64 only (the build, the scan and a smoke run of the
stripped `mm2` and `mm2-inspect`). The Linux `strip` branch — GNU
`strip --strip-debug` over the four binaries, then the same scan — is
*untested*: no maintainer machine runs it, and hosted CI's `test` job
builds the debug profile rather than `scripts/release-build.sh`.
Promoting it needs a CI job or a Linux machine that runs the script, and
`.github/` is owner-controlled, so it is left to the owner (recorded on
task #1328). The Windows build is *untested*; on Windows the scan is the
safeguard, not a guarantee. The release profile deliberately keeps symbol
names so a profiler can read the binary (see the profiling section of the
README); the script's output is the one to distribute.

What the artifact must not contain is a policy, not only a path check: no
original archive or extracted asset, no profile, no screenshot or perf
capture, no credential. `assets/` holds only the project's own synthetic
files; `retail/` and `screenshots/` are git-ignored.

## Packaging

```sh
scripts/package.sh                          # release build, then every layout
scripts/package.sh --layout app             # only the macOS .app bundle
scripts/package.sh --bin-dir <dir>          # package binaries you already built
```

`scripts/package.sh` runs `scripts/release-build.sh`, then assembles the
install layouts and checks each one. `--layout` picks them; it defaults to
`all`, every layout the platform ships:

| `--layout` | Produced (under `target/package/`) | Layout |
|---|---|---|
| `flat` | `rust-mm2-<os>-<arch>/` and an archive | the four binaries, `assets/`, `README.md`, `LICENSE-MIT`, `LICENSE-APACHE` and `docs/` at the root |
| `app` | `rust-mm2.app/` and an archive | `Contents/MacOS/` binaries, `Contents/Resources/assets/`, `Contents/Resources/{README.md,LICENSE-*,docs/}` — macOS only |
| `prefix` | `rust-mm2-<os>-<arch>-prefix/` and an archive | `bin/` binaries, `share/rust-mm2/assets/`, `share/doc/rust-mm2/{README.md,LICENSE-*,docs/}` |

The archive is a `.tar.gz` on macOS and Linux and a `.zip` on Windows
(MSYS/Cygwin), so the artifact a Windows user downloads opens in Explorer
with nothing installed. Windows is detected from `uname -s`
(`MINGW*`/`MSYS*`/`CYGWIN*`), which also names the binaries `*.exe` and
shortens the OS part of the artifact name to `windows`, so the portable
artifact is `rust-mm2-windows-<arch>.zip` rather than the raw
`MINGW64_NT-10.0-…` release string. `zip` and `unzip` must be on the PATH
of the machine that assembles it. The staged folder is otherwise the
same on every platform: only the binaries' names and the archive type
change.

Every layout carries the same files, in the place that layout — and the
app's own asset search — expects them. `--layout app` is a usage error off
macOS, where a bundle of foreign binaries means nothing; `all` skips it
there. `scripts/check-package.sh --layout <flat|app|prefix> <dir>` judges
a folder against that layout's allowlist; `scripts/package.sh` runs it on
the staged folder, writes the archive, **extracts the archive into a
scratch directory and checks that too**, since the archive is what ships.
It never overwrites: an existing folder or archive is an error.

`scripts/check-package.sh [--layout <flat|app|prefix>] <dir>` is an
allowlist, not a blocklist. A file the package is not meant to carry fails
even if nobody thought to forbid it; the mistakes worth naming (an
original `*.ar` archive, a `driver-*` profile or `settings.json`, a
`*.csv`/`*.report.json` perf capture, a `*.log`, `.env`/`*.pem`/`*.key`)
get their own message. The allowlist paths are translated from the layout,
so `Contents/Resources/assets/car.dds` and `share/rust-mm2/assets/car.dds`
fail like `assets/car.dds` would while the message names the real file.
Symlinks fail (they can point out of the package), assets must be of a
known type and under 8 MiB, all four binaries (all `.exe` or none) and the
layout's `texture/dev_road.png` and `README.md` must be present, and every
file goes through `scripts/scan-private-paths.sh`. Every problem is listed,
not only the first. Run it on any folder you are about to hand to someone.

A package carries `LICENSE-MIT` and `LICENSE-APACHE` (the repository is
`MIT OR Apache-2.0`; the retail game data is not covered by either and is
never packaged). It carries no original content by
construction: nothing in the script reads an install.

The flat folder, the macOS `.app` (`Contents/MacOS/mm2` with
`Contents/Resources/assets`) and the Unix prefix (`bin/mm2` with
`share/rust-mm2/assets`) layouts are each launched in
`tests/package.rs` with the freshly built `mm2` copied into place and run
from an unrelated directory, and the layouts `scripts/package.sh`
assembles are checked there too. The Windows `.zip` is assembled and
checked in the same file from fake `*.exe` binaries with a stub `uname`
that puts the script on its Windows path — `rust-mm2-windows-<arch>.zip`,
all four `*.exe`, both licenses, checked again after unzipping. That is
the script's Windows branch and the checker's `.exe` allowlist; a real
Windows build, a real `uname` and launching the `.exe` binaries on Windows
are *untested*.

## Where the app finds files

**Its own base assets** (`assets/texture/dev_road.png` and friends) are
found from the executable, not from the working directory. The first of
these that holds `texture/dev_road.png` wins:

1. `<exe dir>/assets` — a flat portable folder (Windows/Linux);
2. `<exe dir>/../Resources/assets` — a macOS `.app` bundle;
3. `<exe dir>/../share/rust-mm2/assets` — a Unix prefix install;
4. `assets/` of an ancestor of the executable (up to four levels) — the
   cargo `target/<profile>/` dev tree;
5. `<cwd>/assets` — last resort.

So a portable build is the binary with the repository's `assets/` folder
beside it, and it works when launched from any directory. No path is
compiled in. The lookup is unit-tested and process-tested
(`tests/app_assets.rs`, `tests/package.rs`); `scripts/package.sh`
assembles the `.app` and prefix layouts and checks them in place.

**The original game** is never bundled and never modified. Point at a
retail installation with `--mm2-path "/path/to/Midtown Madness 2"` (the
directory holding the `.ar` archives and/or loose files). It is mounted
read-only through the VFS; mods are layered above it with `--mods <dir>`
(see [modding.md](modding.md)). `--dev-world` needs no original data.

### Slow or failing storage

Mounting reads a whole `.ar` or loose file in one call and applies no
timeout. Slow media makes that read (and startup) take longer, but it
cannot truncate the result: a `DAVE` archive is only accepted once its
whole range is present, so a stream that stops early is a reported error
(`skipped archive …` on the mount report, or a named read error), never a
silently short or empty load. A loose path that is not a regular file is
refused by name rather than read, so a FIFO or device node cannot block a
load forever.

Guarded by `cargo test -p mm2_assets --locked` —
`crates/mm2_assets/tests/slow_storage.rs` trickles a synthetic archive
through a FIFO (slow in, whole out; truncated in, `AssetsError::Archive`
out and no partial mount; a corrupt `.ar` is reported in `MountReport::skipped`)
and the `slow_storage` unit tests do the same for the bounded loose-file
read. Synthetic media only: no real slow device has been tested.

## Where the app writes files

| What | Default location | Override |
|---|---|---|
| Driver profiles (`driver-*`) and `settings.json` | macOS `~/Library/Application Support/rust-mm2/profiles`; Windows `%APPDATA%\rust-mm2\profiles`; other Unix `$XDG_DATA_HOME/rust-mm2/profiles`, else `~/.local/share/rust-mm2/profiles` | `--profile-dir <dir>` |
| Cmd/Ctrl+P screenshots | `screenshots/` in the working directory, or `rust-mm2/screenshots` beside the profiles when the working directory is protected | — |
| `--screenshot`, `--perf-log` (+ `.report.json`) | exactly the path given | the path |

The app refuses — exit status 2, nothing created — any of those
destinations inside the `--mm2-path` installation, the app's base-asset
directory or the executable's directory (symlinks followed, `..` folded).
There is no cache directory and no log file; diagnostics go to the
terminal. `--mods` directories are read, never written. Windows paths are
*untested*.

Verified 2026-10-11 on macOS arm64 (commit `d26bfff`): `scripts/package.sh`
exits 0 ("package clean: 8 files" for the folder and for the extracted
tarball, private-path scan clean), and the `mm2` extracted from the
tarball, run from `/`, passes `--headless --dev-world --bot --frames 300`
with the same result (`status=pass travel=70.0m`) as the build-tree
binary. Without `--bot` the headless smoke reports `status=fail ... car
never drove` for both, because the default driver holds no input. That is
the smoke's own verdict, not a packaging fault.

Verified 2026-10-12 on macOS arm64 (commit `ca3511f`): `scripts/package.sh
--out target/f30-layouts` (a release build, `--layout all`) assembled and
checked all three layouts — the flat folder, `rust-mm2.app` and the prefix
— each `package clean: 10 files` for the folder and for its extracted
tarball. The `mm2` inside `rust-mm2.app/Contents/MacOS` and inside
`…/rust-mm2-darwin-arm64-prefix/bin`, run from `/`, each log `mounted app
assets` at its own bundle/prefix path and pass `--headless --dev-world
--bot --frames 300` with `status=pass travel=70.0m`.

### Release-candidate acceptance (F30-AC06)

The smoke above runs one command. `scripts/check-release-candidate.sh`
runs a fixed set of the README's headless evidence commands and compares
each one's `smoke=` record between a release candidate and a baseline
build:

```sh
scripts/check-release-candidate.sh \
    --bin target/package/rust-mm2-darwin-arm64/mm2 \
    --baseline target/debug/mm2 [--out <dir>]
```

Two commands run on the dev world (no original data); four run on the
retail cities and need `MM2_RETAIL`. A city command without the install
is reported `skipped`, never counted as a pass. Every record must be
byte-identical between the two binaries — status, world and every
subsystem counter the record carries (collision, pedestrians, traffic,
signals, movers, damage, race rules) — so an optimisation that changes
scene content or a game rule fails the check.

Verified 2026-10-12 on macOS arm64 (commit `895e89a`): the `mm2` inside
the package `scripts/package.sh --out target/f30-rc` built (release
profile, stripped, re-signed) and the unoptimized `target/debug/mm2`
printed the same six records, byte for byte:

| command | recorded evidence | record sha256 |
|---|---|---|
| `--dev-world --headless --frames 600` | `status=pass travel=158.4m` | `46519810…` |
| `--dev-world --headless --bot --frames 600` | `status=pass travel=197.4m` | `63950dbd…` |
| `--city sf --headless --frames 600` | `status=pass` + the full scene record (pvs/wtr/sig/peds/traf/movers/damage) | `353d0e0d…` |
| `--city london --event blitz:0 --headless --frames 2400` | `status=pass phase=results outcome=timed-out` | `e3e4766a…` |
| `--city london --event blitz:0 --headless --bot --frames 2400` | `status=pass outcome=finished place=1` | `0413e2f1…` |
| `--city sf --event checkpoint:0 --headless --bot --frames 3000` | `status=pass cp=6/6 outcome=finished place=1` | `398d36d1…` |

The commands are headless: they show the optimised binary loads and steps
the same content and reaches the same race outcome, not that a scene's
geometry or textures were re-inspected. A rendered release-candidate
check (the `--cam` captures run on the package as on the dev build) is
still open. The per-command logs and full shas are written under the
`--out` directory (default `target/release-candidate/`, git-ignored).

The packaged binary cannot run the in-process Rust integration tests:
they link the game crates into the test process, not the shipped binary.
The headless commands above are the part of that evidence the binary
itself can produce. The in-process game-rule and scene tests can still be
run against the optimised code by switching profile:

```sh
cargo test --release -p mm2_app --test app
```

Verified 2026-10-12 on macOS arm64 (`cargo test --release -p mm2_app
--test app` on this branch): 1206 of the 1208 app integration tests pass
in the release profile. The two that do not are
`nonfinite_ray::avian_asserts_a_finite_ray_origin` and
`…an_infinite_ray_origin`, both `#[should_panic]` tests of Avian's
`debug_assert!(origin.is_finite())`. The release profile has
`debug-assertions = false`, so the assertion they pin is not compiled in
and there is nothing to panic on: they are debug-build tests, not a scene
or game-rule regression. Their siblings, the finite-ray case and
`avian_shape_intersections_tolerate_a_non_finite_pose`, pass in both
profiles.

## Entry points

```sh
cargo run -- --dev-world                          # no original data needed
cargo run -- --mm2-path "<install>"               # menu front-end
cargo run -p mm2_inspect -- scan "<install>"      # inventory / diagnostics
cargo run --bin mm2-host -- --help                # dedicated lobby host
cargo run --bin mm2-join -- --help                # lobby client
```

The README documents the flags in context; `mm2 --help` lists them all.

## Not done yet

- A Windows build and a launch of the Windows `.zip`: `scripts/package.sh`
  takes its Windows branch (`*.exe`, `.zip`) under a stub `uname` in
  `tests/package.rs`, but no maintainer machine or CI runner has run the
  script with a real Windows `uname`, built the `.exe` binaries or opened
  the zip.
- A double-clickable `.app`: the bundle has the layout the app's asset
  search uses (`Contents/MacOS/mm2`, `Contents/Resources/assets`) but no
  `Contents/Info.plist`, so it is run from `Contents/MacOS/mm2` rather
  than opened by Finder, and it is not signed or notarized (task #1375).
- A *rendered* release-candidate check: `scripts/check-release-candidate.sh`
  runs the headless commands, not the `--cam` captures, so it shows the
  optimised package loads the same content and reaches the same race
  outcomes, not that a scene's geometry or textures still draw.
- A hosted CI job that runs `scripts/package.sh`. The non-gating
  `build-check` job is green on `macos-latest` and `windows-latest`
  (commit `895e89a`), but it is a `cargo check`, not a package build or a
  launch, and no CI job runs `scripts/release-build.sh` (so its Linux
  `strip` branch stays untested).
- Windowed runs on Linux and Windows; non-ASCII installation paths on
  every platform.
