//! Reward-table production: normalize a catalog's parsed
//! `&lt;city&gt;_rewards.csv` rows into `mm2_game`'s session-scoped
//! [`RewardTable`] (F16-B).
//!
//! `catalog_events` attaches indexed rows (`crash,3`) to their event
//! and keeps `half`/`all` milestones at catalog level; [`reward_table`]
//! is the second pass that resolves every row into a rule with its
//! target — indexed rows as `(EventKey, rule)` pairs, milestones with
//! the family's authored event count as denominator. Rows that cannot
//! become a rule (unrecognized `RaceType`, unusable `RaceNum`, an
//! index no authored event owns) are kept as diagnostics — coverage
//! audits (F16-AC05) need every authored row accounted for, not
//! silently dropped.

use mm2_formats::rewards::{RewardNum, RewardRow};
use mm2_game::{EventKey, EventTableKind, RewardRequirement, RewardRule, RewardTable, Unlock};

use crate::events::EventCatalog;

/// Build the reward table for `catalog`'s city. The catalog already
/// carries the city name and per-row event identity — every rule
/// measures records under [`EventKey`]s built from the resolved event.
pub fn reward_table(catalog: &EventCatalog) -> RewardTable {
    let city = &catalog.city;
    let mut table = RewardTable::default();
    // Denominators are the catalog's authored table rows — discovered
    // extras land in `catalog.extras`, not here, so they never inflate
    // what `half`/`all` measure.
    for ev in &catalog.events {
        *table.family_sizes.entry(ev.event_ref.table).or_insert(0) += 1;
    }
    for ev in &catalog.events {
        for row in &ev.rewards {
            match rule(city, row) {
                Ok(rule) => table.per_event.push((
                    EventKey {
                        city: ev.event_ref.city.clone(),
                        table: ev.event_ref.table,
                        stem: ev.stem.clone(),
                    },
                    rule,
                )),
                Err(d) => table.diagnostics.push(d),
            }
        }
    }
    for row in &catalog.milestone_rewards {
        match rule(city, row) {
            Ok(rule) => match rule.requirement {
                RewardRequirement::Event(i) => table.diagnostics.push(format!(
                    "{city}_rewards.csv:{}: indexed row `{i}` did not attach to an event",
                    row.line
                )),
                _ => table.milestones.push(rule),
            },
            Err(d) => table.diagnostics.push(d),
        }
    }
    table
}

fn rule(city: &str, row: &RewardRow) -> Result<RewardRule, String> {
    let source = format!("{city}_rewards.csv:{}", row.line);
    let Some(family) = EventTableKind::from_reward_token(&row.race_type) else {
        return Err(format!(
            "{source}: unrecognized race type `{}`",
            row.race_type
        ));
    };
    let requirement = match &row.race_num {
        RewardNum::Half => RewardRequirement::Half,
        RewardNum::All => RewardRequirement::All,
        RewardNum::Index(i) => RewardRequirement::Event(*i),
        RewardNum::Other(s) => {
            return Err(format!("{source}: unusable race num `{s}`"));
        }
    };
    let unlock = if row.variant == 0 {
        Unlock::Vehicle(row.car.clone())
    } else {
        Unlock::Paint {
            car: row.car.clone(),
            variant: row.variant,
        }
    };
    Ok(RewardRule {
        family,
        requirement,
        unlock,
        message: row.message.clone(),
        line: row.line,
    })
}
