#!/bin/sh
# Populated soak (F30-AC02): a bot-driven race and a loopback host + client
# pair, release build, vsync off, one CSV + mm2-perf-report/1 JSON per
# process. Each report's `timings.soak` judges entity, voice and queue growth
# and discarded game time; this script fails when any verdict is `fail`.
#
#   MM2_RETAIL=<install> [SOAK_CITY=sf] [SOAK_EVENT=checkpoint:0] \
#       [SOAK_LEGS="race network"] scripts/soak.sh [out-dir] [frames]
#
# Needs a display, a GPU and the original install (read only). Loopback only;
# every process runs under `timeout` and is killed by PID on exit. Frames
# default to 3000 (about a minute at 60 Hz): the soak's thirds need enough
# samples, and a shorter run is `inconclusive`, which this script also
# reports as not passed. Output defaults to target/soak. Do not commit it.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
: "${MM2_RETAIL:?set MM2_RETAIL to the original install}"
city=${SOAK_CITY:-sf}
event=${SOAK_EVENT:-checkpoint:0}
legs=${SOAK_LEGS:-race network}
out=${1:-target/soak}
frames=${2:-3000}
limit=$(((frames + 1500) / 10 + 240))
mkdir -p "$out"

cargo build --release -p mm2_app --bin mm2
bin=${CARGO_TARGET_DIR:-target}/release/mm2
common="--mm2-path $MM2_RETAIL --car vpauditt --bot --frames $frames --no-vsync"
pids=""
trap 'for p in $pids; do kill "$p" 2>/dev/null || true; done' EXIT

case " $legs " in *" race "*)
echo "== race ($city $event, $frames frames)"
# shellcheck disable=SC2086
timeout "$limit" "$bin" $common --city "$city" --event "$event" \
    --perf-log "$out/race.csv" 2>&1 | tee "$out/race.summary.txt" | tail -40
;; esac

case " $legs " in *" network "*)
# The host and the client count --frames from process start, lobby wait
# included; the recorder opens only once the session counts down, so the
# lobby budget below is slack and not measurement.
lobby=1500
nframes=$((frames + lobby))
echo "== network ($city $event, loopback host + client, $frames frames after the lobby)"
fifo="$out/host.stdin"
rm -f "$fifo" "$out/host.log" "$out/client.log"
mkfifo "$fifo"
# shellcheck disable=SC2086
timeout "$limit" "$bin" --mm2-path "$MM2_RETAIL" --car vpauditt --bot \
    --frames "$nframes" --no-vsync --city "$city" --event "$event" --host \
    --perf-log "$out/host.csv" <"$fifo" >"$out/host.log" 2>&1 &
host=$!
pids="$host"
exec 3>"$fifo"
wait_for() { # file pattern seconds
    for _ in $(seq 1 "$3"); do
        grep -q "$2" "$1" 2>/dev/null && return 0
        sleep 1
    done
    echo "timed out waiting for '$2' in $1"
    tail -20 "$1"
    return 1
}
wait_for "$out/host.log" "listening=" 120
addr=$(sed -n 's/.*listening=\([0-9.]*:[0-9]*\).*/\1/p' "$out/host.log" | head -n 1)
timeout "$limit" "$bin" --mm2-path "$MM2_RETAIL" --car vpauditt --bot \
    --frames "$nframes" --no-vsync --join "$addr" --ready \
    --perf-log "$out/client.csv" >"$out/client.log" 2>&1 &
client=$!
pids="$pids $client"
wait_for "$out/client.log" "joined lobby" 120
echo start >&3
wait "$host" || true
wait "$client" || true
exec 3>&-
grep '^perf:' "$out/host.log" | tail -4
grep '^perf:' "$out/client.log" | tail -4
;; esac

# A verdict only counts for a populated, long-enough run: a `pass` over a
# parked lobby (hundreds of entities) or a few frames proves nothing.
status=0
results=""
case " $legs " in *" race "*) results="race" ;; esac
case " $legs " in *" network "*) results="$results host client" ;; esac
for r in $results; do
    result=$(python3 -I -c '
import json, sys
j = json.load(open(sys.argv[1]))
t = j["timings"]
if not t:
    print("missing (no frames recorded)")
    raise SystemExit
measured = j["run"]["frames_measured"]
entities = t["live_entities"]["max"]
v = t["soak"]["verdict"]
if measured < int(sys.argv[2]) // 2 or entities < 10000:
    v = "unpopulated (%d measured frames, %d peak entities)" % (measured, entities)
print(v)
' "$out/$r.csv.report.json" "$frames" 2>/dev/null || echo "missing")
    echo "soak $r: $result"
    [ "$result" = pass ] || status=1
done
exit $status
