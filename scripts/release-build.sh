#!/bin/sh
# Build the release binaries without the builder's paths in them, then
# prove it (F30 requirement 6).
#
#   scripts/release-build.sh
#
# Builds `mm2`, `mm2-host`, `mm2-join` and `mm2-inspect` into
# `target/release/` from the committed lockfile. Cargo's own `trim-paths`
# profile option is still nightly-only, so the same effect is had by
# remapping the three prefixes Rust embeds — the checkout, the cargo
# registry and the rustup toolchain — through RUSTFLAGS, and the result
# is checked by `scan-private-paths.sh`. Any RUSTFLAGS you already set
# are kept.
#
# Remapping is not enough on its own: the linker also records the path
# of every object file it read as a debug-map entry (macOS `N_OSO`,
# which names the toolchain's `libstd` archive under the rustup home),
# and rustc's own debuginfo strip does not remove it. The binaries are
# therefore stripped of debug information afterwards (`strip -S` on
# macOS, `strip --strip-debug` on Linux; symbol names stay, so a profiler
# still reads them) and, on macOS, re-signed ad hoc, which Apple Silicon
# requires of any modified binary.
#
# Nothing here reads or copies an original installation; the output is
# the program only. The base assets it needs live in `assets/` and are
# placed beside the binary by hand (see docs/building.md).
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

cargo_home="${CARGO_HOME:-$HOME/.cargo}"
rustup_home="${RUSTUP_HOME:-$HOME/.rustup}"

RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=${root}=/rust-mm2 --remap-path-prefix=${cargo_home}=/cargo --remap-path-prefix=${rustup_home}=/rustup"
export RUSTFLAGS

cargo build --locked --release -p mm2_app -p mm2_inspect

exe=
os=$(uname -s)
case "$os" in
    MINGW*|MSYS*|CYGWIN*) exe=.exe ;;
esac
bins="target/release/mm2${exe} target/release/mm2-host${exe} target/release/mm2-join${exe} target/release/mm2-inspect${exe}"

# A Windows build records no object-file paths in the .exe (its PDB is a
# separate file that is not shipped), so only Unix needs the strip.
for bin in $bins; do
    case "$os" in
        Darwin)
            strip -S "$bin"
            codesign --force --sign - "$bin"
            ;;
        Linux)
            strip --strip-debug "$bin"
            ;;
    esac
done

# shellcheck disable=SC2086 # the list is words by construction
sh scripts/scan-private-paths.sh $bins
