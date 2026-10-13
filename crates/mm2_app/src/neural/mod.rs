//! Neural-network opponent driving.
//!
//! A small policy network turns what a car can see of its route and
//! surroundings into steering and pedals; route choice, race rules and
//! last-resort recovery stay conventional code around it.
//!
//! - [`guide`] — the line a driver follows and its position on it.
//! - [`observe`] — the observation layout and the action mapping.
//! - [`policy`] — the network itself, evaluated on the CPU.
//! - [`env`] — the lean multi-car environment the policy is trained in
//!   (served to the trainer by the `mm2_gym` example).
//!
//! See `docs/neural-opponents.md` for the training pipeline.

pub mod env;
pub mod guide;
pub mod observe;
pub mod policy;
