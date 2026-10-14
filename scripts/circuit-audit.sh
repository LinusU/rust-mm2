#!/bin/sh
# Catalog-wide Circuit completability audit (F14-AC01/AC06): every original
# circuit (sf and london, circuit:0..9) run headless with its authored
# opponents and a parked player, one `smoke=headless-physics` record each.
#
#   MM2_RETAIL=<install> scripts/circuit-audit.sh [out-dir] [frames] [jobs]
#
# Needs the original install (read only), no display. Output defaults to
# target/captures/circuit-audit; frames default to 36000 (597 s of sim at
# 60 Hz). The summary prints `opp=<finished>/<fielded>` and the `opps=`
# rows from each record; read the columns in crates/mm2_app/src/smoke.rs.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
: "${MM2_RETAIL:?set MM2_RETAIL to the original install}"
out=${1:-target/captures/circuit-audit}
frames=${2:-36000}
jobs=${3:-5}
mkdir -p "$out"

cargo build -p mm2_app --bin mm2
bin=${CARGO_TARGET_DIR:-target}/debug/mm2

n=0
for city in sf london; do
    for i in 0 1 2 3 4 5 6 7 8 9; do
        "$bin" --mm2-path "$MM2_RETAIL" --city "$city" --event "circuit:$i" \
            --headless --parked --frames "$frames" >"$out/$city-$i.log" 2>&1 &
        n=$((n + 1))
        if [ $((n % jobs)) -eq 0 ]; then wait; fi
    done
done
wait

for f in "$out"/*.log; do
    printf '%s ' "$(basename "$f" .log)"
    grep -o 'status=[a-z]*\|opp=[0-9/]*\|opps=[^ ]*' "$f" | tr '\n' ' '
    echo
done
