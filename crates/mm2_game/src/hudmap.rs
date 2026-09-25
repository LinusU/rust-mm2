//! The HUD map's session state machine (F22-A.1).
//!
//! `mm2_app::hudmap` owns the rendering; this module owns the *state* —
//! which corner view is up, whether the map rotates with the player, the
//! eased zoom level and the full-screen pause map — because it is the
//! session's contract (HUD-4): TAB cycles two smaller map views and off,
//! E toggles the zoom level, F toggles rotation, Q opens the full-screen
//! map (single player only). Everything is driven by the authored
//! [`HudMapSpec`] the session loaded (`tune/<city>.mmhudmap`), so the
//! distances, icon scales and easing rate are authored values, not
//! invented ones.
//!
//! Split evidence on what is and is not recovered:
//!
//! - **Documented (HUD-4):** the four controls and their effects, the
//!   player/opponent/checkpoint/finish marker vocabulary.
//! - **Inferred:** `ZoomInDist`/`ZoomOutDist`/`IconScale*` read as world
//!   metres (the magnitudes sit at city scale); `Pos`/`Size` as
//!   top-left-origin window fractions (they land the map in the
//!   bottom-right corner, where the original draws it).
//! - **Designed:** the *order* TAB cycles in (Inset → Large → Off), the
//!   second inset's exact size (the original's two "smaller views"
//!   geometry is unrecovered), and `ZoomIn == 0` meaning "start zoomed
//!   out" (the field's semantics are unrecovered).

use bevy::prelude::*;
use mm2_formats::hudmap::HudMapSpec;

/// Which corner-map presentation the HUD shows — TAB cycles it
/// (HUD-4: "two smaller map views and off"). The cycle order is
/// designed; the original's ordering is unrecovered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MapView {
    /// The authored corner inset (`Pos`/`Size` window fractions).
    #[default]
    Inset,
    /// The second smaller view — a larger corner inset (designed size;
    /// HUD-4 only says two views exist).
    Large,
    /// Map hidden.
    Off,
}

impl MapView {
    /// TAB order: `Inset → Large → Off → Inset`.
    pub fn next(self) -> Self {
        match self {
            MapView::Inset => MapView::Large,
            MapView::Large => MapView::Off,
            MapView::Off => MapView::Inset,
        }
    }

    /// Stable lowercase name for the `map=` record field.
    pub fn name(self) -> &'static str {
        match self {
            MapView::Inset => "inset",
            MapView::Large => "large",
            MapView::Off => "off",
        }
    }
}

/// Whether the map rotates to keep the player's heading up — F toggles
/// (HUD-4). `NorthUp` is the authored orientation: the map tiles are
/// world-space XZ geometry with north (−Z) at the top.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MapOrientation {
    /// Fixed, north up.
    #[default]
    NorthUp,
    /// Rotates with the player's heading.
    Rotating,
}

impl MapOrientation {
    /// Stable lowercase name for the `map=` record field.
    pub fn name(self) -> &'static str {
        match self {
            MapOrientation::NorthUp => "north",
            MapOrientation::Rotating => "rotating",
        }
    }
}

/// The session's HUD-map state — a session-scoped resource inserted
/// when the city carried authored map content (`mm2_app::hudmap` spawns
/// it alongside the tiles) and removed on teardown with the rest. No
/// map exists without the authored spec: a missing/unparseable
/// `tune/<city>.mmhudmap` or `hudmap_<city>.pkg` means no map, reported
/// on the app side's load report — never a fabricated substitute.
#[derive(Resource, Debug)]
pub struct HudMap {
    /// The authored `mmHudMap` spec this session loaded.
    pub spec: HudMapSpec,
    /// Current corner-map view.
    pub view: MapView,
    /// Fixed vs. rotating map (F).
    pub orientation: MapOrientation,
    /// `true` at the zoomed-in level, `false` zoomed out — E toggles.
    /// Seeded from the authored `ZoomIn` field (designed reading: `0` =
    /// start zoomed out).
    pub zoomed_in: bool,
    /// Current view half-extent in metres, eased toward
    /// [`Self::target_zoom`] by [`Self::advance`]. Starts *at* the
    /// target — the easing animates transitions, not the first frame.
    pub zoom: f32,
    /// Full-screen pause map (HUD-4's Q). Presentation only — the pause
    /// itself is the session's `Paused` phase, so a non-pausable
    /// authority can never open it.
    pub fullscreen: bool,
    /// Session generation this map belongs to.
    pub generation: u64,
}

impl HudMap {
    /// A map state for `spec`/`generation`, starting on the authored
    /// `ZoomIn` level with the zoom already at its target.
    pub fn new(spec: HudMapSpec, generation: u64) -> Self {
        let zoomed_in = spec.zoom_in != 0;
        let mut map = Self {
            spec,
            view: MapView::Inset,
            orientation: MapOrientation::NorthUp,
            zoomed_in,
            zoom: 0.0,
            fullscreen: false,
            generation,
        };
        map.zoom = map.target_zoom();
        map
    }

    /// Whether this resource belongs to an older session generation —
    /// the frames between a restart's `begin` and the producer's
    /// re-insert must not drive a stale map.
    pub fn is_stale(&self, generation: u64) -> bool {
        self.generation != generation
    }

    /// Whether the map camera should render at all this frame.
    pub fn visible(&self) -> bool {
        self.fullscreen || self.view != MapView::Off
    }

    /// The view half-extent (metres) the zoom eases toward: the
    /// authored in/out distance of the active presentation — the
    /// `*FS` pair while the full-screen map is up.
    pub fn target_zoom(&self) -> f32 {
        match (self.fullscreen, self.zoomed_in) {
            (false, false) => self.spec.zoom_out_dist,
            (false, true) => self.spec.zoom_in_dist,
            (true, false) => self.spec.zoom_out_dist_fs,
            (true, true) => self.spec.zoom_in_dist_fs,
        }
    }

    /// The marker extent (metres) at the current eased zoom — the
    /// authored `IconScale` pair interpolated across the zoom range,
    /// `Min` at zoomed in and `Max` at zoomed out (the interpolation
    /// is designed; the authored endpoints are not).
    pub fn icon_scale(&self) -> f32 {
        let (near, far, lo, hi) = if self.fullscreen {
            (
                self.spec.zoom_in_dist_fs,
                self.spec.zoom_out_dist_fs,
                self.spec.icon_scale_min_fs,
                self.spec.icon_scale_max_fs,
            )
        } else {
            (
                self.spec.zoom_in_dist,
                self.spec.zoom_out_dist,
                self.spec.icon_scale_min,
                self.spec.icon_scale_max,
            )
        };
        let t = ((self.zoom - near) / (far - near)).clamp(0.0, 1.0);
        lo + (hi - lo) * t
    }

    /// Ease [`Self::zoom`] toward [`Self::target_zoom`] at the authored
    /// `Approach Rate` — exponential approach, frame-rate independent.
    pub fn advance(&mut self, dt: f32) {
        let target = self.target_zoom();
        let t = 1.0 - (-self.spec.approach_rate.max(0.0) * dt).exp();
        self.zoom += (target - self.zoom) * t;
    }

    /// TAB — cycle the corner views (HUD-4).
    pub fn cycle_view(&mut self) {
        self.view = self.view.next();
    }

    /// E — toggle the zoom level (HUD-4).
    pub fn toggle_zoom(&mut self) {
        self.zoomed_in = !self.zoomed_in;
    }

    /// F — toggle north-up vs. rotating (HUD-4).
    pub fn toggle_orientation(&mut self) {
        self.orientation = match self.orientation {
            MapOrientation::NorthUp => MapOrientation::Rotating,
            MapOrientation::Rotating => MapOrientation::NorthUp,
        };
    }

    /// The `map=` record field body: `<view>/<orient>/z<zoom>` plus
    /// `fs` while the full-screen map is up.
    pub fn smoke_detail(&self) -> String {
        format!(
            "{}/{}/z{:.0}{}",
            self.view.name(),
            self.orientation.name(),
            self.zoom,
            if self.fullscreen { "/fs" } else { "" },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> HudMapSpec {
        // The retail SF record's values.
        HudMapSpec::parse(
            "type: a\nmmHudMap {\n  Size 0.21 0.25\n  Pos 0.78 0.75\n  ZoomIn 0\n  Approach Rate 1.2\n  ZoomInDist 577\n  ZoomOutDist 1195\n  IconScaleMin 34.09\n  IconScaleMax 52.4\n  ZoomInDistFS 786\n  ZoomOutDistFS 1581\n  IconScaleMinFS 15.07\n  IconScaleMaxFS 18\n  Ocean Color 0.084 0.7 0.94\n}\n",
        )
        .unwrap()
    }

    #[test]
    fn starts_inset_north_up_at_the_authored_zoom_level() {
        let m = HudMap::new(spec(), 7);
        assert_eq!(m.view, MapView::Inset);
        assert_eq!(m.orientation, MapOrientation::NorthUp);
        assert!(!m.fullscreen);
        // ZoomIn 0 → zoomed out, and already at the target (the easing
        // animates transitions, not spawn).
        assert!(!m.zoomed_in);
        assert_eq!(m.zoom, 1195.0);
        assert!(m.visible());
        assert!(!m.is_stale(7) && m.is_stale(6));
    }

    #[test]
    fn tab_cycles_two_views_and_off() {
        let mut m = HudMap::new(spec(), 0);
        m.cycle_view();
        assert_eq!(m.view, MapView::Large);
        m.cycle_view();
        assert_eq!(m.view, MapView::Off);
        assert!(!m.visible());
        m.cycle_view();
        assert_eq!(m.view, MapView::Inset);
    }

    #[test]
    fn zoom_eases_to_the_toggled_level() {
        let mut m = HudMap::new(spec(), 0);
        m.toggle_zoom();
        assert_eq!(m.target_zoom(), 577.0);
        // A second of 1/60 frames lands within the authored rate's
        // reach of the target without ever overshooting.
        for _ in 0..60 {
            m.advance(1.0 / 60.0);
        }
        assert!(m.zoom < 1195.0 && m.zoom > 577.0);
        for _ in 0..600 {
            m.advance(1.0 / 60.0);
        }
        assert!((m.zoom - 577.0).abs() < 0.5, "eased to target: {}", m.zoom);
        m.toggle_zoom();
        for _ in 0..600 {
            m.advance(1.0 / 60.0);
        }
        assert!((m.zoom - 1195.0).abs() < 0.5);
    }

    #[test]
    fn fullscreen_map_uses_the_fs_pair() {
        let mut m = HudMap::new(spec(), 0);
        m.fullscreen = true;
        assert_eq!(m.target_zoom(), 1581.0);
        m.toggle_zoom();
        assert_eq!(m.target_zoom(), 786.0);
        // Fullscreen also overrides Off.
        m.view = MapView::Off;
        assert!(m.visible());
    }

    #[test]
    fn icon_scale_interpolates_between_the_authored_pair() {
        let mut m = HudMap::new(spec(), 0);
        m.zoom = m.spec.zoom_out_dist;
        assert!((m.icon_scale() - 52.4).abs() < 1e-4);
        m.zoom = m.spec.zoom_in_dist;
        assert!((m.icon_scale() - 34.09).abs() < 1e-4);
        m.zoom = (m.spec.zoom_in_dist + m.spec.zoom_out_dist) / 2.0;
        assert!((m.icon_scale() - (34.09 + 52.4) / 2.0).abs() < 1e-3);
        // Fullscreen interpolates the FS pair instead.
        m.fullscreen = true;
        m.zoom = m.spec.zoom_in_dist_fs;
        assert!((m.icon_scale() - 15.07).abs() < 1e-4);
    }

    #[test]
    fn toggle_orientation_and_smoke_detail() {
        let mut m = HudMap::new(spec(), 0);
        m.toggle_orientation();
        assert_eq!(m.orientation, MapOrientation::Rotating);
        assert_eq!(m.smoke_detail(), "inset/rotating/z1195");
        m.fullscreen = true;
        assert!(m.smoke_detail().ends_with("/fs"));
    }
}
