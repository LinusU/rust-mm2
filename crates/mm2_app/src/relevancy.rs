//! Per-client relevancy for the host's replicated world streams
//! (F26-A.1). Traffic and props are sent to a client only while they are
//! near that client's own vehicle.
//!
//! The interest set has hysteresis: an item *enters* inside
//! [`ENTER_RADIUS`] and only *leaves* beyond [`EXIT_RADIUS`], so a car
//! driving along the boundary does not flap in and out of the stream.
//! The radii and the per-client caps are designed numbers, not recovered
//! from the original (which had no replicated traffic): see
//! `docs/original-rules.md`.

use std::collections::BTreeSet;

use bevy::prelude::*;

/// An item enters a client's interest set inside this distance, metres.
pub const ENTER_RADIUS: f32 = 300.0;
/// ... and leaves it only beyond this one. Wider than the enter radius.
pub const EXIT_RADIUS: f32 = 360.0;

/// What one client's update produced.
#[derive(Debug, Default, PartialEq)]
pub struct Interest<K> {
    /// Items to carry for this client this frame, nearest first.
    pub relevant: Vec<K>,
    /// Those that were not in the set last frame: the caller owes the
    /// client their current state, whatever else it has marked unchanged.
    pub entered: Vec<K>,
}

/// One client's interest set.
#[derive(Debug, Default)]
pub struct InterestSet<K: Ord + Copy> {
    inside: BTreeSet<K>,
}

impl<K: Ord + Copy> InterestSet<K> {
    /// Re-evaluate the set around `centre` over `items` (key, position).
    /// Non-finite positions are never relevant. At most `cap` items are
    /// kept, nearest first, so a crowded neighbourhood bounds the
    /// client's bandwidth instead of the host's whole population.
    pub fn update(
        &mut self,
        centre: Vec3,
        items: impl Iterator<Item = (K, Vec3)>,
        cap: usize,
    ) -> Interest<K> {
        let mut near: Vec<(f32, K)> = items
            .filter(|(_, pos)| pos.is_finite())
            .filter_map(|(key, pos)| {
                let distance = pos.distance(centre);
                let radius = if self.inside.contains(&key) {
                    EXIT_RADIUS
                } else {
                    ENTER_RADIUS
                };
                (distance <= radius).then_some((distance, key))
            })
            .collect();
        near.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        near.truncate(cap);
        let relevant: Vec<K> = near.into_iter().map(|(_, key)| key).collect();
        let entered = relevant
            .iter()
            .copied()
            .filter(|key| !self.inside.contains(key))
            .collect();
        self.inside = relevant.iter().copied().collect();
        Interest { relevant, entered }
    }

    /// Forget everything: the next update re-enters what is near.
    pub fn clear(&mut self) {
        self.inside.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(x: f32) -> Vec3 {
        Vec3::new(x, 0.0, 0.0)
    }

    #[test]
    fn hysteresis_keeps_an_item_between_the_radii() {
        let mut set = InterestSet::<u32>::default();
        let near = set.update(Vec3::ZERO, [(1, at(ENTER_RADIUS - 1.0))].into_iter(), 8);
        assert_eq!(near.entered, vec![1]);
        // Drifted past the enter radius but inside the exit one: kept,
        // and not re-announced as an entry.
        let between = set.update(Vec3::ZERO, [(1, at(ENTER_RADIUS + 20.0))].into_iter(), 8);
        assert_eq!(between.relevant, vec![1]);
        assert!(between.entered.is_empty());
        let gone = set.update(Vec3::ZERO, [(1, at(EXIT_RADIUS + 1.0))].into_iter(), 8);
        assert!(gone.relevant.is_empty());
        // Back between the radii it must re-enter by the enter radius.
        let still_out = set.update(Vec3::ZERO, [(1, at(ENTER_RADIUS + 20.0))].into_iter(), 8);
        assert!(still_out.relevant.is_empty());
    }

    #[test]
    fn the_cap_keeps_the_nearest_and_reports_entries() {
        let mut set = InterestSet::<u32>::default();
        let items = [(1, at(50.0)), (2, at(10.0)), (3, at(30.0))];
        let got = set.update(Vec3::ZERO, items.into_iter(), 2);
        assert_eq!(got.relevant, vec![2, 3]);
        assert_eq!(got.entered, vec![2, 3]);
    }

    #[test]
    fn non_finite_positions_are_never_relevant() {
        let mut set = InterestSet::<u32>::default();
        let items = [(1, Vec3::NAN), (2, at(1.0))];
        assert_eq!(
            set.update(Vec3::ZERO, items.into_iter(), 8).relevant,
            vec![2]
        );
    }
}
