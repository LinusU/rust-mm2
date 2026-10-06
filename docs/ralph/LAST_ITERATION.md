# Last iteration — world-clock replication for Cruise (new-run iteration 7)

Selection: the previous review passed with no blocking findings, so no
repair was owed. I first considered the review's "per-client relevancy"
gap for traffic and dropped it: the host's population is capped at
`SpawnPolicy::max_active` = 32, already under the 64-row wire bound, so
the id-ordered truncation can never drop a car today (a mod raising the
cap would need it; recorded, not built). The next F26-A gap that is real
today is the one the iteration-5 clock audit named and the iteration-6
race-row re-seek only half closed: **a Cruise has no race row**, so a
Cruise client — and above all a late joiner (AC02) — had nothing to align
its timed scenery (drawbridge leaves, boats, ferries, trains) to.

Change (protocol **v20**): `Message::World { generation, ticks }`
(tag 0x12, 17 B, host→client only; a client-sent one drops the peer
`Malformed`, AC04). `mm2_app::worldclock`: `publish_world_clock` (host;
`Countdown`/`Playing`/`Results`; at once, then every
`PUBLISH_EVERY_TICKS` = 120 world ticks, immediately if the clock went
backwards — so a pause stops the frames), `WorldStage` (inside
`RemoteSnaps`; newest tick per generation, ≤4 generations, older/equal
dropped `stale`, foreign generation refused at apply and never able to
stale-mark the session's own, a tick past `MAX_SEEK_TICKS` = 2^21 refused
*before* it can become a watermark because a seek replays every actor from
its start), `apply_world_clock` (client; held through `Loading`/`Paused`)
→ the existing `WorldClock::sync` (6-tick tolerance). Wired in main.rs,
smoke.rs and the `net_app` harness. Record gains ` wclk=sent,landed,seek,
ref` (absent when the wire carried none; `world=` was already taken by the
city name). Docs: `docs/research/net.md` (new section + budget row), PLAN
F26-A slice 7.

Tests: proto round-trip/truncation/padding; lobby
`a_client_cannot_assert_the_world_clock`; 5 `WorldStage` units (per-
generation newest, foreign generation, implausible tick not a watermark,
bounded generations, reset); `net_app::the_host_publishes_its_world_clock_
at_the_cadence` (real loopback socket: first frame at once, none a tick
short of the cadence — the next frame on the wire is the one at the
cadence — and a restart announced at once) and
`net_app::a_world_clock_frame_re_seeks_a_cruise_clients_scenery` (late
joiner lands on the host's tick, reordered older frame dropped, jitter
inside tolerance lands without a seek, foreign generation refused, absurd
tick refused without poisoning the honest frame after it). All spin on a
condition under a bounded deadline (report 6 rule).

Original-data / process evidence (separate from the synthetic tests):
`MM2_RETAIL=/Users/linus/coding/rust-mm2/retail cargo test --locked -p
mm2_app --test network two_retail_processes_replicate_the_hosts_traffic`
(now also asserts the clock): two real `mm2` processes on loopback,
retail sf Cruise, one Apple Silicon machine: host `wclk=sent15`, client
`wclk=sent0,landed15,seek13,ref0`, both `status=pass`. The 13 seeks are
**not** evidence of alignment quality: the headless harness free-runs
both processes at unrelated update rates, so the clocks diverge between
frames. How rarely a vsync-bound client seeks is unobserved. Not
rendered, not impaired.

Gates (all exit 0): `cargo fmt --all -- --check` PASS; `cargo clippy
--locked --workspace --all-targets --all-features -- -D warnings` PASS (no
new allow); `cargo test --locked --workspace` PASS (1991 passed, 0 failed;
was 1982). The retail two-process leg was re-run on the final code and
passed. No test processes left running.

Still open (F26-A stays active, not AC01..06 completion): no RTT
compensation (a client trails the host by the one-way delay; past ~50 ms
it re-seeks every frame), proximity leaves are still per-peer, no
windowed/GPU or impaired-network scenery measurement, per-client traffic
relevancy (moot at the default cap), signal heads, interpolation beyond
the velocity carry, mid-session weather, late-join of props beyond the
resend cycle, sound replication. Status: implemented candidate, not
independently checked.

---

# Last iteration — ambient-traffic replication (new-run iteration 6)

Selection: the previous review passed with no blocking findings, so no
repair was owed. F26-A's remaining largest gap against AC01 ("two clients
see the same relevant traffic") was that a networked session fielded *no*
ambient traffic at all (the MP-4 gate in `load_ambient_traffic`), so there
was nothing to share. Weather/time-of-day already agree (they ride
`Start`'s session config, which a late joiner also gets); props landed in
iterations 2–5.

Policy decision (recorded, nobody to ask — DSN-71): MP-4 documents "no
ambient traffic, cops or AI opponents in MP *races*" and names no Cruise
exception, while the F26 spec wants Cruise clients to share traffic. So
`traffic::fields_ambient_traffic` = offline always, networked only in
free-roam Cruise (enhanced policy, original unverified); networked races
keep none on both sides. The `Host` simulates; a `Remote` client holds
copies.

Change (protocol **v19**): `Message::Traffic { generation, tick, roster,
rows }` (host→client only; a client-sent one drops the peer, AC04) with
`SnapCar { id, class, state, pos, rot, vel }`, ≤64 rows (`MAX_SNAP_CARS`).
`mm2_app::worldtraffic`: `publish_traffic` (host-minted per-spawn ids via
`TrafficLedger::collect`, every third frame, roster digest on each frame),
`TrafficStage` (per-car latest-wins on `(generation, tick)`, staged ≤256,
applied ledger ≤16,384, held through `Loading`), `apply_traffic` +
`TrafficReplica` (client roster/class cache; kinematic `TrafficCopy`
bodies with the class model, collider and ambient engine table; roster
mismatch refused counted; velocity carry; snap past 3 m; TTL retirement
after 240 ticks; ≤128 copies). Record gains ` cars=sent,omit,live,landed,
mism` (absent while the wire carried no car). Docs: `docs/research/net.md`
(budget 26 B + 47 B/row), ledger DSN-71 + MP-4 note, PLAN F26-A slice 6.

Tests: proto round-trip/bound/truncation (mm2_net); lobby
`a_client_cannot_assert_traffic`; 7 `TrafficStage`/digest unit tests;
`traffic::networked_cruise_traffic_is_the_hosts_and_a_clients_is_a_replica`
(replaces the old "networked spawns none" test: Host cruise fields it,
Remote cruise holds only the replica, networked races none, Local race and
roam unchanged); `traffic::a_client_copies_the_hosts_traffic_and_retires_it_when_frames_stop`
(production row collector → frame codec → `apply_traffic`: one copy per
car, same class/pose, follows the host's motion as the *same entities*,
foreign roster refused, copies survive a short silence and retire after
the TTL). That leg caught a real bug — the retention pass forgot copies
spawned in the same run (queued in `Commands`), so every frame
duplicated the whole population; fixed with a `known` set.

Original-data / process evidence (separate from the synthetic tests):
operator-run `MM2_RETAIL=/Users/linus/coding/rust-mm2/retail cargo test
--locked -p mm2_app --test network two_retail_processes_replicate` — two
real `mm2` processes on loopback, one Apple Silicon machine, one binary,
retail sf Cruise: host `cars=sent4539,omit0`; client
`cars=sent0,omit0,live16,landed4316,mism0`, both `status=pass`. Not
rendered/audible evidence (headless) and not impaired.

Gates (all exit 0): `cargo fmt --all -- --check` PASS; `cargo clippy
--locked --workspace --all-targets --all-features -- -D warnings` PASS (no
new allow); `cargo test --locked --workspace` PASS (1982 passed, 0 failed;
was 1970). The retail two-process leg was re-run on the final code and
passed. No test processes left running.

Still open (F26-A stays active, not AC01..06 completion): per-client
relevancy (broadcast; bounded by the host's interest union), signal heads
not replicated, no measured impairment cell / windowed or real-GPU leg for
`Traffic` or `Props`, interpolation beyond the velocity carry, mid-session
weather (static per session), late-join of props beyond the resend cycle,
drawbridge/mover/sound replication, a client's contact with a copy is
predicted against the copy's last pose. Status: implemented candidate, not
independently checked.

---

# Last iteration — retail two-process site-table evidence + staging repair (new-run iteration 5)

Selection: the previous review passed with no blocking findings. Its
largest verification gap — no two-process retail run showing host and
client report equal `SiteTable`s — is the next evidence step for F26-A,
and its two staging-looseness notes are cheap, in-file repairs.

Repairs (`worldprops::PropStage`): the host's table is now held *per
generation* (≤4, oldest evicted) and replaced only by a frame of that
generation at least as new in tick — so a reordered older frame cannot
carry a stale, partly stamped table in, and a newer-generation frame no
longer decides the older one's rows (the review's "falls through to
agreed" note: a drained row's generation always has its own table, and a
row whose table is gone is refused counted as `mismatched`). A first
attempt that kept one global table broke the existing
`a_client_folds_prop_rows_into_its_stamped_world` leg (an interleaved
foreign-generation frame poisoned current rows) — caught by the full
suite, redesigned. Tests: `a_reordered_older_frame_cannot_replace_the_hosts_table`,
`each_generation_keeps_its_own_table`,
`rows_whose_generation_table_was_evicted_are_refused`.

Evidence plumbing: `NetDriveReport` gains `prop_world` (own table),
`props_landed`, `props_mismatched`, written by `publish_props`
(host) / `apply_props` (client); the headless record gains
` props=sites<count>:<digest>,landed<n>,mism<n>` (absent while nothing is
stamped, so other records stay identical). New operator-run test
`network::net_drive::two_retail_processes_stamp_the_same_prop_world`
(skips without `MM2_RETAIL=<install>`): `mm2 --host --city sf` + `mm2
--join`, both headless, loopback.

Result (retail install at `/Users/linus/coding/rust-mm2/retail`, one
Apple Silicon machine, one binary): host and client both printed
`props=sites5953:ce16a67de227adeb`; in a longer manual 4000-frame run the
client landed 3657 rows with `mism0`; the test run passed (7.5 s).
Evidence level: real processes, real loopback, retail world — site
ordinals agree *on the same platform*. Cross-platform agreement (the
quantised-home rounding-boundary concern) is still unobserved; name-only
digest is the recorded fallback if it ever diverges.

Gates: `cargo fmt --all -- --check` exit 0; `cargo clippy --locked
--workspace --all-targets --all-features -- -D warnings` exit 0 (one more
targeted `too_many_arguments` allow, on `apply_props`); `cargo test --locked
--workspace` exit 0 (1970 passed, 0 failed); the retail test re-run on the
final code passed. No test processes left running.

Still open (unchanged): measured impairment cell or real-GPU leg for
`Props`; interpolation/velocities; traffic/weather/time-of-day;
late-join beyond the resend cycle; drawbridge/mover/sound replication.
Not F26-AC01..06 completion. Status: implemented candidate, not
independently checked.

---

# Last iteration — world-agreement check for replicated props (new-run iteration 4)

Selection: the previous review passed with no blocking findings, so no
repair was owed. Its first verification gap — "site-ordinal agreement
between host and client … any stamp-time difference between peers (model
-load failure, content mismatch) would silently misattribute rows; there
is no checksum or site-count handshake" — is the highest-value ready
small slice of F26-A, and a silent wrong-prop pose is worse than no
pose.

Change (protocol **v18**): `Message::Props` carries `SiteTable { count,
digest }`. `mm2_app::worldprops::SiteRegistry` records each stamped
placement (ordinal, authored name, home position quantised to 0.25 m,
taken from `Added<BangerSite>` before any impact can move it, scoped to
the session generation) and hashes them in ordinal order (FNV-1a). The
host puts its table on every frame (`PropLedger.sites`); a client keeps
its own in `PropStage` and, on drain, compares: a disagreement drops the
whole drain counted as `mismatched` (distinct from `unresolved`), applies
nothing, logs one warning and sets `PropStage::divergence()`; the first
frame whose table agrees resumes replication. All-or-nothing by design —
with a shifted ordinal no row can be trusted. Rotation is excluded from
the digest (trig-derived, platform-sensitive); the 0.25 m quantum is a
design choice (Implementation choice) so cross-platform last-bit float
noise hashes alike. Docs: `docs/research/net.md` (budget header 18→30 B),
ledger DSN-70, PLAN F26-A slice 4.

Tests: proto round-trip/bounds/truncation carry the table;
`SiteRegistry` unit tests (order- and noise-insensitive; shifted,
renamed, moved, missing and gapped worlds all differ; generation reset);
`PropStage` mismatch drain; `net_app::a_client_refuses_rows_from_a_host_
with_a_different_world` (client with two of the host's three placements
poses nothing, then recovers under an agreeing table); the convergence
leg now asserts `mismatched()==0`, no divergence and a 4-placement
table; the publish-window leg asserts every frame's table equals the
22-placement world's. Existing legs adjusted so the host stamps props at
their homes and moves them afterwards (identity is the home pose).

Gates (all exit 0): `cargo fmt --all -- --check`; `cargo clippy --locked
--workspace --all-targets --all-features -- -D warnings` (one targeted
`too_many_arguments` allow on the `publish_props` Bevy system);
`cargo test --locked --workspace` (1966 passed, 0 failed). Retail sf
`--headless --frames 300` smoke (`/Users/linus/coding/rust-mm2/retail`):
`status=pass`, `bng=6110d`. No test processes left running.

Not verified / open: the check is exercised on synthetic stamps and the
in-process loopback harness only — a two-process retail run where host
and client report equal `SiteTable`s is still the next evidence step
(retail agreement not observed, so the ordinals-agree claim on real
worlds stays unverified); no measured impairment cell or real-GPU leg
for `Props`; no interpolation/velocities; traffic/weather/time-of-day,
late-join beyond the resend cycle, drawbridge/mover/sound replication
remain open. Not F26-AC01..06 completion. Status: implemented
candidate, not independently checked.

---

# Last iteration — world-prop replication repair (new-run iteration 3)

**Recovery of review rejection for iteration 2 (F26-A).** Root cause
(implementation): `PropStage::drain_for` recorded every drained
current-generation row in the unbounded `applied` watermark map, resolved
or not, so a host streaming ever-new `(site, fragment)` keys grew client
memory for the whole session while docs claimed a bound. Fix: `applied`
is now written only by `PropStage::remember`, called from `apply_props`
for rows that resolved against the local world, and is hard-capped at
`MAX_APPLIED_PROPS` (16,384; at the cap a new prop is simply
unwatermarked, harmless since phases only move forward). Regression
tests: `unresolvable_rows_never_grow_the_applied_ledger` (drains >4,096
distinct unresolvable keys across frames, ledger stays 0) and
`the_applied_ledger_has_a_hard_cap_even_for_resolved_rows`. Also guarded
the reviewer's `BangerFragment.index` gap: clamp is now 254 so a
pathological set can never alias the 255 placement sentinel. Still open
(unchanged, reviewer-listed): two-process retail site-agreement check,
measured impairment/real-GPU legs for `Props`. Status: implemented
candidate, not independently checked. Results of this repair's gates are
at the end of this file.

---

# Previous iteration — world-prop replication (new-run iteration 2)

Selection: F26-A, the prop half of F26-AC01. The previous review passed
with no blocking findings, so no repair work was owed. Of the operator's
networking follow-ups (report 6) the world-clock slice had landed; the
largest remaining F26-A gap was that **a client's props never move at
all** — `activate_bangers`/`settle_bangers` are authority-only and the
module doc promised "replication (F26) delivers authoritative
`BangerStateChanged`", which nothing did. Proximity leaves / latency
compensation stay open as recorded (no RTT, no retail `prox` path).

Design decision (recorded here, nobody to ask): replicate *state*, not
events, as its own frame. `Message::Props` (protocol v17, host→client
only) instead of a new `Snap` field, which would have touched 54 test
constructors for no gain and ties props to the pose stream's watermark.
Identity is a new `BangerSite` stamp ordinal (`Session::mint_banger_site`),
**not** `ObjectId`, whose slots interleave with vehicles/remote seats/
fragments in per-process order. Fragments are named `(site, piece index)`
via a `BangerFragment{parent,index}` tag. Bounded and loss-tolerant: every
active body + fresh changes + a rolling 8-row resend window of
settled/broken, every 2nd `Update`, ≤96 rows; client inbox ≤4,096,
latest-wins per prop on `(generation, tick)`, held through `Loading`,
phases never regress. Details: `docs/research/net.md`, ledger DSN-70.

Code: `mm2_net::proto` (`SnapProp`, `Message::Props`, `MAX_SNAP_PROPS`,
`OversizeProps`), `mm2_game` (`BangerSite`, `BangerFragment`,
`Session::mint_banger_site`), `mm2_app::banger` (`shatter_placement`,
`spawn_fragment` shared by authority and client; fragments tagged),
`city::spawn_banger_prop` (site stamp), new `mm2_app::worldprops`
(`publish_props`, `apply_props`, `PropStage` inside `RemoteSnaps`), wired
into `main.rs`, `smoke.rs` and the `net_app` harness.

Tests added: proto round-trip/bounds/truncation (mm2_net 3 + lobby 1:
a client-sent `Props` drops the peer, AC04); `mint_banger_site` is
independent of object-slot order; real pathset stamp gives consecutive
sites and fragments carry piece tags (`tests/banger.rs`); 7 `PropStage`
unit tests; 4 `net_app` legs (client rows incl. stale/unknown/NaN/foreign
generation/fragment-past-pieces, load-time hold, publish-window bound and
cycle, two-app host→client convergence over a real loopback socket with
the production systems). New legs spin under bounded deadlines on the
condition, never fixed frame counts (report 6 rule).

Gates (all exit 0): `cargo fmt --all -- --check`; `cargo clippy --locked
--workspace --all-targets --all-features -- -D warnings`; `cargo test
--locked --workspace` (mm2_app lib 143, app suite 763, network suite 85,
mm2_net 86 …, 0 failed). Retail sf `--headless --frames 300` smoke
(`/Users/linus/coding/rust-mm2/retail`): `status=pass`, `bng=6110d`
dormant bangers stamped — no regression from the site stamp. No test
processes left running.

Not verified / open (F26-A stays active, not AC01..06 completion): no
interpolation or velocities (active props move at the publish rate); no
measured impairment cell or two-process/real-GPU leg for `Props`; site
agreement across peers rests on deterministic stamp order (all `Vec`
iteration, seeded parked-car rolls) and was checked only on synthetic
stamps — a two-process retail check that both sides report the same
site→position table is the next evidence step; traffic/weather/time-of-
day replication; late-join beyond the resend cycle (~sites/8 frames);
drawbridge/mover/sound state still clock-only. Status: implemented
candidate, not independently checked.

## Repair gate results (iteration 3)

`cargo fmt --all -- --check` exit 0; `cargo clippy --locked --workspace
--all-targets --all-features -- -D warnings` exit 0; `cargo test --locked
--workspace` exit 0 (1962 passed, 0 failed; includes the 2 new
`worldprops` ledger tests). No retail/graphical run this iteration. No
test processes left running.
