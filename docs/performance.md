# Performance baseline and how to reproduce it

Measurement only (F30-A). Nothing here changes engine behaviour, and no
optimisation has been made on its strength. The instruments are described
in the README's "Frame-time profiling"; this page records one run of the
reproducible set and what it does and does not show.

## Reproduce

```sh
MM2_RETAIL=<original install> scripts/bench.sh [out-dir] [frames]
```

Builds the release `mm2`, then drives `--city sf` and `--city london` with
`--bot --car vpauditt --no-vsync` for `frames` (default 1500) frames each
and writes `<scene>.csv`, `<scene>.csv.report.json` (`mm2-perf-report/1`:
commit, build, CPU/GPU, content fingerprints, settings, percentiles, soak
verdict) and `<scene>.summary.txt` under `out-dir` (default `target/bench`).
It needs a display, a GPU and the original install; hosted CI has none, so
this is original-content / rendered evidence only, local. Do not commit the
outputs.

## Recorded run (2026-10-11)

Apple M1 (8 logical cores, Metal, integrated), macOS, release profile,
1280x720, MSAA 4x, shadows High, vsync off, 60 Hz fixed step, 1500 frames
(first 120 dropped). Content fingerprint for the install used: catalog
`fnv1a64:0711c87e6d11b21a`, gameplay `fnv1a64:3abca35555b5f542`
(5591 files, 28,832,429 bytes). Engine commit `d2d584f` plus the
uncommitted `scripts/bench.sh`.

**Caveat: the machine was shared.** Load average was ~13 on 8 cores while
the run executed (other agents building and testing), so absolute times
are pessimistic and not a baseline to hold future work to. Re-record on an
otherwise idle machine before using the numbers for a regression budget.

| | SF | London |
|---|---|---|
| frame ms median / p95 / p99 / max | 44.3 / 120.2 / 180.0 / 254.8 | 41.9 / 54.8 / 66.7 / 116.8 |
| mean ms per frame: fixed / update / render | 17.8 / 10.3 / 24.6 | 12.2 / 10.2 / 20.1 |
| fixed steps per frame (mean / max) | 3.16 / 15 | 2.55 / 6 |
| Avian ms per step: broad / narrow / solver | 0.06 / 0.35 / 0.41 | 0.07 / 0.26 / 0.33 |
| max contacts | 448 | 324 |
| live entities first / last / max | 43821 / 43734 / 43868 | 43762 / 43767 / 43788 |
| live audio voices first / last / max | 55 / 55 / 59 | 49 / 48 / 52 |
| frames over the 250 ms virtual cap | 2 (42.2 ms discarded) | 0 |
| soak verdict | **fail** (overload only; entities and voices pass) | pass |

## Second run (2026-10-11, lighter load)

Same machine, settings and command as above, engine commit `d26bfff`
(clean tree). Load average was 4-7 on 8 cores during the run (other
agents still active), so this is better than the first run but still not
an idle-machine baseline.

| | SF | London |
|---|---|---|
| frame ms median / p95 / p99 / max | 18.6 / 20.1 / 33.8 / 53.1 | 19.7 / 24.9 / 34.6 / 60.8 |
| mean ms per frame: fixed / update / render | 2.8 / 5.1 / 11.1 | 3.2 / 5.3 / 11.9 |
| fixed steps per frame (mean / max) | 1.14 / 3 | 1.22 / 4 |
| max contacts | 395 | 323 |
| live entities first / last / max | 43849 / 43700 / 43854 | 43696 / 43720 / 43777 |
| live audio voices first / last / max | 57 / 56 / 57 | 49 / 50 / 52 |
| frames over the 250 ms virtual cap | 0 | 0 |
| soak verdict | pass | pass |

The first run's SF soak failure (two frames over the virtual cap) did not
reproduce, which supports it having been machine load. The fixed stage
dropped from 17.8 to 2.8 ms per frame, so the first run's "most of the
fixed-step time is in other systems" figure was largely contention, not a
property of the engine. Evidence level: original-content, rendered, local;
free-roam only (no race, no networked session), so F30-AC02 stays open.

## Populated soak: race and loopback network (F30-AC02)

```sh
MM2_RETAIL=<original install> scripts/soak.sh [out-dir] [frames]   # default 3000
```

Runs a bot-driven SF `checkpoint:0` race, then a loopback `--host` plus
`--join --ready` pair on the same event (each process under `timeout`, killed
by PID on exit), and exits non-zero unless every report's `timings.soak`
verdict is `pass` over a populated run (at least half the requested frames
measured and a peak of 10,000 entities, so a parked lobby cannot pass).

Budgets (declared in `perf.rs`, echoed in each report): the last third's
peak of live entities, audio voices and queue depth (waiting speech lines
plus replicated impact rows not yet applied) must not exceed the middle
third's by more than the slack (entities: 32 or 1% of the peak; voices 8;
queues 16), and the virtual clock may discard no game time. The recorder now
opens only when the session reaches its countdown, so a lobby wait or a load
is not in any window. A leak slower than the slack per third is invisible
to a run this short.

Recorded run (2026-10-12, Apple M1, release, 3000 frames, shared machine,
commit `a21df6d` plus this change): race **pass**; network host **pass**;
network client **fail on overload only** (one 1135 ms render-stage stall,
906 ms of game time discarded; entities, voices and queues passed). The
stall did not recur in the host process and is unexplained: it is recorded,
not waved through. Entities were flat (race 41,820 to 42,444, host 40,715 to
40,729, client 40,847 to 40,796), voices peaked at 38/17/17 and queue depth
never exceeded 1. A 600-frame race run failed the voice check (25 to 34)
because the ambient voices were still filling; at 3000 frames it passed.

Evidence level: original-content, rendered, local, one machine, one event
and one car. The network leg is a two-process loopback, not a LAN; the host
and client each ran a bot. F30-AC02 is advanced, not closed: a longer run,
more events and cities, and an explanation of the client stall remain open.

## What it shows (first run)

- Entity and voice counts are flat in both scenes over 1380 measured
  frames; the soak's only failure is the two SF frames that exceeded the
  virtual-clock cap and so discarded game time (worst fixed-step catch-up:
  15 steps in one frame). Those frames coincide with the machine load, so
  whether an idle machine still overloads is open.
- Avian is not the cost: its three phases sum to about 0.8 ms per fixed
  step (SF) while the whole `fixed` stage takes 17.8 ms over 3.16 steps,
  roughly 5.6 ms per step. Most of the fixed-step time is therefore in
  other fixed-step systems, not collision or solving. The per-system split
  is **not measured**; `perf.rs` only times the three stages and Avian.
- `render` (24.6 / 20.1 ms) is the largest stage and `update` is a steady
  ~10 ms. A `sample` of the SF process (main thread: about 57% in the
  update schedule, the rest in render) agrees with that, but the capture
  was taken under the same load and is dominated by waiting threads, so it
  does not name a function worth optimising. Frustum/OBB intersection and
  shadow specialisation appear among the visible non-idle frames.

## Still open (F30-AC01 / AC02)

- Per-system timing inside the fixed and update stages to name the
  bottleneck (needed before F30-B optimises anything).
- A baseline on an idle machine, and a scene set that includes a race and
  a networked session (this run covers free-roam SF and London only).
- Draw calls, GPU memory, allocations and network bandwidth columns.
- AC02's longer and wider soaks (see above), and the unresolved ~+60 entities per
  600 frames wander seen in the longer SF run noted in the soak history.
