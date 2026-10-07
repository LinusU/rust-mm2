#!/bin/sh
# Fail when a build artifact embeds a path from the machine that built it
# (F30 requirement 6: no private paths in a distributable artifact).
#
#   scripts/scan-private-paths.sh [--forbid <text>]... <file>...
#
# Rust writes the absolute path of every source file it can panic in into
# the binary — the registry copy of each dependency, the toolchain's std
# sources, the checkout — so a plain `cargo build --release` carries the
# builder's home directory. `scripts/release-build.sh` remaps those
# prefixes; this is the check that the remap held.
#
# With no `--forbid` the forbidden strings are this machine's $HOME,
# $CARGO_HOME, $RUSTUP_HOME and the repository root. Passing `--forbid`
# replaces that default list (a test, or a CI runner with other roots).
#
# Exit status: 0 clean, 1 a forbidden string was found, 2 usage error or
# an unreadable file. Matches are counted, never printed in full, so the
# report itself does not leak the path into a log.
set -u

forbidden=
files=
explicit=0
while [ $# -gt 0 ]; do
    case "$1" in
        --forbid)
            [ $# -ge 2 ] || { echo "error: --forbid needs a value" >&2; exit 2; }
            explicit=1
            forbidden="${forbidden}${2}
"
            shift 2
            ;;
        -h|--help)
            sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        -*)
            echo "error: unknown option $1" >&2
            exit 2
            ;;
        *)
            files="${files}${1}
"
            shift
            ;;
    esac
done

if [ -z "$files" ]; then
    echo "error: no files to scan" >&2
    exit 2
fi

if [ "$explicit" -eq 0 ]; then
    root=$(cd "$(dirname "$0")/.." && pwd)
    forbidden="${HOME:-}
${CARGO_HOME:-${HOME:-}/.cargo}
${RUSTUP_HOME:-${HOME:-}/.rustup}
${root}
"
fi

found=0
checked=0
old_ifs=$IFS
IFS='
'
for file in $files; do
    if [ ! -f "$file" ] || [ ! -r "$file" ]; then
        echo "error: cannot read $file" >&2
        exit 2
    fi
    idx=0
    for pat in $forbidden; do
        idx=$((idx + 1))
        # A bare "/" or a one-character value would match every binary.
        [ "${#pat}" -ge 3 ] || continue
        checked=$((checked + 1))
        n=$(grep -a -o -F -e "$pat" "$file" | wc -l | tr -d ' ')
        if [ "$n" -gt 0 ]; then
            echo "FOUND ${n}x forbidden string #${idx} (${#pat} chars) in $file" >&2
            found=1
        fi
    done
done
IFS=$old_ifs

if [ "$checked" -eq 0 ]; then
    echo "error: no usable forbidden strings (HOME unset?)" >&2
    exit 2
fi
if [ "$found" -ne 0 ]; then
    exit 1
fi
echo "clean: no private path in the scanned files"
