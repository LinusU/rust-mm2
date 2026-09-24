//! Vehicle audio contracts (F07) — the authored `aud/cardata` bindings
//! a spawned vehicle carries, shared between the content loader and the
//! app-side voice systems.
//!
//! Only the *binding* lives here: which samples the vehicle references
//! and at what authored volumes. Voice lifecycle, wave resolution and
//! mixing are app concerns (`mm2_app::audio`), and the engine-sample
//! fade-window semantics are still research-open (UNK-25).

use bevy::prelude::*;
use mm2_formats::cardata::CarAudio;

/// The authored per-vehicle audio table attached to a spawned vehicle —
/// `aud/cardata/{player,opponent}/<id>.csv` verbatim (F07-A.2).
///
/// Absent when the vehicle's cardata record did not resolve or decode:
/// consumers treat a missing `VehicleAudio` as "no authored audio" and
/// report it rather than fabricating bindings.
#[derive(Component, Debug, Clone)]
pub struct VehicleAudio {
    /// The parsed cardata body (horn/clutch bindings + engine rows).
    pub spec: CarAudio,
}
