#!/bin/sh
# Fail when a distributable folder holds anything it should not, or lacks
# something it needs (F30 requirement 6: clean artifacts without original
# content, private paths, saves or captures).
#
#   scripts/check-package.sh [--forbid <text>]... <package-dir>
#
# The folder is judged against an allowlist, not a blocklist: a file the
# package is not meant to carry fails even if nobody thought to forbid it.
# Allowed:
#
#   mm2, mm2-host, mm2-join, mm2-inspect   (all four, all `.exe` or none)
#   README.md, LICENSE*                    (README.md is required)
#   docs/<name>.md
#   assets/**.{png,tga,ktx2,ogg,wav,txt}   (assets/texture/dev_road.png is
#                                           required; each file < 8 MiB)
#
# Named classes fail with their own message first, because they are the
# mistakes worth naming: original archives (`*.ar`), profiles
# (`driver-*`, `settings.json`), perf captures (`*.csv`, `*.report.json`),
# logs and credentials (`*.log`, `.env*`, `*.pem`, `*.key`). Symlinks and
# special files fail: a link can point out of the package. Finally every
# file is scanned with `scan-private-paths.sh` (`--forbid` is passed on).
#
# Exit status: 0 clean, 1 a problem was found, 2 usage error. Every
# problem is listed, not just the first.
set -u
set -f # the newline-split lists below must never glob

here=$(cd "$(dirname "$0")" && pwd)
scan_args=
dir=
while [ $# -gt 0 ]; do
    case "$1" in
        --forbid)
            [ $# -ge 2 ] || { echo "error: --forbid needs a value" >&2; exit 2; }
            scan_args="${scan_args}--forbid
$2
"
            shift 2
            ;;
        -h|--help)
            sed -n '2,28p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        -*)
            echo "error: unknown option $1" >&2
            exit 2
            ;;
        *)
            [ -z "$dir" ] || { echo "error: one package directory only" >&2; exit 2; }
            dir=$1
            shift
            ;;
    esac
done

if [ -z "$dir" ] || [ ! -d "$dir" ]; then
    echo "error: need an existing package directory" >&2
    exit 2
fi
dir=${dir%/}
max_asset_bytes=8388608

problems=0
problem() {
    echo "PROBLEM: $1" >&2
    problems=$((problems + 1))
}

nl='
'

# Special files and symlinks first: they never belong, and the loop below
# only walks regular files.
odd=$(find "$dir" -mindepth 1 ! -type d ! -type f)
old_ifs=$IFS
IFS=$nl
for path in $odd; do
    problem "symlink or special file: ${path#"$dir"/}"
done

files=$(find "$dir" -mindepth 1 -type f | sort)
count=0
exe_count=0
plain_count=0
for path in $files; do
    count=$((count + 1))
    rel=${path#"$dir"/}
    base=${rel##*/}
    case "$rel" in
        driver-*|*/driver-*) problem "profile or settings: $rel"; continue ;;
    esac
    case "$base" in
        *.ar) problem "original game archive: $rel"; continue ;;
        settings.json) problem "profile or settings: $rel"; continue ;;
        *.report.json|*.csv) problem "perf capture: $rel"; continue ;;
        *.log) problem "log file: $rel"; continue ;;
        .env*|*.pem|*.key) problem "credential-shaped file: $rel"; continue ;;
    esac
    case "$rel" in
        mm2|mm2-host|mm2-join|mm2-inspect) plain_count=$((plain_count + 1)) ;;
        mm2.exe|mm2-host.exe|mm2-join.exe|mm2-inspect.exe) exe_count=$((exe_count + 1)) ;;
        README.md|LICENSE|LICENSE-*|LICENSE.*) ;;
        docs/*.md)
            case "${rel#docs/}" in
                */*) problem "not on the allowlist: $rel" ;;
            esac
            ;;
        assets/*)
            case "$base" in
                *.png|*.tga|*.ktx2|*.ogg|*.wav|*.txt)
                    size=$(wc -c < "$path" | tr -d ' ')
                    if [ "$size" -ge "$max_asset_bytes" ]; then
                        problem "asset over 8 MiB ($size bytes): $rel"
                    fi
                    ;;
                *) problem "asset type not on the allowlist: $rel" ;;
            esac
            ;;
        *) problem "not on the allowlist: $rel" ;;
    esac
done
IFS=$old_ifs

# The program: all four binaries, in one flavour.
if [ "$exe_count" -gt 0 ] && [ "$plain_count" -gt 0 ]; then
    problem "binaries of both flavours (.exe and plain)"
fi
if [ "$exe_count" -gt 0 ]; then
    suffix=.exe
else
    suffix=
fi
for bin in mm2 mm2-host mm2-join mm2-inspect; do
    if [ ! -f "$dir/$bin$suffix" ]; then
        problem "missing binary: $bin$suffix"
    elif [ -z "$suffix" ] && [ ! -x "$dir/$bin" ]; then
        problem "binary is not executable: $bin"
    fi
done
[ -f "$dir/README.md" ] || problem "missing README.md"
[ -f "$dir/assets/texture/dev_road.png" ] || problem "missing assets/texture/dev_road.png"

if [ "$count" -gt 0 ]; then
    # Scan whatever there is, so a private path is reported alongside the
    # other problems rather than hidden behind them.
    set --
    IFS=$nl
    for path in $files; do
        set -- "$@" "$path"
    done
    IFS=$old_ifs
    old_ifs=$IFS
    IFS=$nl
    # shellcheck disable=SC2086 # newline-separated by construction
    set -- $scan_args "$@"
    IFS=$old_ifs
    sh "$here/scan-private-paths.sh" "$@" >/dev/null
    case $? in
        0) ;;
        1) problem "a file embeds a private path (see the lines above)" ;;
        *) echo "error: the private-path scan could not run" >&2; exit 2 ;;
    esac
fi

if [ "$problems" -ne 0 ]; then
    echo "package NOT clean: $problems problem(s) in $count file(s)" >&2
    exit 1
fi
echo "package clean: $count files"
