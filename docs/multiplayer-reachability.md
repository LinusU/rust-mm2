# Multiplayer reachability runbook and report (F24-AC06, F24-AC01)

Owner-run evidence for the lobby protocol (`mm2_net`). Loopback runs
prove the protocol; **they say nothing about LAN or Internet support.**
This page separates the three scopes and holds the report template.
F24-AC06 stays open until the LAN and Internet tables below are filled
in with real results.

## What the network does

- One **TCP** listener, on the single address passed to `--bind`, for the
  lobby and all reliable traffic. No discovery broadcast: clients need
  the exact `host:port`.
- One **UDP** socket (NETPLAY-06A), bound on the same IP as the TCP
  listener with an **ephemeral port** (`Host::lane_port()`). It carries
  the sequenced unreliable datagram lane: the client's `InputWindow`
  stream and the host's `Snap` once a client has negotiated a lane
  (`Client::join_lane`). The client learns the port from the host over
  TCP (`LaneGrant`), together with a per-seat token every datagram must
  carry; datagrams from any other address, seat, token or session
  generation are dropped and counted.
- **Firewall / port forward requirement:** inbound UDP to the host's lane
  port, from every client, must be allowed — in addition to the TCP
  port. The lane port is ephemeral today, so on a host behind a
  firewall or NAT either allow the process's UDP traffic or open a
  range; a fixed lane port is not configurable yet. The client's own UDP
  socket needs only outbound UDP plus the replies NAT/stateful firewalls
  allow back. The lane expects datagrams from the exact UDP source port
  the client announced, so a NAT that rewrites the source port is not
  supported.
- A client that joins with a lane and gets no echo within 3 s fails the
  join with `no UDP lane echo from <host>:<port> within 3s ... allow
  inbound UDP on that port` rather than carrying on over TCP: a TCP-only
  data plane (`Client::join`, `Transport::Tcp`) is not equivalent
  quality (head-of-line blocking) and is never presented as such.
- **Path MTU:** the establishing probe and its echo are padded to the
  largest lane datagram (1200 bytes, `MAX_DATAGRAM`), so a path that
  cannot carry that — a VPN or tunnel with a small MTU, a filtering
  middlebox — fails the join with the same `no UDP lane echo` error
  (which names the MTU) instead of establishing and then silently losing
  every full-size `Snap`. Loopback proof: `lane_impair_tests`.
- **Evidence level:** loopback tests (`crates/mm2_net/src/lane_tests.rs`)
  prove the code path only. They say nothing about whether UDP crosses a
  real LAN, router or the Internet; that stays an OWNER measurement
  (#1249) — fill in the LAN/Internet tables below with the lane in use.
- The host binds loopback + ephemeral port unless told otherwise. A LAN
  or public bind is always an explicit `--bind`. Use a **fixed port**
  (e.g. `:27015`) for LAN/Internet runs so firewall and port-forward
  rules can name it.
- Handshake timeout 10 s; keepalive 1 s; a silent host is declared dead
  by a client after 5 s (`LIVENESS_TIMEOUT`). The host declares a silent
  lane client `Lost` after 8 s (`HOST_LIVENESS_TIMEOUT`, designed): every
  authenticated datagram counts, and an idle client sends a lane probe
  each second so a quiet lobby seat is never reaped. A TCP-only join has
  no lane to hear and is only reaped when a write fails
  (`crates/mm2_net/src/{conn,lobby,rt}.rs`).
- Every machine must mount an install and mods with the same gameplay
  fingerprint (`fnv1a64:...`, printed in the host's `listening=` record
  and the client's `connected=` record). A mismatch is `join_failed`.

## Commands

`$MM2` is the retail install path on each machine. Build once per
machine: `cargo build --release -p mm2_app`; binaries are
`target/release/{mm2-host,mm2-join,mm2}`.

Host (headless; stdin takes `start`, `cancel`, `quit`):

```sh
mm2-host --mm2-path "$MM2" --city sf --bind 0.0.0.0:27015
# or one explicit interface:  --bind 192.168.1.20:27015
# no city content needed:     --dev-world
```

Read `listening=`, `lane_port=` (the **UDP** port the lane listens on —
allow it inbound as well as the TCP port) and `fingerprint=` from its
first line. `0.0.0.0` prints the wildcard; clients connect to the host's
real LAN or public address.

Headless client (prints `connected=`, `event=roster`, `event=started`;
`connected=` carries `transport=lane` and the lane datagram counters. A
blocked UDP path ends the join with `event=join_failed` and an
`error: no UDP lane echo ... allow inbound UDP on that port` line on
stderr; the windowed menu shows the same text):

```sh
mm2-join --mm2-path "$MM2" --connect HOST_IP:27015 --driver alice --vehicle "" --ready
```

`--vehicle ""` picks the synthetic dev car; give a real `<id>[:paint]`
for a retail car. Type `quit` on stdin to leave cleanly.

Windowed client:

```sh
mm2 --mm2-path "$MM2" --join HOST_IP:27015 --driver alice --ready
```

The host may also be the full app: `mm2 --mm2-path "$MM2" --city sf --host --bind 0.0.0.0:27015`
(stdin `start`/`cancel`/`quit`).

Start the round with `start` on the host once at least the clients under
test show `ready`. Pass: the host prints `event=started generation=1`
and every client prints `event=started generation=1`.

## Scope 1: same machine (agent-verified, loopback only)

Run on 2026-10-10 against the retail install, `--dev-world`,
`--bind 127.0.0.1:0`, host plus two `mm2-join` processes (`alice`,
`bob`), both `--vehicle "" --ready`:

- Both joined (ids 1, 2), roster reached `players=2 ready=2`, host
  `start` printed `event=started generation=1`, `bob` received
  `event=started generation=1`.
- `alice` was told `quit` before `start` and was logged as
  `event=left ... cause=quit`; when the host then quit, `bob` reported
  `event=closed` and exited 1 (documented exit code for a lost host).

Level reached: synthetic integration with retail-mounted content, no
windowed client, no real network. The automated multi-process tests
cover the same ground in CI, including (NETPLAY-06F) late join into a
running session, late join behind a lossy lane, and a killed client
reaped as `cause=lost` (2.3 ms on loopback, because the kernel closes
its sockets) with the session running on:
`crates/mm2_app/tests/net_lifecycle.rs`, table in
`docs/netcode-redesign.md` "Lifecycle on the lane".

## Scope 2: LAN (owner fills in)

Host machine A and client machine B on the same subnet; neither goes
through a NAT between them. Run each row; record host and client output.

| # | Host bind | Client | Expected | Result | Notes |
|---|-----------|--------|----------|--------|-------|
| L1 | `A_LAN_IP:27015` | `mm2-join` on B | joined, ready, started | | |
| L2 | `0.0.0.0:27015` | `mm2-join` on B | same | | |
| L3 | `A_LAN_IP:27015` | `mm2 --join` on B | joined, lobby shows both | | |
| L4 | `A_LAN_IP:27015` | `mm2-join` on B with OS firewall on at A, no rule | blocked: `join_failed` after ~10 s | | record the OS prompt/behaviour |
| L5 | as L1, after allowing `mm2-host` in the firewall | `mm2-join` on B | joined | | |
| L6 | as L1, pull B's cable/Wi-Fi mid-lobby | — | host `left ... cause=lost` within ~8 s (host-side silence bound) | | |
| L7 | as L1, `mm2-join` with a different mod set | — | `join_failed` (fingerprint) | | |

## Scope 3: Internet (owner fills in)

Host behind a home router; client outside the NAT (mobile hotspot or a
remote machine). The host needs a port-forward of **TCP 27015 and the UDP `lane_port=`** to its
LAN address, plus an OS firewall allowance. Record the router model,
whether the ISP uses CGNAT (WAN address differs from the public address
shown by a "what is my IP" site: forwarding cannot work), and IPv4/IPv6.

| # | Setup | Client | Expected | Result | Notes |
|---|-------|--------|----------|--------|-------|
| I1 | TCP 27015 forwarded to A, bind `0.0.0.0:27015` | `mm2-join --connect PUBLIC_IP:27015` from outside | joined, ready, started | | |
| I2 | as I1 | `mm2 --join PUBLIC_IP:27015` from outside | joined | | |
| I3 | no port-forward | `mm2-join` from outside | `join_failed` after ~10 s (timeout) or refused | | note which |
| I4 | forwarded; host on CGNAT | — | not reachable; note it | | |
| I5 | as I1, drop the client's network mid-lobby | — | `left ... cause=lost` within ~8 s (host-side silence bound) | | |
| I6 | as I1, client from a different NAT behind Wi-Fi hotspot | `mm2-join` | joined | | |

## Report template

Copy this block into the task or a PR. Fill every field; leave "not
run" rather than guessing.

```text
Date:
Build (git sha, both machines):
Fingerprint (host / client):
Host OS + firewall state:
Client OS + firewall state:
Network: same machine / LAN subnet / Internet (router, CGNAT yes/no, v4/v6)
Port used + forwarding rule:
Commands run (exact):
Same-machine result:
LAN rows L1-L7:
Internet rows I1-I6:
NAT/firewall behaviour observed (silent drop, refusal, prompt):
Time to failure when blocked:
Gaps / not run:
```

## Claims

- Same-machine success supports F24-AC01 at synthetic-integration
  level only.
- LAN support may be claimed only from filled-in L rows from two
  physical machines.
- Internet support may be claimed only from filled-in I rows with a
  client outside the host's NAT. Loopback or LAN success never counts.
