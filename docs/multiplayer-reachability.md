# Multiplayer reachability runbook and report (F24-AC06, F24-AC01)

Owner-run evidence for the lobby protocol (`mm2_net`). Loopback runs
prove the protocol; **they say nothing about LAN or Internet support.**
This page separates the three scopes and holds the report template.
F24-AC06 stays open until the LAN and Internet tables below are filled
in with real results.

## What the network does

- One **TCP** listener, on the single address passed to `--bind`. No
  UDP, no second port, no discovery broadcast: clients need the exact
  `host:port`.
- The host binds loopback + ephemeral port unless told otherwise. A LAN
  or public bind is always an explicit `--bind`. Use a **fixed port**
  (e.g. `:27015`) for LAN/Internet runs so firewall and port-forward
  rules can name it.
- Handshake timeout 10 s; keepalive 1 s; a silent peer is declared dead
  after 5 s (`crates/mm2_net/src/{conn,lobby}.rs`).
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

Read `listening=`, `fingerprint=` from its first line. `0.0.0.0` prints
the wildcard; clients connect to the host's real LAN or public address.

Headless client (prints `connected=`, `event=roster`, `event=started`):

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
cover the same ground in CI.

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
| L6 | as L1, pull B's cable/Wi-Fi mid-lobby | — | host `left ... cause=lost` within ~5 s | | |
| L7 | as L1, `mm2-join` with a different mod set | — | `join_failed` (fingerprint) | | |

## Scope 3: Internet (owner fills in)

Host behind a home router; client outside the NAT (mobile hotspot or a
remote machine). The host needs a port-forward of **TCP 27015** to its
LAN address, plus an OS firewall allowance. Record the router model,
whether the ISP uses CGNAT (WAN address differs from the public address
shown by a "what is my IP" site: forwarding cannot work), and IPv4/IPv6.

| # | Setup | Client | Expected | Result | Notes |
|---|-------|--------|----------|--------|-------|
| I1 | TCP 27015 forwarded to A, bind `0.0.0.0:27015` | `mm2-join --connect PUBLIC_IP:27015` from outside | joined, ready, started | | |
| I2 | as I1 | `mm2 --join PUBLIC_IP:27015` from outside | joined | | |
| I3 | no port-forward | `mm2-join` from outside | `join_failed` after ~10 s (timeout) or refused | | note which |
| I4 | forwarded; host on CGNAT | — | not reachable; note it | | |
| I5 | as I1, drop the client's network mid-lobby | — | `left ... cause=lost` within ~5 s | | |
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
