//! `--nav` BAI navigation debug overlay (F09-B, AC04).
//!
//! When the session config carries `dev.nav_overlay`, a City session
//! loads `city/<stem>.bai` plus `city/<stem>.aimap` into the
//! session-scoped [`CityNav`] resource, and [`draw_nav_overlay`]
//! renders it through Bevy gizmos: lane polylines lifted just above
//! the road surface, direction chevrons along each lane's travel
//! tangent, intersection centre markers, aimap-closed roads in red,
//! and an optional route probe (`--nav-route from:to`) highlighted in
//! amber. A nav load failure is logged and the session continues
//! without an overlay — diagnostics must never sink a loadable city.

use std::path::Path;

use bevy::prelude::*;
use mm2_assets::Vfs;
use mm2_game::{
    LaneId, LaneKind, NavGraph, NavIssue, NavOverrides, Route, RouteError, RouteOptions,
    SessionConfig, TravelDir, WorldMode,
};
use tracing::{info, warn};

/// Lane polylines hover this far above the curve so they render on
/// top of the road surface instead of z-fighting it.
const LIFT: f32 = 0.35;
/// Route-highlight lanes hover higher so they read over the base
/// overlay.
const ROUTE_LIFT: f32 = 0.9;
/// Metres between direction chevrons along a vehicle lane.
const CHEVRON_SPACING: f32 = 20.0;

/// The session-scoped navigation data the overlay draws. Inserted by
/// `load_session_world` only when `dev.nav_overlay` is set and the
/// graph loads; removed with the rest of the session's resources on
/// teardown.
#[derive(Resource)]
pub struct CityNav {
    /// The shared navigation graph.
    pub graph: NavGraph,
    /// Structural problems reported by the build.
    pub issues: Vec<NavIssue>,
    /// Overrides distilled from `city/<stem>.aimap`; `None` when the
    /// file is absent (a modded city may not ship one) or unparseable.
    pub overrides: Option<NavOverrides>,
    /// Route probe outcome when `dev.nav_overlay.route` was set.
    pub route: Option<Result<Route, RouteError>>,
}

/// Load the overlay data for a session config — `None` unless the
/// config is a City world with `dev.nav_overlay` set. A graph load
/// failure or an unparseable aimap is logged, not propagated.
pub fn load_city_nav(vfs: &Vfs, config: &SessionConfig) -> Option<CityNav> {
    let overlay = config.dev.nav_overlay.as_ref()?;
    let WorldMode::City { psdl } = &config.world else {
        warn!("--nav has no effect outside a city world");
        return None;
    };
    let stem = Path::new(psdl)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(psdl.as_str());
    let build = match mm2_content::load_nav_graph(vfs, stem) {
        Ok(b) => b,
        Err(e) => {
            warn!(error = %e, "nav overlay: graph failed to load");
            return None;
        }
    };
    let overrides = match mm2_content::load_nav_overrides(vfs, &format!("city/{stem}.aimap")) {
        Ok(o) => o,
        Err(e) => {
            warn!(error = %e, "nav overlay: aimap failed to parse; overrides disabled");
            None
        }
    };
    let route = overlay.route.map(|(from, to)| {
        let opts = overrides
            .as_ref()
            .map_or_else(RouteOptions::default, NavOverrides::route_options);
        build.graph.route_roads(from, to, &opts)
    });
    if let Some(o) = &overrides {
        info!(
            closed = o.closed_roads.len(),
            speed_limit = ?o.default_speed_limit,
            "nav overlay: aimap overrides applied"
        );
    }
    Some(CityNav {
        graph: build.graph,
        issues: build.issues,
        overrides,
        route,
    })
}

/// A semantic class per overlay segment; [`draw_nav_overlay`] maps
/// these to colors, tests assert on the classes themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayClass {
    /// Vehicle lane travelling with the authored section order.
    LaneForward,
    /// Vehicle lane travelling against it.
    LaneBackward,
    /// Sidewalk curve, or a non-routable vehicle curve.
    Sidewalk,
    /// Tram/train rail curve.
    Rail,
    /// Direction chevron along a vehicle lane's travel tangent.
    Direction,
    /// Lane on an aimap-closed road.
    Closed,
    /// Lane of an arc on the route probe.
    Route,
    /// Intersection centre marker.
    Intersection,
}

/// One drawable segment: world-space endpoints plus its class.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OverlaySegment {
    /// Segment start.
    pub a: Vec3,
    /// Segment end.
    pub b: Vec3,
    /// What the segment depicts.
    pub class: OverlayClass,
}

/// Build the overlay's segments from the loaded nav data — pure, so
/// the geometry can be asserted without a renderer.
pub fn overlay_lines(nav: &CityNav) -> Vec<OverlaySegment> {
    let g = &nav.graph;
    let lift = |p: [f32; 3], dy: f32| Vec3::new(p[0], p[1] + dy, p[2]);
    let mut out = Vec::new();

    let route_lanes: Vec<LaneId> = match &nav.route {
        Some(Ok(r)) => r
            .steps
            .iter()
            .flat_map(|a| g.arc(*a).lanes.iter().copied())
            .collect(),
        _ => Vec::new(),
    };

    for lane in g.lanes() {
        let closed = nav
            .overrides
            .as_ref()
            .is_some_and(|o| o.is_closed(lane.id.road));
        let class = if closed {
            OverlayClass::Closed
        } else if route_lanes.contains(&lane.id) {
            OverlayClass::Route
        } else {
            match lane.id.kind {
                LaneKind::Vehicle => match lane.arc.map(|a| g.arc(a).dir) {
                    Some(TravelDir::Forward) => OverlayClass::LaneForward,
                    Some(TravelDir::Backward) => OverlayClass::LaneBackward,
                    None => OverlayClass::Sidewalk,
                },
                LaneKind::Sidewalk => OverlayClass::Sidewalk,
                LaneKind::Tram | LaneKind::Train => OverlayClass::Rail,
            }
        };
        let dy = if class == OverlayClass::Route {
            ROUTE_LIFT
        } else {
            LIFT
        };
        for w in lane.vertices().windows(2) {
            out.push(OverlaySegment {
                a: lift(w[0], dy),
                b: lift(w[1], dy),
                class,
            });
        }
        // Direction chevrons on routable vehicle lanes: an arrowhead
        // whose tip points along the travel tangent.
        if lane.id.kind == LaneKind::Vehicle && lane.arc.is_some() {
            let mut s = CHEVRON_SPACING * 0.5;
            while s < lane.length {
                if let Some(sample) = g.sample_lane(lane.id, s) {
                    let p = lift(sample.position, dy);
                    let t = Vec3::from_array(sample.tangent);
                    let side = Vec3::new(-t.z, 0.0, t.x);
                    let tip = p + t * 2.2;
                    out.push(OverlaySegment {
                        a: tip,
                        b: p + side,
                        class: OverlayClass::Direction,
                    });
                    out.push(OverlaySegment {
                        a: tip,
                        b: p - side,
                        class: OverlayClass::Direction,
                    });
                }
                s += CHEVRON_SPACING;
            }
        }
    }

    for int in g.intersections() {
        let c = lift(int.center, 1.5);
        out.push(OverlaySegment {
            a: c - Vec3::X * 3.0,
            b: c + Vec3::X * 3.0,
            class: OverlayClass::Intersection,
        });
        out.push(OverlaySegment {
            a: c - Vec3::Z * 3.0,
            b: c + Vec3::Z * 3.0,
            class: OverlayClass::Intersection,
        });
    }
    out
}

/// One-line HUD summary of the overlay state (`nav …a/…l closed=N
/// route=…`), appended to the HUD line when the resource is present.
pub fn hud_summary(nav: &CityNav) -> String {
    let s = nav.graph.stats();
    let mut t = format!("nav {}a/{}l", s.vehicle_arcs, s.vehicle_lanes);
    if let Some(o) = &nav.overrides {
        t += &format!(" closed={}", o.closed_roads.len());
    }
    match &nav.route {
        Some(Ok(r)) => t += &format!(" route={}st/{:.0}m", r.steps.len(), r.length),
        Some(Err(e)) => t += &format!(" route=err:{e}"),
        None => {}
    }
    t
}

/// Render the overlay through gizmos — emitted fresh every frame.
pub fn draw_nav_overlay(nav: Res<CityNav>, mut gizmos: Gizmos) {
    for s in overlay_lines(&nav) {
        gizmos.line(s.a, s.b, class_color(s.class));
    }
}

fn class_color(class: OverlayClass) -> Color {
    match class {
        OverlayClass::LaneForward => Color::srgb(0.2, 0.9, 0.3),
        OverlayClass::LaneBackward => Color::srgb(0.2, 0.6, 0.95),
        OverlayClass::Sidewalk => Color::srgb(0.5, 0.5, 0.5),
        OverlayClass::Rail => Color::srgb(0.7, 0.4, 0.9),
        OverlayClass::Direction => Color::srgb(1.0, 1.0, 1.0),
        OverlayClass::Closed => Color::srgb(0.95, 0.15, 0.15),
        OverlayClass::Route => Color::srgb(1.0, 0.75, 0.1),
        OverlayClass::Intersection => Color::srgb(1.0, 0.9, 0.2),
    }
}
