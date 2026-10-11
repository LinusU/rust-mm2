#!/bin/sh
# Reproducible benchmark set (F30-AC01): the same named scenes, bot-driven,
# release build, vsync off, one CSV + mm2-perf-report/1 JSON per scene.
#
#   MM2_RETAIL=<install> scripts/bench.sh [out-dir] [frames]
#
# Needs a display and GPU; needs the original install (read only). Output
# defaults to target/bench. Reports from different commits, builds or
# hardware are not comparable (see README "Frame-time profiling").
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
: "${MM2_RETAIL:?set MM2_RETAIL to the original install}"
out=${1:-target/bench}
frames=${2:-1500}
mkdir -p "$out"

cargo build --release -p mm2_app --bin mm2
bin=${CARGO_TARGET_DIR:-target}/release/mm2

for scene in sf london; do
    echo "== $scene ($frames frames)"
    "$bin" --mm2-path "$MM2_RETAIL" --city "$scene" --car vpauditt \
        --bot --frames "$frames" --no-vsync \
        --perf-log "$out/$scene.csv" 2>&1 | tee "$out/$scene.summary.txt" | tail -40
done
echo "reports: $out/*.csv.report.json"
