#!/usr/bin/env bash
# NETPLAY-10 residual (#2158): the congestion qualification with real mm2
# processes (one headless --paced host, seven headless --paced clients, each
# lane behind its own bandwidth-capped UdpImpairProxy), ordinary envelope
# and recovery legs. Gates are real-time: run on an otherwise idle machine.
# Loopback only. Output is kept in target/captures/netcode-qualification/.
set -euo pipefail
cd "$(dirname "$0")/.."
out=target/captures/netcode-qualification
mkdir -p "$out"
cargo test -p mm2_app --release --test network net_qualify -- --ignored --nocapture --test-threads=1 2>&1 | tee "$out/congestion-mm2.txt"
exit "${PIPESTATUS[0]}"
