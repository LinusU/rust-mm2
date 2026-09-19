//! `mm2_app` — the Bevy executable's import and world modules, exposed as a
//! library so integration tests can drive the same code paths the binary
//! uses.

pub mod camera;
pub mod city;
pub mod dev_world;
pub mod input;
pub mod vehicle_visual;

/// Whether the world finished loading. Vehicle input is ignored until the
/// world is `Ready`; a `Failed` load shows the reason instead of spawning
/// into an empty world.
#[derive(Debug, Clone, PartialEq, bevy::prelude::Resource)]
pub enum WorldState {
    /// Still building (no load attempt has run yet).
    Loading,
    /// World is built; the player may drive.
    Ready,
    /// Required content failed to load.
    Failed(String),
}
