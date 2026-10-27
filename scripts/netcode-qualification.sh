#!/usr/bin/env bash
# NETPLAY-10E: the congestion qualification run.
#
# A real host and seven real lane clients on loopback, every client behind
# a bandwidth-capped drop-tail relay (1 Mbit/s down, 256 kbit/s up), the
# host's 5 Mbit/s uplink cap dipping to 2 Mbit/s and back, clustered loss,
# a seat that stops reading, then tagged recovery legs (process pauses,
# blackouts, burst loss). Prints the report and leaves it in
# target/captures/netcode-qualification/congestion.txt; exits non-zero when
# a gate fails. About one minute of wall time after the build; run it on an
# otherwise idle machine, because its gates are real-time. Loopback only:
# see docs/netcode-redesign.md "NETPLAY-10E".
set -euo pipefail
cd "$(dirname "$0")/.."
exec cargo test -p mm2_net --release congest_qual -- --ignored --nocapture
