# Last iteration — parked cars across peers in a networked race (iteration 13 of the new run)

Selection: iteration 12 (`7eaee43`) passed gates and review with no blocking findings. Its next-step list offered F27-C (needs a second driven client) or report 6 follow-up 3. I took the checkable half of follow-up 3: "whether the seed-rolled parked cars agree across peers in networked races". The existing two-process prop-world leg was a Cruise, where the session skips parked cars, so the question had no evidence.

Change (candidate, not independently checked):
- `smoke.rs`: the networked `props=` record cell gains `parked<n>` (cars this process placed, from `ParkedCarReport`). Records without a stamped networked world are unchanged.
- `net_drive`: new `two_retail_processes_roll_the_same_parked_cars_in_a_race` (`MM2_RETAIL`-gated): host `--event checkpoint:0`, parked client; both must place the same non-zero parked count, share one `SiteTable` digest, and the client must refuse no row (`mism0`). The cruise leg now also asserts `parked0` on both sides (the original skips them in networked cruise).
- `docs/research/net.md` and the PLAN F26-A row / report-6 summary record it.

Evidence (retail london `checkpoint:0`, loopback, headless; `MM2_RETAIL=/Users/linus/coding/rust-mm2/retail`): 3/3 passes (~7.5 s each); host and client both `props=sites6632:6c14ab6b820cd46b,…,parked481`; client landed 486–495 prop rows, `mism0`. The four other `two_retail_processes_*` legs also passed in the same run (82 s together).

Gates (foreground): `cargo fmt --all -- --check` pass; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` exit 0; `cargo test --locked --workspace` exit 0, 58 `test result: ok` lines, none failed (without `MM2_RETAIL`; the retail legs ran separately above).

Not verified: the paint roll is not in the digest (same seed stream, so a paint-only divergence would pass); one event and city; same machine and binary, so cross-platform rolls are unobserved; nothing rendered. Drawbridge, mover (sailboat/ferry/train) and sound actors of follow-up 3 are still clock-only / local; follow-up 3 is not closed.

Status: implemented candidate; not independently checked. Next: F27-C multi-client contested pickup (needs a second driven client), or the remaining follow-up-3 actors (proximity leaves, sounds).
