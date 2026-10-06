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
