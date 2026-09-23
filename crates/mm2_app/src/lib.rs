//! `mm2_app` — the Bevy executable's import and world modules, exposed as a
//! library so integration tests can drive the same code paths the binary
//! uses.
//!
//! Session lifecycle (load/ready/play/failure) lives in
//! `mm2_game::Session`; the app reads it instead of keeping a parallel
//! world-state resource.

pub mod banger;
pub mod breakaway;
pub mod camera;
pub mod car_visual;
pub mod city;
pub mod contracts;
pub mod damage;
pub mod damage_fx;
pub mod decals;
pub mod dev_world;
pub mod environment;
pub mod input;
pub mod menu;
pub mod nav_overlay;
pub mod opponents;
pub mod pause;
pub mod profile;
pub mod progression;
pub mod pvs;
pub mod race;
pub mod recovery;
pub mod results;
pub mod scripted;
pub mod session;
pub mod smoke;
pub mod spark_fx;
pub mod stuck;
pub mod texel_fx;
pub mod traffic;
