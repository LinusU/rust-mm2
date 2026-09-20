# Last implementation iteration

- Task ID and title: F04-C.1 — original-content banger strike
  evidence: developer tooling to place the vehicle at authored prop
  positions, transition-level counters in the headless record, a
  session-level AC05 restart test that exercises a real break, and
  the first retail strikes of authored props — one activation/settle
  and one BREAK-fragment break on original data.
- Starting commit and resulting commits: started at
  `d552e28c0a169c30b6c758b6fd97617529a91bf2` (clean tree, branch
  `ralph/night`; F04-B.1 had just passed external gates + review,
  verdict pass, no blocking findings). Result = one feature commit
  plus this handoff note.
- Why this slice: the F04-B.1 review's primary verification gap was
  that no real retail prop had ever been physically struck — all
  break evidence was synthetic + parse/count audits. This iteration
  closes the "does the runtime path actually fire on original
  content" question while keeping ACs honestly open.
- What changed:
  - `crates/mm2_game/src/config.rs` — `SpawnPose` (position + yaw
    radians, `Quat::from_rotation_y` convention: yaw 0 drives −Z) and
    `DevOverrides::spawn`, quarantined with `--cam`/`--nav`: never a
    session-legal or progression parameter.
  - `crates/mm2_app/src/main.rs` — `--spawn x,y,z[,yaw-deg]`
    diagnostic flag; invalid input exits 2.
  - `crates/mm2_app/src/session.rs` — `load_session_world` applies
    the dev pose after all world/event spawn sources have written,
    so it wins over roam spawn and authored event slots alike.
  - `crates/mm2_app/src/smoke.rs` — the headless record gains
    `bng_ev=<a>a/<s>s/<b>b`: emitted `BangerStateChanged`
    transitions counted per phase (drained per update). `bng=` stays
    the end-state bucket counts — a prop that activated then settled
    shows in both event buckets, fragments show as `3a` in `bng=`
    with no `a` event (they spawn `Active` silently by design).
  - `crates/mm2_app/tests/session.rs` —
    `dev_spawn_override_pins_the_player_pose`: override pose wins
    and `SpawnPoint` records it for resets.
  - `crates/mm2_app/tests/banger.rs` —
    `restart_restores_stamped_placements_after_a_break`: a synthetic
    install (`city/test.psdl` + `props.pathset` stamping an authored
    two-piece `breakpkg`), loaded through the real
    `load_session_world` path; a striker breaks it (husk `Broken` +
    2 fragment bodies); then `SessionControl.restart` through
    `drive_session` restamps the placement dormant under
    `SessionEntity(2)` with its collider and `BangerPieces`
    restored, and zero generation-1 entities survive.
- Retail evidence actually captured (this machine, macOS arm64,
  `target/debug/mm2 --mm2-path <retail>`):
  - London roam activation + settle: `--city london --car vpbug
    --spawn 762,0.5,-424,0 --headless --frames 400` →
    `status=pass ... impacts=2 bng=1187d/0a/1s/0b
    bng_ev=1a/1s/0b`. One authored `sp_bollard_black_l` (the stamped
    placement near 757,−425 on road494) activated on a real impact
    and later settled — the first observed original-content
    transition.
  - SF roam BREAK: `--city sf --car vpsemi
    --spawn=-169,35.5,744,172 --headless --frames 600` →
    `status=pass ... impacts=14 bng=924d/3a/0s/1b
    bng_ev=0a/0s/1b`. One authored `sp_wrongwayfw` freeway sign
    (stamped at −178.9,34.9,784.8 beside road4) went `Broken` and
    spawned 3 `Active` fragments — matching its authored
    `NumParts 3` / BREAK01–03 chunks exactly. Fragments emit no
    spawn event by design, so `bng_ev` shows only the single logical
    break. Reproduced identically at 1500 frames.
  - SF below-threshold contact (AC01's other side): `--spawn=-180,
    35.5,776,187` grazed the same sign at ~3 m/s → `impacts=1`,
    `bng=925d/0a/0s/0b bng_ev=0a/0s/0b` — contact registered, no
    transition. Same authored prop: graze → nothing, ~10 m/s strike
    → break.
  - London event overlay: `--event circuit:7` stamps 899 overlay
    bangers (2087 dormant total; `sp_sawhrslt_f` wall,
    `sp_stackboxs_f` piles, `sp_jumptrailer_f`, `sp_barrelgray_f`
    placed by `race/london/race7.pathset`). The sawhorse pile
    (ImpulseLimit2 34982) was reachable only at ~8 m/s semi —
    below threshold, no transition.
- What this does and does not prove:
  - Proves: the F04 runtime fires on real authored content — a
    knockable activated and settled; a breakable produced one
    logical break event and exactly its authored fragment count as
    dynamic bodies; a below-threshold contact produced nothing.
  - Does not prove: original break timing/threshold semantics
    (UNK-22 — the break still fires on the provisional activation
    edge; whether the original gates breaks separately is open);
    visual correctness of fragments (no GPU capture — the `hold`
    driver is headless-only); original-game fragment behaviour;
    fragment settle on retail data (see observation below).
- Observation worth tracking: in the 1500-frame SF rerun the 3
  sign fragments stayed `Active` (never reached Avian sleep) while
  resting on the sloped elevated freeway — plausible slope
  tumbling/sliding rather than a settle-path defect (the London
  bollard settled normally on flat ground). Not proven a bug; a
  flat-ground retail break would settle it definitively.
- Commands actually run and results:
  - `cargo fmt --all -- --check` — PASS.
  - `cargo clippy --locked --workspace --all-targets --all-features
    -- -D warnings` — PASS.
  - `cargo test --locked --workspace` — PASS, all 32 suites,
    0 failures (incl. the 2 new tests).
  - `mm2-inspect` audits used for targeting: `banger` records
    (thresholds/`NumParts`), `banger-bind`, `pathset`, `events`,
    `pkg`; BAI road centre-lines parsed from `city/{sf,london}.bai`
    via `mm2-inspect dump` to place the car on real streets.
- Acceptance IDs satisfied / still open:
  - F04-AC01: partially evidenced on original content — below- and
    above-threshold contacts on the same authored prop produce
    different outcomes (nothing vs break). Still open pending
    review.
  - F04-AC02: unchanged (synthetic dedup coverage).
  - F04-AC03: real authored BREAK chunks spawned as fragments on
    retail data; original timing/effects unproven — stays open.
  - F04-AC04: unchanged.
  - F04-AC05: now covered at session level — restart after a real
    break removes husk + fragments and restamps the placement
    dormant (synthetic city through the production session path).
    Original-content restart unexercised.
  - F04-AC06: unchanged groundwork; fragment ObjectIds mint
    deterministically in authored order.
- Deferred deliberately: `BirthRule` particles, `AudioId`/`Flash`/
  `TexNumber` effects, decals, prop-rule stamping (UNK-21), `Timer`
  despawn, replication (F26), GPU capture of a break, fragment
  settle on retail data.
- Scope decisions recorded: `--spawn` is a developer diagnostic like
  `--cam` — parsed into `DevOverrides`, applied after spawn-source
  selection, never read by session/progression/network rules.
  `bng_ev` counts the semantic event stream, not entity buckets, so
  fragment spawns (silent by contract) do not inflate activation
  counts.
- Stock data/GPU/audio/network limitations: all retail evidence is
  headless physics through the VFS on the real install (read-only).
  No GPU, audio, or network capability was exercised; those remain
  evidence gaps, not passes.
- Unresolved blockers or discovered regressions: none known.
- Next smallest useful action: a flat-ground retail break for
  fragment-settle evidence (reachable candidates: `sp_sawhrslt_f`
  walls in race overlays, park-lamp rows — needs a scripted/route
  driver or more speed than `hold` reaches); F04-C.2 repeated-
  collision/pool-reclaim retail runs; prop-rule stamping (UNK-21);
  F13-A; F09-C.

This is a candidate handoff. External code-gate and separate review
results live in the runner state directory and are not implied by
this report.
