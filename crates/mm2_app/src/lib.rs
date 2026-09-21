//! `mm2_app` — the Bevy executable's import and world modules, exposed as a
//! library so integration tests can drive the same code paths the binary
//! uses.
//!
//! Session lifecycle (load/ready/play/failure) lives in
//! `mm2_game::Session`; the app reads it instead of keeping a parallel
//! world-state resource.

pub mod banger;
pub mod camera;
pub mod car_visual;
pub mod city;
pub mod contracts;
pub mod decals;
pub mod dev_world;
pub mod input;
pub mod nav_overlay;
pub mod opponents;
pub mod profile;
pub mod race;
pub mod scripted;
pub mod session;
pub mod smoke;
