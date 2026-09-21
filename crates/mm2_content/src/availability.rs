//! Availability-table production (F16-B): map a catalog's authored
//! event rows into `mm2_game`'s session-scoped [`AvailabilityTable`] —
//! which events a progressing profile may select.
//!
//! The gate structure is authored data plus documented rules:
//!
//! - Blitz and Circuit rows are always open — no gating rule exists
//!   for either family.
//! - Checkpoint rows gate in authored-order sets of three: the first
//!   set is open (CHK-2, verified), each later set needs every event
//!   in the previous set beaten (CHK-3). A trailing partial set still
//!   gates on the set before it.
//! - Crash Course rows read their authored `Description` tag
//!   (`lesson<N>`/`midtrm<N>`/`final…`, CC-2): lessons are always
//!   open; `midtrm<N>` gates on that group's lessons (`lesson{3N-2}`
//!   …`lesson{3N}` — the tag arithmetic matches the authored order);
//!   the `final` gates on every midterm (CC-3). A row whose tag is
//!   unreadable, a midterm whose lesson group is absent, or a final
//!   with no midterms stays *open* and lands in `diagnostics` — a
//!   malformed table fails open and visible, not silently locked.
//!
//! What "beaten" means and how gates evaluate lives in
//! `mm2_game::progression` — this file only names the prerequisite
//! events.

use mm2_game::{AvailabilityRow, AvailabilityTable, EventGate, EventKey, EventTableKind};

use crate::events::{CatalogEvent, EventCatalog};

/// The authored checkpoint unlock group size (CHK-2/CHK-3).
const CHECKPOINT_SET: usize = 3;

/// Build the availability table for `catalog`'s city. Like
/// [`crate::reward_table`], this never fails: unreadable structure
/// lands in `diagnostics`, keyed by the event's stable identity.
pub fn availability_table(catalog: &EventCatalog) -> AvailabilityTable {
    let city = &catalog.city;
    let key_of = |ev: &CatalogEvent| EventKey {
        city: ev.event_ref.city.clone(),
        table: ev.event_ref.table,
        stem: ev.stem.clone(),
    };

    // Crash-course gates reference other rows, so classify the whole
    // family before assigning — a midterm names its lesson group by
    // tag number, the final names every midterm.
    let mut lessons: Vec<(u32, EventKey)> = Vec::new();
    let mut midterms: Vec<EventKey> = Vec::new();
    let mut crash_rows: Vec<(&CatalogEvent, CrashTag)> = Vec::new();
    for ev in catalog
        .events
        .iter()
        .filter(|e| e.event_ref.table == EventTableKind::CrashCourse)
    {
        let tag = CrashTag::parse(&ev.description);
        match &tag {
            CrashTag::Lesson(n) => lessons.push((*n, key_of(ev))),
            CrashTag::Midterm(_) => midterms.push(key_of(ev)),
            _ => {}
        }
        crash_rows.push((ev, tag));
    }

    let mut table = AvailabilityTable::default();
    // Authored-order position inside the checkpoint family — the set a
    // row belongs to and the set gating it. `crash_rows` holds every
    // CrashCourse event in the same order, so the two stay in step.
    let mut checkpoint: Vec<EventKey> = Vec::new();
    let mut crash_iter = crash_rows.iter();
    for ev in &catalog.events {
        let key = key_of(ev);
        let gate = match ev.event_ref.table {
            EventTableKind::Blitz | EventTableKind::Circuit => EventGate::Open,
            EventTableKind::Checkpoint => {
                let set = checkpoint.len() / CHECKPOINT_SET;
                let gate = if set == 0 {
                    EventGate::Open
                } else {
                    EventGate::AfterAll(
                        checkpoint[(set - 1) * CHECKPOINT_SET..set * CHECKPOINT_SET].to_vec(),
                    )
                };
                checkpoint.push(key.clone());
                gate
            }
            EventTableKind::CrashCourse => {
                let (_, tag) = crash_iter.next().expect("every crash row was classified");
                crash_gate(city, ev, tag, &lessons, &midterms, &mut table.diagnostics)
            }
        };
        table.rows.push(AvailabilityRow { key, gate });
    }
    table
}

/// The authored `Description` tag on a `mmcrashdata.csv` row
/// (CC-2 — `lesson1`…`lesson9`, `midtrm1`…`midtrm3`, `final13`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CrashTag {
    /// `lesson<N>` — always selectable.
    Lesson(u32),
    /// `midtrm<N>` — gates on lesson group N.
    Midterm(u32),
    /// `final…` — gates on every midterm.
    Final,
    /// No recognizable tag.
    Unknown,
}

impl CrashTag {
    fn parse(description: &str) -> Self {
        let d = description.trim().to_ascii_lowercase();
        if let Some(n) = d.strip_prefix("lesson").and_then(|s| s.parse().ok()) {
            Self::Lesson(n)
        } else if let Some(n) = d.strip_prefix("midtrm").and_then(|s| s.parse().ok()) {
            Self::Midterm(n)
        } else if d.starts_with("final") {
            Self::Final
        } else {
            Self::Unknown
        }
    }
}

fn crash_gate(
    city: &str,
    ev: &CatalogEvent,
    tag: &CrashTag,
    lessons: &[(u32, EventKey)],
    midterms: &[EventKey],
    diagnostics: &mut Vec<String>,
) -> EventGate {
    match tag {
        CrashTag::Lesson(_) => EventGate::Open,
        CrashTag::Midterm(n) => {
            // Group N's lessons are `lesson{3N-2}`…`lesson{3N}` — the
            // tag numbering matches the authored order (CC-2). The tag
            // number is authored data: a modded/corrupt table can name
            // any u32, and `3N` can exceed the lesson numbers' range
            // without ever matching a row — compute in u64 so that
            // case falls through to the no-lessons diagnostic instead
            // of panicking (debug) or wrapping onto a real group
            // (release).
            let first = 3 * u64::from(n.saturating_sub(1)) + 1;
            let reqs: Vec<EventKey> = lessons
                .iter()
                .filter(|(l, _)| (first..first + 3).contains(&u64::from(*l)))
                .map(|(_, k)| k.clone())
                .collect();
            if reqs.is_empty() {
                diagnostics.push(format!(
                    "{city}: crash row `{}` (`midtrm{n}`) found no lesson{first}–{} rows — always available",
                    ev.stem,
                    first + 2
                ));
                EventGate::Open
            } else {
                EventGate::AfterAll(reqs)
            }
        }
        CrashTag::Final => {
            if midterms.is_empty() {
                diagnostics.push(format!(
                    "{city}: crash row `{}` (`final…`) found no midterm rows — always available",
                    ev.stem
                ));
                EventGate::Open
            } else {
                EventGate::AfterAll(midterms.to_vec())
            }
        }
        CrashTag::Unknown => {
            diagnostics.push(format!(
                "{city}: crash row `{}` has no lesson/midterm/final tag ({:?}) — always available",
                ev.stem, ev.description
            ));
            EventGate::Open
        }
    }
}
