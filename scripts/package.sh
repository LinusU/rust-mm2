#!/bin/sh
# Assemble the distributable folder and tarball, then check what actually
# ships (F30 requirement 4/6, AC04-AC06).
#
#   scripts/package.sh [--bin-dir <dir>] [--out <dir>] [--forbid <text>]...
#
# Without `--bin-dir` it runs `scripts/release-build.sh` (a locked release
# build with the builder's paths remapped and stripped) and takes the four
# binaries from `target/release/`. With it, the binaries are taken from
# that directory as they are — the check below still decides whether they
# may ship. The package is
#
#   <out>/rust-mm2-<os>-<arch>/        (default <out>: target/package)
#       mm2 mm2-host mm2-join mm2-inspect   (.exe on Windows)
#       assets/                        the project's own synthetic assets
#       docs/building.md docs/modding.md, README.md
#
# and `<out>/rust-mm2-<os>-<arch>.tar.gz` beside it. The folder is
# checked with `scripts/check-package.sh`, and then so is the tarball
# *after extracting it into a scratch directory*, because the archive is
# what gets distributed. Nothing is read from an original installation,
# and nothing is overwritten: an existing package folder or tarball is an
# error (exit 2), left for you to look at and remove.
#
# Only the Unix tarball is produced. A Windows zip and a macOS `.app`
# bundle are not built yet (docs/building.md). `--forbid` is passed to the
# private-path scan.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

bin_dir=
out=target/package
forbid=
while [ $# -gt 0 ]; do
    case "$1" in
        --bin-dir) [ $# -ge 2 ] || { echo "error: --bin-dir needs a value" >&2; exit 2; }; bin_dir=$2; shift 2 ;;
        --out) [ $# -ge 2 ] || { echo "error: --out needs a value" >&2; exit 2; }; out=$2; shift 2 ;;
        --forbid) [ $# -ge 2 ] || { echo "error: --forbid needs a value" >&2; exit 2; }; forbid="${forbid}--forbid
$2
"; shift 2 ;;
        -h|--help) sed -n '2,26p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
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
stage="$out/$name"
tarball="$out/$name.tar.gz"
if [ -e "$stage" ] || [ -e "$tarball" ]; then
    echo "error: $stage or $tarball already exists; remove it first" >&2
    exit 2
fi

for bin in mm2 mm2-host mm2-join mm2-inspect; do
    if [ ! -f "$bin_dir/$bin$exe" ]; then
        echo "error: $bin_dir/$bin$exe is missing" >&2
        exit 2
    fi
done

mkdir -p "$stage/docs"
for bin in mm2 mm2-host mm2-join mm2-inspect; do
    cp "$bin_dir/$bin$exe" "$stage/"
done
cp -R assets "$stage/assets"
cp README.md "$stage/"
cp docs/building.md docs/modding.md "$stage/docs/"
# A stray editor/OS file in the source assets must not ride along.
find "$stage" -name .DS_Store -type f -exec rm {} +

check() {
    IFS='
'
    # shellcheck disable=SC2046,SC2086 # newline-separated by construction
    set -f; set -- $forbid "$1"; set +f
    unset IFS
    sh scripts/check-package.sh "$@"
}

check "$stage"

# macOS tar would add AppleDouble `._*` entries for extended attributes.
COPYFILE_DISABLE=1 tar -C "$out" -czf "$tarball" "$name"

scratch=$(mktemp -d "${TMPDIR:-/tmp}/rust-mm2-package.XXXXXX")
trap 'rm -rf "$scratch"' EXIT
tar -C "$scratch" -xzf "$tarball"
[ -d "$scratch/$name" ] || { echo "error: the tarball does not unpack to $name/" >&2; exit 1; }
check "$scratch/$name"

echo "packaged: $tarball"
