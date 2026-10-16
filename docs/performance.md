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
more events and cities, and an explanation of the client stall's exact
trigger remain open.

### Longer and wider soak (2026-10-16)

`scripts/soak.sh` now reads `SOAK_CITY`, `SOAK_EVENT` and `SOAK_LEGS`
(`race`, `network`, or both; default `sf`, `checkpoint:0`, both), so a run
on another event is `SOAK_CITY=london SOAK_EVENT=circuit:0 SOAK_LEGS=race
scripts/soak.sh target/soak-x 10000`. Every row below is 10,000 requested
frames, release, vsync off, Apple M1, shared machine, one run each, base
`main` at `6432c0d` plus the script change. Every verdict is `pass`
(including the zero-discarded-game-time overload check) and the script
exited 0 in all four runs. "Entities" is the recorder's first sample and the
last sample (the verdict compares the middle and last third peaks, which are
listed after it).

| event | process | entities first / last (thirds' peaks mid / last) | voices first / last (thirds' peaks) | queue depth |
|---|---|---|---|---|
| sf checkpoint:0 | race | 47,010 / 47,228 (47,228 / 47,228) | 17 / 36 (36 / 36) | 1 / 0 |
| sf checkpoint:0 | host | 40,783 / 40,813 (40,836 / 40,821) | 10 / 15 (18 / 17) | 1 / 0 |
| sf checkpoint:0 | client | 40,785 / 40,843 (40,942 / 40,939) | 10 / 15 (17 / 17) | 1 / 0 |
| london checkpoint:0 | race | 48,449 / 48,594 (48,598 / 48,598) | 17 / 36 (39 / 39) | 1 / 0 |
| london checkpoint:0 | host | 40,803 / 40,831 (40,862 / 40,866) | 9 / 16 (20 / 19) | 1 / 0 |
| london checkpoint:0 | client | 40,804 / 40,841 (40,958 / 40,942) | 9 / 15 (16 / 16) | 1 / 0 |
| sf circuit:0 | race | 47,074 / 47,212 (47,218 / 47,273) | 12 / 25 (28 / 32) | 1 / 0 |
| london circuit:0 | race | 48,902 / 49,075 (49,597 / 49,206) | 19 / 42 (55 / 47) | 1 / 0 |

Reading it: entity counts move by at most a few hundred over 10,000 frames
and the last third never exceeds the middle by more than the slack (about
1%); voices fluctuate with what is audible (peaks 16 to 56) and fall or stay
level between the two judged thirds; queue depth never exceeded 1. The
earlier 3000-frame client stall (one 1135 ms render stage) did not recur in
either network client at 10,000 frames; it needed GPU contention to
reproduce and is placed in the next subsection. The Circuit legs are
single-process only: no networked Circuit run exists
(`docs/multiplayer-coverage.md`).

Evidence level: original-content, rendered, local, one machine, one run per
cell, bot-driven. A leak under about 1% of the entity peak per third would
still hide inside the slack. F30-AC02 stays open for a LAN or multi-machine
network run, repeated runs for variance, and the client stall's exact
trigger (next subsection).

### Explaining the client's 1135 ms render-stage stall (#1473, 2026-10-16)

The 2026-10-12 client leg failed on one 1135 ms `render`-stage frame, and
the stage alone says only *that* the main thread spent 1.1 s in `render`,
not *where*. Bevy renders on a second thread: the main schedule runs first,
then the main thread blocks handing the render world back and forth
(`bevy_render::pipelined_rendering`, `renderer_extract` after `Main`), so a
shader/pipeline compile, a render-asset GPU upload, a driver wait and an OS
preemption all hide inside that one stage.

`perf.rs` now splits the render thread's own schedule. Stub systems stamp
the `RenderSystems` set boundaries into a clock the main world shares, and
each row records the split the render thread last published (the one that
row's `render` stage waited on). The CSV gains `rthread_total_ms`,
`rthread_extract_ms`, `rthread_assets_ms`, `rthread_views_ms`,
`rthread_draw_ms` and `rthread_cleanup_ms`; the report gains
`timings.render_stage_ms` (`worst_frame` and `per_segment_max`). One
frame's segments add up to its `total`.

In order: `extract_commands` (`ExtractCommands`); `prepare_assets`
(`PrepareAssets`), where a lazily loaded mesh/texture is uploaded to the
GPU; `views` (`PrepareMeshes`..`Prepare`), material specialization,
shadow-view creation, phase sort and bind groups; `render` (the `Render`
set), `PipelineCache::process_pipeline_queue_system` then `render_system`
— the render graph, command-buffer submit and `window.present`; and
`cleanup`. Read a stall by comparing the render thread's worst `total` with
the main `render` stage max: if they match, `per_segment_max` names the
segment it stalled in; if the main max is far above the render thread's
worst, the block was the main thread or the OS (`renderer_extract`, or a
descheduled process) and no render work explains it.

A fresh loopback soak at 3000 frames passed with no stall (single pair;
client median frame 24 ms, render thread median `total` 22.7 ms, split
`views` 9.3 + `render` 11.8 + `prepare-assets` 1.0 + `extract` 0.2 +
`cleanup` 0.1 ms; worst measured frame 55.5 ms, still `views`). The only
large frames were the first two (warm-up, excluded from the stats): frame 1
was `prepare-assets` 160.8 + `views` 177.6 ms, the initial GPU upload and
specialization, which is exactly where a lazy asset lands.

A three-pair contention soak (six GPU-bound processes on one 8-core
machine, load ~8) reproduced the stall class and the instrumentation named
it. On the clients, over the measured frames (`total` is the render
thread's whole schedule, `render` the `Render` set, all in ms):

| client | frame | main `render` | render-thread `total` = extract + assets + views + **render** + cleanup |
|---|---|---|---|
| pair3 | 428 | 8887.5 | 8930.9 = 3.3 + 1.5 + 26.8 + **8899.3** + 0.0 |
| pair1 | 247 | 9734.7 | 38.3 = 0.2 + 0.8 + 27.3 + **10.0** + 0.1 |
| pair1 | 1664 | 265.5 | 356.7 = 0.1 + 1.3 + 21.9 + **322.5** + 0.1 |
| pair2 | 1706 | 213.1 | 217.8 = 0.0 + 0.5 + 165.1 + **32.4** + 0.0 |

Both classes appear. Frame 428 is a render-thread stall in the `Render`
set — the render graph, submit or `window.present` — at 8899 ms the largest
part of the 8931 ms; frame 247 is the opposite, a 9.7 s main-thread/OS
stall during which the render thread did only 38 ms of work. The
`render`-segment stalls seen span 213-8899 ms, bracketing the original
1135 ms. The hosts, drawing the same event, never exceeded 230 ms on the
main `render` stage.

So the 2026-10-12 1135 ms client stall is not game logic (its `fixed` and
`update` stages were normal, 26 / 20 ms) and not an asset load
(`prepare_assets` stayed under 2 ms): it is a GPU/driver/display wait in
the render thread's `Render` set, the same class as the frame-428 stall, or
less often a main-thread/OS stall like frame 247. The two are now told
apart by the report instead of by guesswork. The stall did not recur at one
pair in two 3000-frame runs (the 2026-10-12 run and one fresh run);
reproducing it took six concurrent GPU-bound processes, so on the soak's
normal single pair it stays rare.

Evidence level: original-content, rendered, local, one machine, one event
and one car, release, 3000 requested frames. The reproducing load is a
stress condition, not the soak's normal single pair.

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

## Headless fixed tick on sf (#1396, 2026-10-13)

The headless smoke (`mm2 --headless --city sf`, one car, retail install)
cost ~22 ms wall per 60 Hz tick in the dev profile, which is what made the
local retail network legs take 10-25 minutes each.

```sh
target/debug/mm2 --headless --mm2-path "$MM2_RETAIL" --city sf --frames 1000
```

Profile: `sample <pid> 15 -file <out>` against a running 1500-frame
headless run (macOS' `xctrace` Time Profiler would not record this
binary), summed over the "Sort by top of stack" table by crate. Of the
non-idle samples, **parry3d was 57%** and avian3d 16% (convex-polyhedron
support points, GJK/EPA, trimesh BVH traversal: Avian's narrow phase);
the rest was spread thin (allocator 8%, mm2_vehicle 5%). The parked task-pool
workers (`__psynch_cvwait`, 72% of all samples) are waiting, not spinning;
the "59 s of CPU" in the issue was real narrow-phase work spread over the
pool.

Cause: `MM2_SMOKE_DIAG=1` showed **42,924 active contact pairs at tick 250**
with one car, about 40,000 of them `Static` against `Static`
(`proprule-*`, `pathset-*`, `parkedcar-*` against `city-ground-*` and
`city-room*`). A dormant banger is a static *sensor*
(`banger.rs`, `banger_bundle`), and Avian skips static-static broad-phase
pairs only for solid colliders, so every dormant prop paired with the city
trimeshes under it and with its neighbours, and ran full manifold generation
each tick for a contact nothing reads.

Fix: dormant bangers carry `GameLayer::dormant_prop()`, which keeps them
off `World` and off each other, and every transition out of `Dormant`
restores `CollisionLayers::default()`.

| sf, `--frames`, dev profile | before | after |
|---|---|---|
| 60 frames, wall | 3.1 s | 1.8 s |
| 1000 frames, wall | 36.1 s (user 83 s) | 9.5-11.5 s (user 12-15 s) |

Both runs were on a loaded machine (load average 8-15, other agents
building), so read the ratio, not the seconds. Release (after the fix only,
same load): 1.5 s for 60 frames, 10.0 s for 1000; with the narrow-phase
waste gone the dev and release builds are within noise of each other, so
the remaining cost is not optimisation level. A profile of the fixed run
afterwards has bevy's `mark_3d_meshes_as_changed_if_their_assets_changed`
as its largest single frame, not a physics function. Dev-profile opt-level
changes were therefore not tried.

Smoke record: the final line is identical but for two dormant parking
meters that no longer activate. Before the change `proprule-meter-r844R-10`
(tick 218) and `proprule-meter-r828R-4` (tick 290) activated on a
`city-ground-m6-flat` "strike" with severity 1.6e-5 and 3.9e-5: the static
ground has infinite striker mass, so any non-zero approach speed clears the
activation limit. That is a defect in the gate rather than original
behaviour (not verified against retail). Fields that moved:
`impacts=137` to `135`, `bng=5895d/12a/4s/4b` to `5897d/12a/2s/4b`,
`bng_ev=2a/4s/4b` to `0a/2s/4b`; `final=(-1316,49.9,620)` and everything
else are unchanged, and repeated runs after the change are identical.

## Still open (F30-AC01 / AC02)

- Per-system timing inside the fixed and update stages to name the
  bottleneck (needed before F30-B optimises anything).
- A baseline on an idle machine, and a scene set that includes a race and
  a networked session (this run covers free-roam SF and London only).
- Draw calls, GPU memory, allocations and network bandwidth columns.
- The client stall's exact trigger: the render-stage stall is now named
  (a `Render`-set GPU/driver/display wait, see "Explaining the client's
  1135 ms render-stage stall"), but which of pipeline compilation, submit
  or `window.present` blocks is not yet split out, and it still needs six
  concurrent GPU-bound processes to reproduce.
- AC02's longer and wider soaks (see above), and the unresolved ~+60 entities per
  600 frames wander seen in the longer SF run noted in the soak history.
