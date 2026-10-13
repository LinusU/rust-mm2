#!/bin/sh
# Race every checkpoint and circuit event headlessly with the player
# parked and record what the opponents did.
#
#   MM2_RETAIL=<install> scripts/opponent_sweep.sh <mm2-binary> <out-dir> [jobs] [frames]
#
# Writes one `<city>-<table>-<row>-<a|p>.txt` smoke record per race under
# <out-dir>, then prints the totals: opponents finished / fielded and
# re-anchor teleports, per difficulty. Copy the binary somewhere stable
# first — rebuilding under a running sweep mixes two builds.
set -eu
bin=$1
out=$2
jobs=${3:-4}
frames=${4:-21600}
: "${MM2_RETAIL:?set MM2_RETAIL to the original install}"
mkdir -p "$out"
export MM2_BIN="$bin" MM2_OUT="$out" MM2_FRAMES="$frames"
for city in sf london; do
    for event in $(seq -f 'race:%g' 0 11) $(seq -f 'circuit:%g' 0 9); do
        for diff in a p; do
            echo "$city $event $diff"
        done
    done
done | xargs -P "$jobs" -L 1 sh -c '
    name=$(echo "$0-$1-$2" | tr ":" "-")
    pro=""
    [ "$2" = p ] && pro="--pro"
    "$MM2_BIN" --mm2-path "$MM2_RETAIL" --city "$0" --event "$1" $pro \
        --headless --parked --no-profile --frames "$MM2_FRAMES" 2>/dev/null \
        | grep "^smoke=" > "$MM2_OUT/$name.txt" || echo "smoke=missing" > "$MM2_OUT/$name.txt"
'
for diff in a p; do
    cat "$out"/*-"$diff".txt | awk -v diff="$diff" '
        {
            for (i = 1; i <= NF; i++) {
                if ($i ~ /^opp=/) { split(substr($i, 5), f, "/"); done += f[1]; all += f[2] }
                if ($i ~ /^opp_rec=/) rec += substr($i, 9)
            }
            races++
        }
        END { printf "%s: %d races, opponents finished %d/%d, re-anchors %d\n", diff, races, done, all, rec }'
done
