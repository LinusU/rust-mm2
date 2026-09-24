# Last iteration — F13-C.4: complete Professional runtime matrix + review-nit repair

Task slice on `ralph/night` (baseline `11af1ea`, the externally
checked F13-C.3 commit). Selected the named F13-C remainder — the
Professional matrix — now that the sf-8 hypervelocity cascade is
bounded and no longer blocks the legs' cost. No production code
changed; this is a runtime-evidence slice plus one doc repair.

## What ran

Retail install `fnv1a64:e91e6cd4b2ae30d9` (read-only), Apple M1,
code commit `11af1ea`, each leg
`mm2 --mm2-path <install> --city <london|sf> --event checkpoint:<i>
(--bot|--parked) --pro --headless --frames 12000`:

- **48/48 legs `status=pass`, exit 0** — 24 events × scripted +
  parked drivers at Professional (authored `.aimap_p` rosters,
  4–7 opponents/event). Full tables in `docs/race-coverage.md`.
- Scripted local finishes: london-0 `place=4` (3 opponents finished
  ahead of it), london-2 `place=1`, sf-3 `place=1`.
- Parked control: **16 opponent finishes with ledger results across
  5 events while the local participant contributed nothing**
  (london-0 5/6, london-2 2/7, sf-0 4/6, sf-2 1/7, sf-3 1/6);
  `cp=0` on 22/24 legs (sf-0/sf-8 `cp=1` = opponent-shove trigger
  crossings, same as amateur); `moved` ≤29 m, `peak` ≤10.3 m/s.
- Restart contamination at Pro matches the amateur pattern: scripted
  wreck-loops london-1 rs5 / london-3 rs8 / london-6 rs5 / sf-10 rs6
  (still `Countdown` at cap). Parked legs on the same events show the
  field progressing regardless (omax 2–7).
- Physics health under the F13-C.3 caps: `dropped` ≤345 on the 5
  worst legs (amateur's 16k–56k drops do not recur); peak ≤34.2 m/s;
  sf-8 Pro scripted ran `cp=6/8`, 80 s wall.
- Divergence from C.2's three Pro legs (run pre-caps at `a7d2797`)
  is disclosed in the doc — sf-0 scripted `place=6/7` → `cp=2/6`,
  london-0 scripted at-cap `rs=2` → `place=4`; those rows are marked
  superseded, not deleted.

## Anomaly kept visible

london-4 parked Pro: `rcv=209f/209r` — the stationary car's elevated
spawn (y≈4.9) sits next to a drop and field contact keeps punting it
over the edge; the recovery anchor catches all 209 falls
(`status=pass`, `wheels=4/4`, `moved=5m`). Machinery holds; the
frequency is the control's own artifact (a parked car cannot dodge).

## Doc repair

The F13-C.3 review's non-blocking nit: PLAN.md said `Tests +4` where
the diff added 3 (21→24). Corrected to `+3 (21→24)`.

## Gates

Docs-only change — no code edited, so the binary is unchanged from
the externally checked `11af1ea`. Gates re-run anyway on the
candidate tree: `cargo fmt --all -- --check` clean,
`cargo clippy --locked --workspace --all-targets --all-features --
-D warnings` clean, `cargo test --locked --workspace` green (all
suites). The 48 retail legs above exercised the `11af1ea` code path
end to end.

## Not done / open

- Pro `hold`-driver legs (24 more) — the scripted+parked matrix is
  complete; hold adds a blind-driving third driver at Pro.
- Deep London/SF non-finishes remain budget- and controller-skill-
  bound; original-fidelity comparisons (retail difficulty, pacing,
  AI competence) stay unverified — engine self-metrics only.
- F13-C stays `active`; remaining rows per the task table.
