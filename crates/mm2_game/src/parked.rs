//! Parked cars — the retail parked-car manager's placement rule.
//!
//! The original creates a parked-car manager per city beside its
//! bridges and ferries; it reads `race/<city>/<city>_parkedcar[_<event
//! stem>].pathset` (the same lookup as every object manager,
//! [`crate::object_pathset_candidates`]) and fills each path's strip
//! with kerbside cars. Recovered from the retail executable — see
//! `docs/research/parked.md`:
//!
//! - the strip spacing is the path's own, floored at
//!   [`MIN_SPACING`];
//! - at each stamp a C `rand() % 3` picks nothing (0), `giz_pcar01_l`
//!   (1) or `giz_pcar02_l` (2) — `giz_pcar03_l` ships but is never
//!   chosen;
//! - a placed car takes a second `rand()` as its paint variant and is
//!   turned 90° about Y from the stamp frame, which lays its local +Z
//!   along the path.
//!
//! The original's `rand()` stream is shared with the rest of the game,
//! so its exact picks are unreproducible; [`ParkedRng`] runs the same
//! generator from the session seed so every peer stamps the same cars.

/// The smallest strip spacing the manager uses, metres.
pub const MIN_SPACING: f32 = 5.0;

/// The strip spacing for a parked-car path whose authored spacing is
/// `authored` metres.
pub fn parked_spacing(authored: f32) -> f32 {
    if authored < MIN_SPACING {
        MIN_SPACING
    } else {
        authored
    }
}

/// The model a `rand()` roll places, `None` for an empty bay.
pub fn parked_model(roll: u32) -> Option<&'static str> {
    match roll % 3 {
        1 => Some("giz_pcar01_l"),
        2 => Some("giz_pcar02_l"),
        _ => None,
    }
}

/// The MSVC C runtime `rand()` the original rolls: a 32-bit LCG
/// returning bits 16–30 (`0..=32767`).
#[derive(Debug, Clone)]
pub struct ParkedRng(u32);

impl ParkedRng {
    /// A generator seeded from the session seed — designed: the
    /// original's stream position is not reproducible.
    pub fn new(seed: u64) -> Self {
        Self((seed ^ (seed >> 32)) as u32)
    }

    /// The next `rand()` value.
    pub fn next_roll(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(214_013).wrapping_add(2_531_011);
        (self.0 >> 16) & 0x7fff
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spacing_is_floored_at_five_metres() {
        assert_eq!(parked_spacing(0.0), 5.0);
        assert_eq!(parked_spacing(4.99), 5.0);
        assert_eq!(parked_spacing(8.0), 8.0);
    }

    #[test]
    fn rolls_pick_an_empty_bay_or_one_of_two_models() {
        assert_eq!(parked_model(0), None);
        assert_eq!(parked_model(1), Some("giz_pcar01_l"));
        assert_eq!(parked_model(5), Some("giz_pcar02_l"));
        assert_eq!(parked_model(32_766), None);
    }

    #[test]
    fn rng_matches_the_msvc_sequence() {
        // MSVC `srand(1); rand()` → 41, 18467, 6334, 26500.
        let mut rng = ParkedRng(1);
        let rolls: Vec<u32> = (0..4).map(|_| rng.next_roll()).collect();
        assert_eq!(rolls, vec![41, 18_467, 6_334, 26_500]);
    }

    #[test]
    fn about_a_third_of_the_bays_stay_empty() {
        let mut rng = ParkedRng::new(7);
        let empty = (0..3000)
            .filter(|_| parked_model(rng.next_roll()).is_none())
            .count();
        assert!((800..1200).contains(&empty), "{empty}");
    }

    /// Networked races keep the kerbside cars, so two peers must roll the
    /// same bays. The stream is a pure function of the session seed (which
    /// the advertised config carries to every client), consumed in stamp
    /// order only by the spawn loop.
    #[test]
    fn peers_sharing_a_session_seed_roll_identical_bays() {
        let bays = |seed: u64| {
            let mut rng = ParkedRng::new(seed);
            (0..64)
                .map(|_| (parked_model(rng.next_roll()), rng.next_roll()))
                .collect::<Vec<_>>()
        };
        assert_eq!(bays(0xfeed_beef), bays(0xfeed_beef));
        assert_ne!(bays(0xfeed_beef), bays(42));
    }
}
