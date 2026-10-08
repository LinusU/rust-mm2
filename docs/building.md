# Building, running and packaging

How to build rust-mm2 from a clean checkout, where it looks for files and
where it writes them. Every claim below is either backed by something that
runs in this repository or labelled *untested*; there is no packaging
step yet (no installer, `.app` builder or archive), only the pieces a
package needs.

## Platforms

| Platform | What has actually been run | Status |
|---|---|---|
| macOS, Apple Silicon (arm64) | The development machine: fmt/clippy/test gates, windowed Metal runs, `scripts/release-build.sh` | **tested** |
| Linux, x86_64 | GitHub Actions `ubuntu-latest` runs fmt, clippy and the workspace tests (headless: no window, GPU or audio device) | build and tests **tested in CI only**; running the game windowed is *untested* |
| Windows (MSVC) | A non-gating `cargo check` job in CI (`build-check`); never built or run by a maintainer | *untested* — the `%APPDATA%` user-data branch has not been compiled by the project |
| macOS Intel, Linux arm64 | nothing | *untested* |

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
stripped `mm2` and `mm2-inspect`). The Linux `strip` branch and the
Windows build are *untested*; on Windows the scan is the safeguard, not a
guarantee. The release profile deliberately keeps symbol names so a
profiler can read the binary (see the profiling section of the README);
the script's output is the one to distribute.

What the artifact must not contain is a policy, not only a path check: no
original archive or extracted asset, no profile, no screenshot or perf
capture, no credential. `assets/` holds only the project's own synthetic
files; `retail/` and `screenshots/` are git-ignored.

## Packaging

```sh
scripts/package.sh                      # release build, then package
scripts/package.sh --bin-dir <dir>      # package binaries you already built
```

`scripts/package.sh` runs `scripts/release-build.sh`, then assembles
`target/package/rust-mm2-<os>-<arch>/` — the four binaries, the
repository's synthetic `assets/`, `README.md` and `docs/building.md` +
`docs/modding.md` — and a `.tar.gz` beside it. It checks the folder with
`scripts/check-package.sh`, writes the tarball, **extracts the tarball into
a scratch directory and checks that too**, since the archive is what
ships. It never overwrites: an existing folder or tarball is an error.

`scripts/check-package.sh <dir>` is an allowlist, not a blocklist. A file
the package is not meant to carry fails even if nobody thought to forbid
it; the mistakes worth naming (an original `*.ar` archive, a `driver-*`
profile or `settings.json`, a `*.csv`/`*.report.json` perf capture, a
`*.log`, `.env`/`*.pem`/`*.key`) get their own message. Symlinks fail
(they can point out of the package), assets must be of a known type and
under 8 MiB, all four binaries (all `.exe` or none) and
`assets/texture/dev_road.png` must be present, and every file goes through
`scripts/scan-private-paths.sh`. Every problem is listed, not only the
first. Run it on any folder you are about to hand to someone.

A package carries no `LICENSE` file because the repository has none yet;
the check accepts one when it appears. It carries no original content by
construction: nothing in the script reads an install.

The flat folder, the macOS `.app` (`Contents/MacOS/mm2` with
`Contents/Resources/assets`) and the Unix prefix (`bin/mm2` with
`share/rust-mm2/assets`) layouts are each launched in
`tests/package.rs` with the freshly built `mm2` copied into place and run
from an unrelated directory. Only the flat tarball is produced; nothing
yet assembles the `.app` bundle or a prefix install, or a Windows zip.

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
(`tests/app_assets.rs`, `tests/package.rs`); the `.app` and prefix layouts
are launched from a copied binary, but nothing builds those packages yet.

**The original game** is never bundled and never modified. Point at a
retail installation with `--mm2-path "/path/to/Midtown Madness 2"` (the
directory holding the `.ar` archives and/or loose files). It is mounted
read-only through the VFS; mods are layered above it with `--mods <dir>`
(see [modding.md](modding.md)). `--dev-world` needs no original data.

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

- The `.app` bundle, a prefix install and a Windows zip (only the Unix
  tarball is produced), and a `LICENSE` file to put in it.
- The release-candidate checks that run the game-rule tests *on the
  packaged binary* (F30-AC06): the package is checked for contents, not yet
  exercised end to end.
- A hosted CI job that runs `scripts/package.sh`, and passing
  Windows/macOS jobs (the added `build-check` job has not run yet).
- Windowed runs on Linux and Windows; non-ASCII installation paths on
  every platform.
