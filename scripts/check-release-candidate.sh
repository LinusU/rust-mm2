#!/bin/sh
# F30-AC06: run the documented game-rule and scene-correctness headless
# evidence commands against a release candidate and, with `--baseline`,
# against the unoptimized build, and compare the smoke records.
#
#   scripts/check-release-candidate.sh --bin <mm2> [--baseline <mm2>] [--out <dir>]
#
# The candidate is normally the `mm2` a package carries (release profile,
# stripped): point `--bin` at `<package>/mm2`. The baseline is the
# development build (`target/debug/mm2`). Two binaries that print the same
# records for the same commands show the optimised build preserves the
# game rules and scene content the commands exercise — the command's
# status, the world it loaded and every subsystem counter must agree.
#
# The `dev-` commands need no original data. The city commands read the
# install in `MM2_RETAIL`; without it they are reported `skipped`, never
# counted as a pass. This is original-content / local evidence (F30-AC06),
# so it is not run from hosted CI.
#
# Exit status: 0 every command ran and matched, 1 a command failed,
# differed or was skipped, 2 a usage error.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

bin=
baseline=
out=target/release-candidate
while [ $# -gt 0 ]; do
    case "$1" in
        --bin) [ $# -ge 2 ] || { echo "error: --bin needs a value" >&2; exit 2; }; bin=$2; shift 2 ;;
        --baseline) [ $# -ge 2 ] || { echo "error: --baseline needs a value" >&2; exit 2; }; baseline=$2; shift 2 ;;
        --out) [ $# -ge 2 ] || { echo "error: --out needs a value" >&2; exit 2; }; out=$2; shift 2 ;;
        -h|--help) sed -n '2,21p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "error: unknown argument $1" >&2; exit 2 ;;
    esac
done

[ -n "$bin" ] || { echo "error: --bin <mm2> is required" >&2; exit 2; }
[ -x "$bin" ] || { echo "error: $bin is not executable" >&2; exit 2; }
if [ -n "$baseline" ]; then
    [ -x "$baseline" ] || { echo "error: $baseline is not executable" >&2; exit 2; }
fi

mkdir -p "$out"
retail=${MM2_RETAIL:-}

# One record per command: the last `smoke=` line on stdout/stderr, with
# the terminal colour codes removed. A run that prints no record is an
# error the caller must see.
record() {
    # shellcheck disable=SC2086 # the caller passes the argument word list
    "$1" ${2} 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | grep '^smoke=' | tail -n 1
}

pass=0
fail=0
skipped=0

# run_case <name> <needs-retail 0|1> <binary> <args...>
run_case() {
    name=$1 needs_retail=$2 exe=$3
    shift 3
    if [ "$needs_retail" = 1 ] && [ -z "$retail" ]; then
        echo "== $name: skipped (MM2_RETAIL is not set)"
        skipped=$((skipped + 1))
        return
    fi
    args="--mm2-path $retail $*"
    [ "$needs_retail" = 1 ] || args=$*
    # shellcheck disable=SC2086 # `args` is a word list by construction
    line=$(record "$exe" "$args") || true
    if [ -z "$line" ]; then
        echo "== $name: FAIL (no smoke record)"
        fail=$((fail + 1))
        return
    fi
    echo "$line" > "$out/$name.$(basename "$exe").log"
    case "$line" in
        *status=pass*) ;;
        *) echo "== $name: FAIL ($line)"; fail=$((fail + 1)); return ;;
    esac
    if [ -n "$baseline" ]; then
        base=$(record "$baseline" "$args") || true
        echo "$base" > "$out/$name.baseline.log"
        if [ "$line" != "$base" ]; then
            echo "== $name: DIFFER"
            echo "  candidate: $line"
            echo "  baseline:  $base"
            fail=$((fail + 1))
            return
        fi
    fi
    echo "== $name: pass"
    pass=$((pass + 1))
}

# dev world: no original data. The hold driver settles and drives; the
# scripted driver steers, so the impact/damage/wheel paths run too.
run_case dev-hold 0 "$bin" --dev-world --headless --frames 600
run_case dev-bot 0 "$bin" --dev-world --headless --bot --frames 600

# retail city: scene content and the game rules the record exposes
# (collision, pedestrians, traffic, signals, movers, damage, race rules).
run_case sf-cruise 1 "$bin" --city sf --headless --frames 600
run_case london-blitz-hold 1 "$bin" --city london --event blitz:0 --headless --frames 2400
run_case london-blitz-bot 1 "$bin" --city london --event blitz:0 --headless --bot --frames 2400
run_case sf-checkpoint-bot 1 "$bin" --city sf --event checkpoint:0 --headless --bot --frames 3000

echo "--"
echo "candidate: $bin"
if [ -n "$baseline" ]; then
    echo "baseline:  $baseline"
fi
echo "pass=$pass fail=$fail skipped=$skipped"
[ "$fail" -eq 0 ] && [ "$skipped" -eq 0 ]
