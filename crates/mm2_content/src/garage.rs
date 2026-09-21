//! Garage selectability production: fold the vehicle catalog and every
//! city's authored reward table into `mm2_game`'s [`GarageTable`]
//! (F16-B.3, VEH-3/VEH-4).
//!
//! A vehicle is reward-gated when some authored `<city>_rewards.csv`
//! row grants `vehicle:<id>` (`VariantNum` 0); a paint index is gated
//! when a row grants `paint:<id>:<variant>` — the variant measured as
//! the zero-based `Colors`/paint-job index (vpvwcup variant 5 = "Team
//! Angel", the documented Angel Cup paint; 6 = "Team MS", the
//! Microsoft Cup). Every city's table feeds the union — a car won in
//! London must open in San Francisco's garage too.
//!
//! Roster membership is the catalog's `canonical_info` flag: the
//! original's select list is the `tune/*.info` scan, so fallback-only
//! entries (vpmoonrover's `.inf` — UNK-3) and metadata-less leftovers
//! are unlisted but still evaluated (designed reading). Rows whose
//! grant cannot become a gate — an unknown vehicle, a paint index the
//! metadata does not declare — land in `diagnostics`, never silently
//! dropped (F16-AC05's accounting rule).

use std::collections::{BTreeMap, BTreeSet};

use mm2_assets::Vfs;
use mm2_game::{GarageRow, GarageTable, PaintGate, RewardTable, Unlock, VehicleGate};

use crate::catalog::VehicleCatalog;
use crate::events::{EventCatalog, race_cities};
use crate::rewards::reward_table;

/// Compose the garage surface for a mounted VFS in one call: vehicle
/// catalog plus every discovered race city's reward table. Reward-table
/// diagnostics fold into [`GarageTable::diagnostics`] — on this path a
/// reward row that could not become a rule is a gate-production finding.
pub fn scan_garage(vfs: &Vfs) -> GarageTable {
    let catalog = VehicleCatalog::scan(vfs);
    let mut rewards = Vec::new();
    let mut diagnostics = Vec::new();
    for city in race_cities(vfs) {
        let events = EventCatalog::scan(vfs, &city);
        let table = reward_table(&events);
        diagnostics.extend(table.diagnostics.iter().map(|d| format!("{city}: {d}")));
        rewards.push(table);
    }
    let mut table = garage_table(&catalog, &rewards.iter().collect::<Vec<_>>());
    table.diagnostics.extend(diagnostics);
    table
}

/// Build the garage table: one [`GarageRow`] per catalog entry, in
/// catalog order, with gates from the union of `rewards`' grants.
pub fn garage_table(catalog: &VehicleCatalog, rewards: &[&RewardTable]) -> GarageTable {
    // Every authored grant, keyed by target vehicle id.
    let mut gated_vehicles: BTreeSet<&str> = BTreeSet::new();
    let mut gated_paints: BTreeMap<&str, BTreeSet<i64>> = BTreeMap::new();
    for table in rewards {
        let rules = table
            .milestones
            .iter()
            .chain(table.per_event.iter().map(|(_, rule)| rule));
        for rule in rules {
            match &rule.unlock {
                Unlock::Vehicle(id) => {
                    gated_vehicles.insert(id.as_str());
                }
                Unlock::Paint { car, variant } => {
                    gated_paints
                        .entry(car.as_str())
                        .or_default()
                        .insert(*variant);
                }
            }
        }
    }

    let mut table = GarageTable::default();
    for entry in &catalog.entries {
        let mut paint_gates = vec![PaintGate::Open; entry.paints.len()];
        if let Some(variants) = gated_paints.get(entry.id.as_str()) {
            for &variant in variants {
                match usize::try_from(variant)
                    .ok()
                    .filter(|&i| i < paint_gates.len())
                {
                    Some(i) => paint_gates[i] = PaintGate::Reward,
                    None => table.diagnostics.push(format!(
                        "{}: rewards grant paint variant {variant} but the catalog declares {} paint(s)",
                        entry.id,
                        entry.paints.len()
                    )),
                }
            }
        }
        table.rows.push(GarageRow {
            id: entry.id.clone(),
            listed: entry.canonical_info,
            gate: if gated_vehicles.contains(entry.id.as_str()) {
                VehicleGate::Reward
            } else {
                VehicleGate::Open
            },
            paint_gates,
            unlock_score: entry.unlock_score,
            unlock_flags: entry.unlock_flags,
        });
    }
    // Grants aimed at ids the catalog never discovered still surface —
    // a reward for a missing vehicle is an audit finding, not silence.
    for id in gated_vehicles {
        if !table.rows.iter().any(|row| row.id == id) {
            table.diagnostics.push(format!(
                "rewards grant vehicle {id} but no catalog entry matches"
            ));
        }
    }
    for (car, _) in gated_paints {
        if !table.rows.iter().any(|row| row.id == car) {
            table.diagnostics.push(format!(
                "rewards grant paint on {car} but no catalog entry matches"
            ));
        }
    }
    table
}
