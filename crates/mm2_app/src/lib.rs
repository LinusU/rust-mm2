//! `mm2_app` — the Bevy executable's import and world modules, exposed as a
//! library so integration tests can drive the same code paths the binary
//! uses.
//!
//! Session lifecycle (load/ready/play/failure) lives in
//! `mm2_game::Session`; the app reads it instead of keeping a parallel
//! world-state resource.

pub mod audio;
pub mod banger;
pub mod breakaway;
pub mod camera;
pub mod car_visual;
pub mod city;
pub mod cnr;
pub mod cnrhud;
pub mod cnrnet;
pub mod cnrvoice;
pub mod contracts;
pub mod damage;
pub mod damage_fx;
pub mod dash;
pub mod decals;
pub mod dev_world;
pub mod drawbridge;
pub mod environment;
pub mod hud;
pub mod hudmap;
pub mod input;
pub mod layers;
pub mod menu;
pub mod movers;
pub mod nav_overlay;
pub mod navarrow;
pub mod navarrow3d;
pub mod net;
pub mod netdrive;
pub mod object_sound;
pub mod oppind;
pub mod opponents;
pub mod pause;
pub mod perf;
pub mod precip;
pub mod profile;
pub mod progression;
pub mod pvs;
pub mod race;
pub mod race_audio;
pub mod racestat;
pub mod racetime;
pub mod racing_line;
pub mod recovery;
pub mod results;
pub mod scripted;
pub mod sequence;
pub mod session;
pub mod settings;
pub mod smoke;
pub mod spark_fx;
pub mod speedometer;
pub mod stuck;
pub mod texel_fx;
pub mod traffic;
pub mod underground;
pub mod water;
pub mod wheel_fx;
pub mod worldclock;
pub mod worldprops;
pub mod worldtraffic;

pub(crate) mod motion_evidence;

mod checkpoint_gate;
