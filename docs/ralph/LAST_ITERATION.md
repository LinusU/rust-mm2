# Last iteration — F27-B.4c (menu leg): offer Cops & Robbers from the main menu (new-run iteration 20)

Selection: the previous review passed with no blocking findings; its gaps
(fmt/clippy absent from `verify.log`, retail-gated test not confirmed to
run, replica path unit-only, no two-process run) are log/two-process
matters I cannot repair here. The match is reachable by CLI and visible
(HUD), so the next smallest honest leg of B.4c is the in-game menu offer
that the HUD slice deliberately waited for.

Change:
- `menu.rs`: root row `Cops & Robbers` → `Screen::CnrCity` (every city,
  disabled with the `net::check_session` reason when its
  `multicopwaypoints.csv` pool is short — the same gate `--cnr --host` and
  joining clients apply; verdict cached in `MenuData`) →
  `Screen::CnrOptions { city, settings }` (Game / Gold weight / Limit
  cycle rows, Left/Right/Enter; `Start match` launches
  `SessionMode::CopsAndRobbers(settings)` single-seat through the same
  `launch` as Cruise). `menu_graphics.rs` titles/side panel updated.
- `mm2_game::cnr_options`: `label()` for variant/gold/limit and wrapping
  `CnrSettings::cycled_{variant,gold,limit}`. Gold/limit names are the
  `mmlang.dll` strings; variant and `No limit` wording are ours.
- Docs: README, `docs/research/menu.md` entry table, ledger CNR-12, PLAN.

Tests: `cnr_options` unit test (wrap, labels); app-level
`the_menu_offers_cops_and_robbers_where_the_city_can_seed_a_round`
(disabled with reason without a pool, enabled with 3 sites, defaults,
cycles incl. Left wrap, launch reaches `Playing` with the picked mode and
a `CnrHost`).

Evidence (recovery iteration 21): iteration 20 left this tree uncommitted
because `cargo test --workspace` failed on its own new unit test — root
cause: the test indexed `MatchLimit::choices()` wrongly (`labels[6]` is
`250 pts`; the list is `None`, 4 time limits, then 100/250/500/1000
points, so 500 is index 7). Test expectation fixed (also asserts index 5 =
`100 pts`); production code unchanged. Results after the fix:
`cargo fmt --all -- --check` pass; `cargo clippy --workspace --all-targets
--all-features -- -D warnings` pass; `cargo test --workspace` exit 0,
2118 passed / 0 failed. Note `cargo test` stops at the first failing
crate, so the earlier run had not exercised later crates.

Not verified / open: menu-hosted lobby offer (menu Multiplayer row is still
F24), commentary, rematch, client carrier-mass prediction, two-process
started match, any screenshot of the new screens (not captured).
F27-AC01..06 open. Status: implemented candidate, not independently checked.

---

# Last iteration — F27-B.4c (HUD leg): the Cops & Robbers match readout (new-run iteration 19)

Selection: the previous review passed with no blocking findings; its
gaps (fmt/clippy absent from `verify.log`, MM2_RETAIL tests skipped
there) are log matters I cannot repair. The match is reachable from the
command line but a player could not see the clock, their points or the
gold's state, so the HUD is the next smallest honest leg of B.4c. The
in-game menu offer stays queued behind it (a menu entry on a match with
no readout would have been a thin button).

Change:
- `mm2_app::cnrhud` (new): `scoreboard_lines(view, me, hz)` (pure) and
  `update_cnr_scoreboard` (reads `CnrHost` on the authority, else
  `CnrReplica`; the local participant is the `PlayerControl::Local` car
  through `participant_id`). Lines: clock/limit, team totals or rank +
  points, the gold's state (carried by you → which marker to take it to,
  by someone, loose, up for grabs) or the result once decided.
  `spawn_cnr_scoreboard` is called from `cnr::start_match`;
  `camera::retarget_hud` pins it (`HudNodes`); the `H` gate hides it.
- Layout, wording and dev-font text are designed (original instrument
  art unrecovered); the data shown matches the documented mode.
- Docs: README, ledger CNR-12, PLAN row.

Tests: 8 `cnrhud` unit tests (clock rounding, timed FFA countdown/rank,
point-limit target and lone driver, team totals + hideout/bank wording,
rival naming, frozen clock + result variants, ordinals, and the system
end to end: host vs replica precedence, `H` gate, no match → hidden);
the `MM2_RETAIL`-gated session test now also asserts one readout with
"COPS & ROBBERS" and "YOU: ROBBERS" in retail sf and none after
teardown.

Evidence: `cargo fmt --all -- --check` PASS; `cargo clippy --locked
--workspace --all-targets --all-features -- -D warnings` exit 0;
`cargo test --locked --workspace` exit 0 (2116 passed, 0 failed; was
2108). `MM2_RETAIL=/Users/linus/coding/rust-mm2/retail cargo test
--locked -p mm2_app --test app cops_and_robbers` ran (0.56 s) and passed.
Graphical: `mm2 --mm2-path <retail> --city sf --cnr cops --cnr-limit
250pts --frames 90 --screenshot` rendered; the panel sits top-right
("COPS & ROBBERS first to 250 / ROBBERS 0 COPS 0 / YOU: ROBBERS 0 pts /
THE GOLD IS UP FOR GRABS"), screenshot kept local (original content).
No audio, no two-process match; the client path is covered by the
replica unit test only. No test processes left.

Not verified / open: in-game menu entry, commentary, rematch, client
carrier-mass prediction, two-process started match; F27-AC01..06 open.
Status: implemented candidate, not independently checked.

---

# Last iteration — F27-B.4c (CLI leg): offer Cops & Robbers from the command line (new-run iteration 18)

Selection: the previous review passed with no blocking findings. Its
gaps (fmt/clippy absent from `verify.log`; client markers and re-seat
only synthetic) are verification-log / two-process matters I cannot
repair from here; the match now starts and seats, so the next ready leg
is making it *reachable*. B.4c (menu, HUD, commentary, rematch) is
broad; the command-line offer is its smallest honest leg (the in-game
menu stays queued).

Change:
- `mm2_game::cnr_options`: `CnrVariant::parse`, `GoldMass::parse`,
  `MatchLimit::parse` (only the host menu's stock values),
  `CnrSettings::parse(variant, gold, limit)` with errors naming what
  would parse. Spellings are ours (implementation choice).
- `mm2 --cnr ffa|cops|robbers [--cnr-gold ..] [--cnr-limit ..]`: sets
  `SessionMode::CopsAndRobbers`; conflicts with `--event`, `--dev-world`,
  `--join`; the option flags require `--cnr`. With `--host` the config
  is `validate`d and `net::check_session`-gated at flag time (a city
  that cannot seed a round exits 2 instead of advertising).
- `mm2-host` deliberately *not* extended: it runs no simulation, so it
  could advertise a match nobody runs.
- Docs: README, ledger CNR-12, PLAN row.

Tests: `mm2_game` `cnr_options` (3: every name parses and the tables are
fully covered, off-table/malformed rejected, settings defaults + error
text); `net_app::cnr_flag_gates_are_named_exits` (9 usage-error shapes
incl. the site-pool gate on a synthetic city, real `mm2` processes);
`net_app::mm2_host_cnr_advertises_the_mode` (`MM2_RETAIL`-gated: the
`listening=` record names "cops & robbers, "; lobby then killed).
Manual: `mm2 --mm2-path <retail> --city sf --cnr cops --cnr-gold half
--cnr-limit 250pts --headless --frames 120` → "cops & robbers match
built markers=3", smoke status=pass (production `load_session_world`).

Evidence: `cargo fmt --all -- --check` exit 0; `cargo clippy --locked
--workspace --all-targets --all-features -- -D warnings` exit 0 (one
`question_mark` lint fixed on the way); `cargo test --locked --workspace`
exit 0 (2108 passed, 0 failed; was 2103). The two new `net_app` tests
also run with `MM2_RETAIL=/Users/linus/coding/rust-mm2/retail` (the
retail leg ran, not skipped). No GPU, audio or two-process started
match. No test processes left.

Not verified / open: no in-game menu entry or HUD; no two-process
started match; client carrier-mass prediction; rematch; F27-AC01..06
open. Status: implemented candidate, not independently checked.

---

# Last iteration — F27-B.4b (client): markers follow the replica, leavers are re-seated (new-run iteration 17)

Selection: the previous review passed with no blocking findings. Two of
its verification gaps were real defects, so they came first: (1) a
participant who vanished and returned was never re-seated; (2) reading
`load_session_world` showed a *client* also ran `start_match` and kept an
inert `CnrHost`, so its markers (`sync_cnr_markers` read only the host)
would have stayed on the opening draw forever. Both are small and share
the "who owns the match" seam, so one change.

Change:
- `mm2_game::gold::GoldMatch::rejoin` — a left member returns on their
  side with their points (refused: match over, never joined, still
  connected); pushes `Joined` so the revision moves and replicas see it.
  Implementation choice (original reconnect handling unrecovered).
- `cnr::enroll_cnr_participants` re-seats a known-but-disconnected
  participant whose car is present, before seating fresh ones.
- `cnr::start_match` inserts `CnrHost` on the authority only; a client
  builds the same seeded draw just to place markers.
- `cnr::sync_cnr_markers` reads `CnrHost` if present, else `CnrReplica`.
- Docs: ledger CNR-12, `research/net.md`, PLAN row.

Tests (synthetic): `mm2_game` `a_leaver_who_returns_resumes_their_side_and_
points`, `nobody_returns_to_a_finished_match`; `mm2_app::cnr`
`a_participant_whose_car_comes_back_is_reseated_on_their_own_side`,
`a_client_draws_its_markers_from_the_replicated_match` (opening, carried →
hidden, delivery → new sites), `a_client_builds_the_same_draw_for_its_
markers_but_holds_no_match`.

Evidence: `cargo fmt --all -- --check` PASS; `cargo clippy --locked
--workspace --all-targets --all-features -- -D warnings` PASS; `cargo
test --locked --workspace` exit 0 (2103 passed, 0 failed; was 2098).
`MM2_RETAIL=/Users/linus/coding/rust-mm2/retail cargo test --locked -p
mm2_app --test app cops_and_robbers` ran (0.55 s, not skipped) and passed
— that is the only original-data evidence, host side, unchanged. No GPU,
audio or two-process run; the client marker path is synthetic only. No
test processes left.

Not verified / open: client car's carrier mass is not predicted; no HUD,
menu or CLI offers the mode; no two-process run of a started match;
F27-AC01..06 open. Status: implemented candidate, not independently
checked.

---

# Last iteration — F27-B.4b (host): start the match, seat the cars (new-run iteration 16)

Selection: the previous review passed with no blocking findings, so no
repair was owed. B.4b ("sides from the roster, spawn + `CnrHost`,
markers, replica") is still broad; its host half is the ready leg — the
`SessionMode` exists (B.4a) but nothing builds or seats a match from it.
Client markers and the menu/CLI offer stay queued (B.4b-client, B.4c).

Change:
- `cnr::start_match` (called from `load_session_world`): builds the
  `CnrHost` from the city's `CnrContent` pool and `CnrSettings::rules`
  at `RACE_TICK_HZ`, seeded from the session seed, with nobody seated;
  spawns the markers; an `Err` (short pool, non-city path) fails the
  session instead of cruising.
- `cnr::enroll_cnr_participants` (fixed schedule, before the step): seats
  each non-AI car on `GoldMatch::balanced_side()` (fewest connected
  members, ties to the first side; designed — team choice is
  unrecovered), in ascending id order; networked sessions need the car's
  `NetPlayer` stamp.
- `cnr::participant_id`: a car is the match's participant under its
  *wire id* when networked (minted `PlayerId`s differ per process, so the
  replicated match could not name a car otherwise), else its minted id.
  `cnr_host_step` and `reconcile_gold_load` use it.
- Docs: ledger CNR-12, `research/net.md`, PLAN row.

Tests: `mm2_game` `joiners_fill_the_sides_alternately`; `mm2_app::cnr`
(6 new, synthetic: pool→match with same-seed same-round, short/absent
pool and non-city path refuse, alternating seating by wire id with bots
and unstamped cars excluded, local-session seating, no seating off
`Playing`, rules read a car by wire id); `tests/session.rs`
`a_cops_and_robbers_session_builds_seats_and_tears_down_its_match`
(production `load_session_world` path, skips without `MM2_RETAIL`; run
against the retail install at `/Users/linus/coding/rust-mm2/retail`: sf
match built, local car seated as Robbers, hideout/bank/gold markers on
the drawn sites, host and markers gone after quit).

Evidence: `cargo fmt --all -- --check` exit 0; `cargo clippy --locked
--workspace --all-targets --all-features -- -D warnings` exit 0 (one
targeted `too_many_arguments` allow on `start_match`, commented); `cargo
test --locked --workspace` exit 0 (2098 passed, 0 failed; was 2090). The
retail session test also run alone with `MM2_RETAIL` set (ran 0.56 s, not
skipped). No GPU, audio or two-process run. No test processes left.

Not verified / open: no menu/CLI offers the mode and no HUD shows it;
clients do not draw markers from `CnrReplica` and their own car does not
predict the carrier's mass; a car respawned by a pick change is not
re-seated (a leaver is never re-seated); no two-process run of a started
match; F27-AC01..06 open. Status: implemented candidate, not
independently checked.

---

# Last iteration — F27-B.4a: Cops & Robbers as a session mode on the wire (new-run iteration 15)

Selection: the previous review passed with no blocking findings, so no
repair was owed. B.4 (lobby/start, HUD, rematch) is too broad for one
change; its first ready leg is making the match a *configurable,
advertisable session mode* — everything later (sides from the roster,
host wiring, client replica, HUD) needs a `SessionConfig` that names it.

Change:
- `mm2_game::cnr_options` (new): `CnrSettings`, `GoldMass`, `MatchLimit`,
  the option tables and rule constants moved verbatim out of
  `mm2_content::cnr` (which re-exports them) so `mm2_game` can carry the
  lobby's choices without depending on content. Plus
  `MAX_LIMIT_MINUTES`/`MAX_LIMIT_POINTS` bounds.
- `SessionMode::CopsAndRobbers(CnrSettings)`; `SessionConfig::validate`
  requires a city world and a bounded limit (`ConfigError::
  CopsAndRobbersNeedsCity`/`CopsAndRobbersLimit`). Wreck outcome =
  free-roam recovery (designed); no `ResultId` event.
- `mm2_app::net`: the mode rides the advertisement by name (not the
  inferred variant numbering); unnamed choices fail the decode;
  `check_session` refuses a city whose site pool is under 3
  (`SessionContentError::CopsAndRobbers`); `late_join_policy` (shared
  with `mm2-host`) closes the mode to late joins.
- Docs: `docs/research/net.md` section, ledger CNR-12, PLAN row.

Tests (all synthetic): mm2_game config validation + wreck outcome; net
unit — 3x3x9 option grid round trip, city/limit refusals (encode and
hand-written blob), unnamed choices rejected, join policy; `net_check` —
site-pool gate (0, 2, 3, 40 sites).

Evidence: `cargo fmt --all -- --check` PASS; `cargo clippy --locked
--workspace --all-targets --all-features -- -D warnings` PASS; `cargo
test --locked --workspace` PASS (2090 passed, 0 failed; was 2084). No original-data, GPU,
two-process run.

Not verified / open: no menu or CLI offers the mode (deliberately — it
would be a hollow button until B.4b starts a match); nothing creates a
`CnrHost` or renders `CnrReplica`; the fresh-generation-per-match
question for rematch stays B.4b/c; F27-AC01..06 open. Status:
implemented candidate, not independently checked.

---

# Last iteration — F27-B.3: the Cops & Robbers match on the wire (new-run iteration 14)

Selection: the previous review passed with no blocking findings, so no
repair was owed. Of F27-B's queued legs, B.3 (replicate the host's match
to clients) is the next ready one: B.4 (lobby, HUD, rematch) needs a
client-visible match to build on, and F27-AC05 (teams/scores/winner agree
across clients) starts here. The host measures positions itself, so no
client pickup request is needed — the wire is one-way, host → client.

Change:
- `mm2_game::gold`: `GoldMatch::view()` → `GoldView` (everything a
  replica shows) with `freshness()` = `(revision, elapsed)`.
- `mm2_net` protocol v21: `Message::Cnr { generation, frame: SnapCnr }`
  (tag 0x13, bounded to 32 seats; opaque discriminants; strict decode).
  A client sending it is dropped `Malformed` like every host→client
  frame.
- `mm2_app::cnrnet` (new): the discriminant tables, `encode_view` /
  `decode_view` (refuses unnamed discriminants, non-finite positions,
  duplicate/reserved/foreign-side seats, a carrier/dropper/winner that
  is not a participant), `CnrStage` (in `RemoteSnaps`; freshest per
  generation, stale/refused counted, malformed frames cannot become a
  watermark), `publish_cnr` (host; on revision change, else every 120
  match ticks), `apply_cnr` (client → `CnrReplica`, removed with the
  session). Registered in `main.rs`/`smoke.rs` next to the world clock;
  `NetDriveReport` gains `cnr_sent/landed/stale/refused`.
- Docs: `docs/research/net.md` (new section), ledger CNR-12, PLAN row.

Tests (17 new, all synthetic): `mm2_game` view; `mm2_net` codec round
trip (every state), float bits kept, truncation/padding, oversize both
ways, bad bool, and a client cannot assert the match (real sockets);
`cnrnet` unit (every ownership state × variant and every outcome shape
survive the wire, 18 self-contradicting frames refused with the right
reason, stage ordering/generation/bound/reset); `net_app` (host
publishes on change and at the cadence, decoded off a loopback socket
== the host's own view; client lands, drops stale/foreign/malformed,
replica dies with the session). A mutation (`<=`→`<` on the freshness
check) fails three of them.

Gates: `cargo fmt --all -- --check` PASS; `cargo clippy --locked
--workspace --all-targets --all-features -- -D warnings` PASS; `cargo
test --locked --workspace` PASS (2084 passed, 0 failed; was 2067). No
original-data, GPU, two-process or impairment run. No test processes
left running.

Not verified / open: nothing starts a match in the shipped app (B.4) and
nothing renders `CnrReplica` (client HUD/markers — `sync_cnr_markers`
still reads only `CnrHost`); there is no join-time unicast (a late
joiner learns the match from the ≤1 s periodic repeat); a rematch inside
one generation restarts `revision` and would be dropped stale (B.4:
new generation per match or a match epoch); the `net=` record line does
not print the cnr counters yet; F27-AC01..06 all open. Status:
implemented candidate, not independently checked.

---

# Last iteration — F27-B.2b: Cops & Robbers host from content, retail markers (new-run iteration 13)

Selection: the previous review passed (no blocking findings), so no
repair was owed. Of F27-B's queued legs B.2b needs neither protocol nor
lobby design, and it connects the match to real content: the city's
authored site pool and the retail marker models.

Change (`crates/mm2_app/src/cnr.rs`, one `Update` system line in
`main.rs`):
- `CnrHost::from_content(content, settings, tick_hz, generation, gold,
  seed, participants)` — match over `CnrContent::sites` (via `v3`) with
  `CnrSettings::rules`; a pool under 3 is refused (`PoolTooSmall`).
- `spawn_cnr_markers` — `wpobj_gold`/`pt_hideout`/`pt_bank` through
  `MovableModels` (the shared PKG→mesh path), session-owned roots with
  render-part children, no collider; a model that fails to load is
  counted in `CnrMarkerReport`, never substituted.
- `sync_cnr_markers` — hideout/bank on the drawn sites, gold marker on
  the resting/dropped gold, hidden while carried (implementation
  choice, ledger CNR-12 updated). Idle without a `CnrHost`.
- 5 synthetic tests (pool→sites, short pool refused, model-name binding
  with a missing model counted, gold marker follows/hides/returns on a
  drop, hideout/bank positions and no-host idle).

Evidence: gates `cargo fmt --all -- --check` PASS; `cargo clippy --locked
--workspace --all-targets --all-features -- -D warnings` PASS; `cargo
test --locked --workspace` PASS (2067 passed, 0 failed; was 2062). One
local original-data run through a throwaway test (deleted, not
committed): against `/Users/linus/coding/rust-mm2/retail` both cities
build a host from the real pool (sf 44 sites, london 46, 0 content
issues) and all three marker PKGs load and spawn (3/3, no missing
models or textures). No GPU/screenshot: marker scale, ground contact
(model origin vs road) and appearance are unverified.

Still open: nothing calls `from_content`/`spawn_cnr_markers` in the
shipped app (B.4 lobby), no wire (B.3), no HUD/commentary; F27-AC01..06
open. Status: implemented candidate, not independently checked.

---

# Last iteration — repair: a wreck must not farm the gold (new-run iteration 12)

Root cause (review of iteration 11, implementation defect): `cnr_host_step`
turned every connected car in reach into a pickup `Contact`, including a
`Disabled` one. After the drop lockout (120 ticks) the wreck sitting on its
own dropped gold was granted it again (+25 recovery points, Picked event),
then the `wrecked` check dropped it again — a loop of ~1 grant/second with
no opponent. The old test only looked one step after the drop, inside the
lockout.

Fix (`crates/mm2_app/src/cnr.rs`): wrecked players are excluded from the
pickup contacts and from the delivery attempt. Regression test
`a_wreck_never_retakes_the_gold_after_the_lockout_ends` keeps a Disabled car
on the gold for 3x the lockout and asserts no carrier, unchanged score, no
events, base mass. Raised/unchanged review gaps stay open: everything is
synthetic, no real Avian vehicle, teardown ordering and the non-authority
idle path untested, 8 m/s / 25-point recovery are placeholders (ledger
CNR-12), and "rammers trading the gold for 25 points each" is a design
placeholder still to revisit with the cop/robber scoring (F27-A open).

Gates: `cargo fmt --all -- --check` PASS; `cargo clippy --workspace --all-targets -- -D warnings` PASS; `cargo test --workspace` PASS (2062 passed, 0 failed; was 2061). The new test was confirmed to FAIL with the guard disabled and pass with it. No test processes left running.
Status: implemented candidate, not independently checked.

---

# Last iteration — gold rules over the host's cars (new-run iteration 11)

Selection: the previous review passed with no blocking findings, so no
repair was owed. Of F27-B's queued legs, B.2 (Bevy consumers of the new
rule core) is the only one that needs neither a protocol nor a lobby
design, and it is where F27-AC04 (load applied once, no leak into a
later race) becomes a property of real bodies instead of a pure function.

Change: `mm2_app::cnr`. `CnrHost` (resource: the `GoldMatch`, the dislodge
threshold, last-seen positions). `cnr_host_step` (fixed, authority only,
Playing only): ticks the clock; a participant whose car was seen and is
now absent leaves (gold drops at the last position; never-seen or
non-finite-pose participants are not leavers); a `Disabled` carrier, or a
car-on-car `ImpactEvent` of the carrier at ≥ `DEFAULT_DISLODGE_SEVERITY`
(8 m/s, enhanced policy — wall/prop hits never count), drops the gold;
every connected car within the pickup radius becomes a `Contact` from the
*host's* `Position` and `resolve_pickups` decides; the carrier tries
`deliver`; gold below `WorldFloor` goes through `gold_out_of_bounds`;
`GoldEvent`s publish as `CnrEvent` messages. `reconcile_gold_load`
writes the carrier's `Mass` and principal `AngularInertia` from a
recorded `GoldLoadApplied` base (never additive, so repeats cannot
stack) and restores it when `load_for` is none or the `CnrHost`
disappears; `drive_session` teardown removes the `CnrHost`. Both systems
are in the fixed chain next to `recovery`. The handling scalar is
recorded but not applied: its effect is unidentified (UNK-10).

Tests: 15 `mm2_app::cnr` units over a plain App with real
`Session`/`Position`/`Mass`/`AngularInertia` (pickup + load, load once
over 50 steps, load ends exactly with the carrying incl. inertia restore,
soft/world/self impacts ignored, nearest of two wins with one award,
rammed carrier → striker recovers while the dropper is locked out,
`Disabled` carrier drops and cannot retake at once, vanished carrier
leaves and gold drops in place with points kept, unspawned participant is
not a leaver, delivery scores once and sheds the load, below-floor gold
re-placed, pause freezes the clock, time limit ends and publishes once,
removing the host strips the load, NaN pose neither picks up nor ejects).
All synthetic. Ledger CNR-12, PLAN F27-B row updated.

Gates (all exit 0): `cargo fmt --all -- --check` PASS; `cargo clippy
--locked --workspace --all-targets --all-features -- -D warnings` PASS;
`cargo test --locked --workspace` PASS (2061 passed, 0 failed; was 2046).
No original-data, GPU or network run. No test processes left running.

Not verified / open: nothing creates a `CnrHost` in the shipped app (lobby
= B.4), no site markers/meshes from `CnrContent` (B.2b), no wire/client
replica (B.3), no HUD/audio. Whether the mass actually changes handling
feel on a real vehicle in Avian is untested (no physics world in these
tests); the 8 m/s threshold is a guess. F27-AC01..06 all open. Status:
implemented candidate, not independently checked.

---

# Last iteration — authoritative gold state machine (new-run iteration 10)

Selection: the previous review passed with no blocking findings, so no
repair was owed. F27-A's data half is done and its remaining items are
research I cannot resolve without running the original; F27-B (authoritative
gold/teams/scoring/handling) was `queued` behind it and the spec's
requirements 3 and 4 (one valid ownership state, deterministic contested
pickups, load applied/removed exactly once) are pure rules that need no
network or GPU. I split F27-B and took B.1, the rule core, over starting
with wire messages: a protocol written before the rules would encode guesses.

Change (no protocol change, no gameplay wired): `mm2_game::gold::GoldMatch`.
One ownership state `Resting` / `Carried` / `Dropped`; `resolve_pickups`
decides a tick's `Contact`s (player + round from the client, *position from
the host's sim*) — nearest wins, equal distance to the lower `PlayerId`, so
arrival order never matters; losers are `Contested`, repeats `Duplicate`,
old rounds `Stale`, a dropper `Locked` for a short lockout; `deliver` scores
the 100 points once, clears the carrier, draws new seeded distinct sites and
advances the round; `dislodge` (rammed/destroyed), `leave` (disconnect:
gold drops in place, points stay), `join`, `gold_out_of_bounds` (re-placed
at a fresh site, no score), `tick` (time limit); point limit compares the
individual in FFA and the team total otherwise; ties are `Winner::Tie`.
`load_for(player)` *derives* the carrier's mass/handling from the state so
the load cannot be applied twice or outlive the carrying, and a fresh match
has none. `mm2_content::cnr::CnrSettings::rules(tick_hz)` builds `GoldRules`
from the recovered tables (new constants `PICKUP_POINTS` = 25,
`PICKUP_RADIUS_M`, `DROP_LOCKOUT_SECONDS`); `CnrVariant` moved into
`mm2_game::gold` (re-exported from `cnr`, no caller changed).

Classification (docs/research/cnr.md table, ledger CNR-11): delivery 100 /
radius 12 / mass options / limits are original constants; the 25-point
pickup is an original constant applied to every grant (trigger unknown, so
an implementation choice); pickup radius, dropper lockout, disconnect drop,
out-of-bounds re-placement, red→hideout/blue→bank and tie handling are
enhanced policy or implementation choices and say so at their definitions.

Tests: 28 `mm2_game::gold` units (construction validation; three distinct
sites for any seed; nearest-wins; every arrival order of equidistant
requests yields one carrier and one award; duplicates; refusals with
reasons and the inclusive radius; no re-pickup of carried gold; dislodge →
recover with the load moving; lockout expiry; stale/repeated impact reports;
non-finite drop position; delivery scoring once, out-of-range, wrong
marker, stale round, no second score; pickup and delivery in the same tick;
cops→bank/robbers→hideout; red/blue; point limit freezes the match; team
total ends it, not an individual; time limit and ties; leaver keeps points;
late join; out-of-bounds re-placement; load follows only the carrier and a
new match has none; deterministic replay and seed sensitivity; revision and
event drain; balanced sides) and 4 `mm2_content::cnr` units (defaults, every
variant×mass×limit choice maps to the enforced rule, lockout follows the
tick rate, stock settings drive a real match to its end). All synthetic.

Gates (all exit 0): `cargo fmt --all -- --check` PASS; `cargo clippy
--locked --workspace --all-targets --all-features -- -D warnings` PASS (no
new allow); `cargo test --locked --workspace` PASS (2046 passed, 0 failed;
was 2014). No original-data run this iteration (no content path changed).
No test processes left running.

Not verified / open: this is rule-core evidence only — no wire, no
client replica, no HUD/lobby, no vehicle component applying the load, no
impact-threshold that calls `dislodge`, no rematch; F27-AC01..06 all open.
The original's own answers for what knocks gold loose, who may recover and
what recovery scores, disconnect/out-of-bounds outcomes and the handling
scalar's physical effect remain unknown (UNK-10); the policies above are
placeholders that are labelled, not evidence. Status: implemented
candidate, not independently checked.

---

# Last iteration — Cops & Robbers rule matrix, data half (new-run iteration 9)

Selection: the previous review passed with no blocking findings, so no
repair was owed. Eight iterations in a row had gone to F26-A, whose
remaining items are measurement/impairment legs I cannot run here and
polish on already-bounded paths. F27-A (verify the full C&R rule/settings
matrix and placement dependencies) was `queued` with the whole mode
unevidenced — the ledger said "no C&R-specific data discovered" — and is
a research slice that needs no network or GPU, so it is the highest-value
ready work.

What was found (all in `docs/research/cnr.md`, each fact classed
verified / inferred / unknown): the mode's data ships — `race/<city>/
multicopwaypoints.csv` is the gold/hideout/bank site pool (44 sf rows, 46
london), plus five marker models (`wpobj_gold`, `pt_hideout`, `pt_bank`,
`pt_red`, `pt_blue`) with banger records, five map dots, the per-city
`cnr<city>.csv` commentary tables (14 cue families: get/drop/stash/recover
per role, has/stashed/dropped per team) and a loading image. The rest is
code constants read from `Midtown2.exe` (disassembly) and `mmlang.dll`
(string table): the packed host-settings word and its tables (gold mass
0/100/200 engine units, time 5/10/20/30 min, points 100/250/500/1,000);
delivery = 100 points within a 12.0-radius marker with the carrier's mass
removed; pickup arbitration by the host only when no carrier exists; the
local carrier's handling scalar 1.0/0.9/0.81. The executable also tries an
optional `multicopsets.csv` first; no retail install ships one.

Code (no protocol change, no gameplay change): `mm2_content::cnr` —
`CnrContent::load` resolves the 18 per-city dependencies and the 14 cue
families through the VFS and records every miss as an issue (never loses a
denominator entry); typed option tables (`TIME_LIMIT_MINUTES`,
`POINT_LIMITS`, `GoldMass` with engine units kept separate from the
documented kilogram reading, `MatchLimit`, `CnrVariant`, `DELIVERY_POINTS`,
`DELIVERY_RADIUS_M`). `mm2-inspect cnr [--city] [--strict]` audits it
(strict also fails on an empty city list). Ledger: CNR-5 narrowed, CNR-6…
CNR-10 added, MP-9 and UNK-10 updated; the inventory's "no C&R data" note
now points at the audit.

Tests: 14 `mm2_content::cnr` units (complete city, every dependency counted
when absent, missing pool, under-3 pool, non-finite site, header-less pool,
empty install, cue family missing/empty, empty cue table, option tables,
kg ratio, limit choices, variant scoring, paths) and 4 `mm2_inspect::cnr`
units. All synthetic.

Original-data evidence (separate): `cargo run -p mm2_inspect -- cnr
/Users/linus/coding/rust-mm2/retail --strict` — london and sf each 18/18
dependencies, 14/14 cue families, pools of 46 and 44, exit 0. Code facts
were read by disassembling the retail executable (`objdump`); they are
reading evidence, not runtime-observed: no original game was run.

Gates (all exit 0): `cargo fmt --all -- --check` PASS; `cargo clippy
--locked --workspace --all-targets --all-features -- -D warnings` PASS (no
new allow); `cargo test --locked --workspace` PASS (2014 passed, 0 failed;
was 1996). No test processes left running.

Not verified / open: the *inferred* items above (variant 1 numbering, the
radius test direction, the mass unit); what knocks gold loose, pickup
radius, cop/robber scoring beyond delivery, respawn timing, disconnect and
out-of-bounds outcomes; the `multicopsets.csv` path is read from code and
unimplemented. No state machine, HUD or lobby flow exists — F27-B/C stay
queued behind F25-B, and F27-AC01..06 are all open. Status: implemented
candidate, not independently checked.

---

# Last iteration — bound the host's power over a client's re-seeks (new-run iteration 8)

Selection: the previous review passed with no blocking findings, so no
repair was owed. Of its verification gaps the one that is a real
defect-in-waiting rather than missing evidence: "a hostile or buggy host
can force replays of up to 2^21 steps per actor per frame by sending
increasing ticks under the cap; no per-second seek rate limit". A seek
replays every actor from its start, so the cost of one grows with the
target. I took this over per-client traffic relevancy (moot at the
default cap) and over the report-6 DSN-11 reconciliation (a larger
multi-process lifecycle change).

Change (no protocol change, still v20): `worldclock::WorldLimits`, held
by `WorldStage`. A generation's first frame is free (a late joiner must
land wherever the host stands; still bounded by `MAX_SEEK_TICKS`). Each
later frame is judged against the last one *taken*: dropped counted
`throttled` if within 500 ms of it, refused (`ref`) if its tick is more
than `480 + 480 × elapsed s` ahead (4× the 120 Hz fixed rate + 4 s
slack, for a stalled host or a burst after a link blackout). Neither
moves the watermark, so the next honest frame is judged against the same
anchor. `push` stamps wall time; `push_at` takes it explicitly so tests
drive the clock. `RemoteSnaps::set_world_limits` lets a harness relax
the bound; the record's `wclk=` gains `thr<n>`. The numbers are margins
(Implementation choice), not measurements of a real host. Docs:
`docs/research/net.md`, PLAN F26-A slice 8.

Tests: 5 new `WorldStage` units (throttle leaves the watermark on the
taken frame; runaway clock refused with no watermark and the inclusive
edge; a regressing clock stays stale and the seek cap holds unbounded;
a frame-per-millisecond hostile host with a 10× clock gets at most one
frame per interval and shuts out once implausible; an honest 1 Hz host
including a 3 s blackout burst is never limited — only the two frames
bunched behind the first are throttled); the `net_app` world-clock leg
now relaxes the limits for its back-to-back legs and ends with a
throttled frame over a real loopback socket (interval no run can
outlast) and a growth-refused frame followed by an honest one. All use
conditions under bounded spins or injected time, no wall-clock timing
decides an outcome.

Process evidence (separate, retail install `/Users/linus/coding/rust-mm2/
retail`): `MM2_RETAIL=… cargo test --locked -p mm2_app --test network
two_retail_processes_replicate_the_hosts_traffic` passes; host
`wclk=sent14`, client `wclk=sent0,landed5,seek4,ref0,thr9`. The nine
throttles are the **headless fixture**: its host clock runs faster than
wall time (1,680 ticks in a ~8 s test), so frames arrive faster than one
per 500 ms. Nothing was refused (the 4× margin held). It is not an
honest vsync-bound host; that run is unobserved. Not rendered, not
impaired.

Gates (all exit 0): `cargo fmt --all -- --check` PASS; `cargo clippy
--locked --workspace --all-targets --all-features -- -D warnings` PASS;
`cargo test --locked --workspace` PASS (1996 passed, 0 failed; was
1991). No test processes left running.

Still open (F26-A stays active, not AC01..06 completion): no RTT
compensation (client trails by the one-way delay; past ~50 ms it
re-seeks each accepted frame, now at most two a second), proximity
leaves per-peer, no windowed/GPU or impaired-network scenery
measurement, the first frame of a generation can still ask for up to
2^21 steps once, per-client traffic relevancy (moot at the default
cap), signal heads, interpolation beyond the velocity carry, mid-session
weather, late-join of props beyond the resend cycle, sound
replication, report-6 follow-ups 1 (two-process leg) and 2 (DSN-11).
Status: implemented candidate, not independently checked.

---

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
