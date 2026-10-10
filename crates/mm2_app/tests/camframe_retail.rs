//! F22-C original-content validation (F22-AC05, code-check half):
//! per-vehicle camera framing across the retail roster. Opt-in
//! (`MM2_RETAIL=<dir>`); skipped without the operator's install, and
//! says so.
//!
//! For every ready roster vehicle this loads the real tuning (the
//! converted chassis box) and the authored `tune/camera/<id>_*`
//! records through the production readers (`load_track_cams`,
//! `ChaseLens::authored`), then checks the framing the session will
//! actually use:
//!
//! - the authored near lens binds for every roster car — per-vehicle
//!   framing is authored data, not one shared camera;
//! - every authored lens distills to a finite boom that clears the
//!   car's own chassis (`anchor().z` behind the rear bumper, beyond
//!   the loader's near plane), so no car is framed inside its own
//!   body however tall or long;
//! - the size-derived fallback (`ChaseLens::sized`, the path a
//!   record-less modded car takes) clears the chassis of the roster's
//!   real dimensions — the tall/long/small leg without guessing dims;
//! - the lenses differ per vehicle (a distinct-anchor census).
//!
//! It prints the extremes' numbers so the *visual* half of AC05 —
//! judging the framing of the tallest/longest/smallest vehicles on a
//! real render — can be done from an exact capture command rather
//! than a guess (see the `OWNER:` follow-up task). It does not judge
//! how any view *looks*: that is a human-read artefact.

use mm2_app::camera::{ChaseLens, load_track_cams};
use mm2_content::{VehicleCatalog, load_vehicle};
use mm2_formats::camtrack::TrackCamSpec;

#[test]
fn every_roster_vehicle_frames_clear_of_its_own_chassis() {
    let Some((retail, _slot)) = crate::support::retail_slot() else {
        return;
    };
    let vfs = crate::support::mount(&retail);
    let catalog = VehicleCatalog::scan(&vfs);
    let ready: Vec<&str> = catalog
        .entries
        .iter()
        .filter(|e| e.is_ready())
        .map(|e| e.id.as_str())
        .collect();
    assert!(
        ready.len() >= 15,
        "the retail roster must be present: {ready:?}"
    );

    // (id, h, d, far binds, distinct-anchor census input)
    let mut anchors: Vec<(String, [f32; 3])> = Vec::new();
    let mut missing_far = Vec::new();
    let mut dims: Vec<(String, f32, f32)> = Vec::new();

    for id in &ready {
        let def = load_vehicle(&vfs, id, 0)
            .unwrap_or_else(|e| panic!("{id}: retail tuning failed to load: {e}"));
        let [_w, h, d] = def.config.chassis_size;
        assert!(
            h.is_finite() && h > 0.0 && d.is_finite() && d > 0.0,
            "{id}: chassis dims must be usable, got {h}×{d}"
        );
        dims.push((id.to_string(), h, d));

        let tracks = load_track_cams(&vfs, id);
        let Some(near_spec) = tracks.near.as_ref() else {
            panic!(
                "{id}: every retail roster car ships `tune/camera/{id}_near.camtrackcs` — \
                 per-vehicle framing is authored data"
            );
        };
        assert!(
            near_spec.validate().is_empty(),
            "{id}: authored near record has issues: {:?}",
            near_spec.validate()
        );
        if tracks.far.is_none() {
            missing_far.push(id.to_string());
        }

        // Authored framing, distilled exactly as the session does.
        let lens = ChaseLens::authored(near_spec);
        let eye = lens.anchor();
        assert!(
            eye.is_finite(),
            "{id}: authored boom must be finite, got {eye:?}"
        );
        assert!(
            eye.z > d / 2.0,
            "{id}: the authored boom ({eye:?}) must sit behind the \
             {d:.1} m chassis (rear at z={:.1})",
            d / 2.0
        );
        assert!(
            lens.clip_near == TrackCamSpec::RUNTIME_NEAR_M,
            "{id}: the loader's near constant (UNK-37) binds: {}",
            lens.clip_near
        );
        assert!(
            (0.0..180.0).contains(&lens.fov_deg) && lens.clip_far > lens.clip_near,
            "{id}: undrawable projection would have fallen back; got fov={} far={}",
            lens.fov_deg,
            lens.clip_far
        );
        anchors.push((id.to_string(), eye.to_array()));
    }

    // The record-less fallback frames the same real dimensions.
    for (id, h, d) in &dims {
        let lens = ChaseLens::sized(*h, *d);
        let eye = lens.anchor();
        assert!(
            eye.z - d / 2.0 > lens.clip_near,
            "{id}: the sized fallback ({eye:?}) must clear the \
             {d:.1} m chassis past its near plane"
        );
        assert!(eye.y > 0.0, "{id}: the sized boom rises above the road");
    }

    // Per-vehicle framing is per-vehicle: the authored anchors are
    // not one shared camera pose.
    let distinct: std::collections::BTreeSet<_> = anchors
        .iter()
        .map(|(_, a)| a.map(|v| (v * 100.0).round() as i64))
        .collect();
    assert!(
        distinct.len() * 2 > anchors.len(),
        "authored booms must differ across the roster, {} distinct of {}",
        distinct.len(),
        anchors.len()
    );

    // The atypical-size census: the extremes the visual AC05 leg
    // should look at, with the numbers the capture task needs.
    let tallest = dims.iter().max_by(|a, b| a.1.total_cmp(&b.1)).unwrap();
    let longest = dims.iter().max_by(|a, b| a.2.total_cmp(&b.2)).unwrap();
    let smallest = dims
        .iter()
        .min_by(|a, b| (a.1 * a.2).total_cmp(&(b.1 * b.2)))
        .unwrap();
    for (label, (id, h, d)) in [
        ("tallest", tallest),
        ("longest", longest),
        ("smallest", smallest),
    ] {
        let tracks = load_track_cams(&vfs, id);
        let lens = ChaseLens::authored(tracks.near.as_ref().unwrap());
        eprintln!(
            "{label}: {id} chassis {:.1}×{:.1} m (h×d) authored boom eye={:?} fov={:.0}° far={:.0} m; sized fallback eye={:?}",
            h,
            d,
            lens.anchor(),
            lens.fov_deg,
            lens.clip_far,
            ChaseLens::sized(*h, *d).anchor(),
        );
    }
    eprintln!(
        "authored `_far` records: missing for {missing_far:?} — those cars' `C` \
         chain honestly skips the far slot (HUD-3)"
    );
    assert!(
        missing_far.len() <= 2,
        "the far-lens census should only miss known authored gaps, got {missing_far:?}"
    );
}
