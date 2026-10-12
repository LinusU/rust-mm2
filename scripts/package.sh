#!/bin/sh
# Assemble the distributable layouts and check what actually ships
# (F30 requirement 4/6, AC04-AC06).
#
#   scripts/package.sh [--layout <flat|all|app|prefix>] [--bin-dir <dir>] [--out <dir>] [--forbid <text>]...
#
# Without `--bin-dir` it runs `scripts/release-build.sh` (a locked release
# build with the builder's paths remapped and stripped) and takes the four
# binaries from `target/release/`. With it, the binaries are taken from
# that directory as they are — the check below still decides whether they
# may ship.
#
# `--layout` picks the install layouts to assemble; it defaults to `all`,
# every layout this platform ships (`.app` is macOS-only):
#
#   flat    <out>/rust-mm2-<os>-<arch>/      the portable folder, plus a
#           .tar.gz beside it: the binaries, assets/, README.md,
#           LICENSE-MIT, LICENSE-APACHE, docs/
#   app     <out>/rust-mm2.app/              a macOS bundle:
#           Contents/MacOS/{mm2,...}, Contents/Resources/assets/,
#           Contents/Resources/{README.md,LICENSE-*,docs/}, plus a .tar.gz
#   prefix  <out>/rust-mm2-<os>-<arch>-prefix/  a Unix prefix:
#           bin/{mm2,...}, share/rust-mm2/assets/,
#           share/doc/rust-mm2/{README.md,LICENSE-*,docs/}, plus a .tar.gz
#
# Every folder is checked with `scripts/check-package.sh` in its own
# layout, and then so is its tarball *after extracting it into a scratch
# directory*, because the archive is what gets distributed. Nothing is
# read from an original installation, and nothing is overwritten: an
# existing package folder or tarball is an error (exit 2), left for you to
# look at and remove.
#
# `--forbid` is passed to the private-path scan.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

bin_dir=
out=target/package
layout=all
forbid=
while [ $# -gt 0 ]; do
    case "$1" in
        --bin-dir) [ $# -ge 2 ] || { echo "error: --bin-dir needs a value" >&2; exit 2; }; bin_dir=$2; shift 2 ;;
        --out) [ $# -ge 2 ] || { echo "error: --out needs a value" >&2; exit 2; }; out=$2; shift 2 ;;
        --layout)
            [ $# -ge 2 ] || { echo "error: --layout needs a value" >&2; exit 2; }
            case "$2" in
                flat|app|prefix|all) layout=$2 ;;
                *) echo "error: unknown layout $2 (flat, app, prefix or all)" >&2; exit 2 ;;
            esac
            shift 2
            ;;
        --forbid) [ $# -ge 2 ] || { echo "error: --forbid needs a value" >&2; exit 2; }; forbid="${forbid}--forbid
$2
"; shift 2 ;;
        -h|--help) sed -n '2,33p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "error: unknown argument $1" >&2; exit 2 ;;
    esac
done

if [ -z "$bin_dir" ]; then
    sh scripts/release-build.sh
    bin_dir=target/release
fi

exe=
case "$(uname -s)" in
    MINGW*|MSYS*|CYGWIN*) exe=.exe ;;
esac
os=$(uname -s | tr '[:upper:]' '[:lower:]')
name="rust-mm2-${os}-$(uname -m)"

# Which layouts this run assembles. A macOS `.app` only means something on
# macOS; asking for one anywhere else is a usage error rather than a
# package of foreign binaries.
case "$layout" in
    all)
        case "$os" in
            darwin) layouts="flat app prefix" ;;
            *) layouts="flat prefix" ;;
        esac
        ;;
    app)
        [ "$os" = darwin ] || { echo "error: the .app bundle is macOS-only (this is $os)" >&2; exit 2; }
        layouts=app
        ;;
    *) layouts=$layout ;;
esac

for bin in mm2 mm2-host mm2-join mm2-inspect; do
    if [ ! -f "$bin_dir/$bin$exe" ]; then
        echo "error: $bin_dir/$bin$exe is missing" >&2
        exit 2
    fi
done

flat_dir="$out/$name"
flat_tar="$out/$name.tar.gz"
app_dir="$out/rust-mm2.app"
app_tar="$out/rust-mm2.app.tar.gz"
prefix_dir="$out/$name-prefix"
prefix_tar="$out/$name-prefix.tar.gz"

mkdir -p "$out"

# Nothing is ever overwritten: leaving one run's files under the next
# run's name mixes them, so an existing folder or tarball stops the script
# for the operator to look at.
for want in $layouts; do
    case "$want" in
        flat) existing_dir=$flat_dir existing_tar=$flat_tar ;;
        app) existing_dir=$app_dir existing_tar=$app_tar ;;
        prefix) existing_dir=$prefix_dir existing_tar=$prefix_tar ;;
    esac
    if [ -e "$existing_dir" ] || [ -e "$existing_tar" ]; then
        echo "error: $existing_dir or $existing_tar already exists; remove it first" >&2
        exit 2
    fi
done

copy_bins() {
    for bin in mm2 mm2-host mm2-join mm2-inspect; do
        cp "$bin_dir/$bin$exe" "$1/"
    done
}

# $1 holds README.md and the licenses, $2 the two docs.
copy_docs() {
    cp README.md LICENSE-MIT LICENSE-APACHE "$1/"
    cp docs/building.md docs/modding.md "$2/"
}

# A stray editor/OS file in the tree must not ride along.
strip_os_files() {
    find "$1" -name .DS_Store -type f -exec rm {} +
}

stage_flat() {
    mkdir -p "$flat_dir/docs"
    copy_bins "$flat_dir"
    cp -R assets "$flat_dir/assets"
    copy_docs "$flat_dir" "$flat_dir/docs"
    strip_os_files "$flat_dir"
}

stage_app() {
    mkdir -p "$app_dir/Contents/MacOS" "$app_dir/Contents/Resources/docs"
    copy_bins "$app_dir/Contents/MacOS"
    cp -R assets "$app_dir/Contents/Resources/assets"
    copy_docs "$app_dir/Contents/Resources" "$app_dir/Contents/Resources/docs"
    strip_os_files "$app_dir"
}

stage_prefix() {
    mkdir -p "$prefix_dir/bin" "$prefix_dir/share/rust-mm2" "$prefix_dir/share/doc/rust-mm2/docs"
    copy_bins "$prefix_dir/bin"
    cp -R assets "$prefix_dir/share/rust-mm2/assets"
    copy_docs "$prefix_dir/share/doc/rust-mm2" "$prefix_dir/share/doc/rust-mm2/docs"
    strip_os_files "$prefix_dir"
}

check() { # $1 = layout, $2 = folder
    IFS='
'
    # shellcheck disable=SC2046,SC2086 # newline-separated by construction
    set -f; set -- --layout "$1" $forbid "$2"; set +f
    unset IFS
    sh scripts/check-package.sh "$@"
}

scratch=$(mktemp -d "${TMPDIR:-/tmp}/rust-mm2-package.XXXXXX")
trap 'rm -rf "$scratch"' EXIT

# Check the folder, archive it, then extract the archive and check that
# too — the tarball is what gets distributed.
publish() { # $1 = layout, $2 = folder, $3 = tarball, $4 = top-level name
    check "$1" "$2"
    # macOS tar would add AppleDouble `._*` entries for extended attributes.
    COPYFILE_DISABLE=1 tar -C "$out" -czf "$3" "$4"
    unpack="$scratch/$4"
    rm -rf "$unpack"
    mkdir -p "$unpack"
    tar -C "$unpack" -xzf "$3"
    [ -d "$unpack/$4" ] || { echo "error: the tarball does not unpack to $4/" >&2; exit 1; }
    check "$1" "$unpack/$4"
    echo "packaged: $3"
}

for want in $layouts; do
    case "$want" in
        flat) stage_flat; publish flat "$flat_dir" "$flat_tar" "$name" ;;
        app) stage_app; publish app "$app_dir" "$app_tar" "rust-mm2.app" ;;
        prefix) stage_prefix; publish prefix "$prefix_dir" "$prefix_tar" "$name-prefix" ;;
    esac
done
