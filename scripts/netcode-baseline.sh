#!/usr/bin/env bash
# NETPLAY-01: record the continuous-driving baseline.
#
# Starts, drives, records and cleans up the three-process production
# scenarios (host + two clients over loopback UDP, headless) and leaves
# machine-readable trajectories and metrics in $MM2_NET_REPORT_DIR
# (default target/captures/netcode-baseline): <scenario>-<seat>.jsonl and
# <scenario>.json. Children are owned by the test harness, which kills
# them by PID on exit; every wait is bounded by the clients' frame budget.
#
#   scripts/netcode-baseline.sh            baseline/report mode
#   MM2_NET_STRICT=1 scripts/netcode-baseline.sh   also gate on Limits::STRICT
#
# Needs no retail install (dev-world). About four minutes.
set -euo pipefail
cd "$(dirname "$0")/.."
exec cargo test -p mm2_app --test network net_baseline -- --nocapture --test-threads=1
