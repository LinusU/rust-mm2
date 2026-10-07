//! Single-player police on the road (F20-A.2): the event's authored
//! `[Police]` lineup fielded as real, session-owned cars.
//!
//! [`spawn_police`] turns each [`PoliceSpec`] of the session's
//! [`PoliceRoster`] (the F20-A.1 content producer) into a physics car:
//! the authored vehicle through the same VFS → `load_opponent` →
//! [`equip_authored_vehicle`] path the racing opponents use (the
//! `aud/cardata/opponent` audio side, the authored damage/smoke/spark/
//! stuck/break records), stamped with the session's `SessionEntity`,
//! `ObjectIdentity` and authority role so teardown, replication-by-
//! authority and damage all treat it like any other simulated car.
//!
//! A cop is deliberately **not** a [`Player`](mm2_game::Player): it has
//! no `RaceProgress`, never appears in the standings, the opponent
//! indicator or the result ledger, and is not a network client.
//!
//! What this slice does *not* do is chase anyone. Pursuit rules are
//! unverified (ledger COP-4 / UNK-9), so a fielded cop stands at its
//! authored position and heading with the handbrake held until the
//! detect → pursue state machine (F20-A.3) lands; `PoliceCar` is the
//! component that machine attaches its state to. The heading's unit and
//! zero axis are inferred (the vehicle-yaw convention — forward
//! `(−sin h, −cos h)` — every other authored heading uses); a road-lane
//! comparison of the Cruise rows was inconclusive (ledger COP-8).
//!
//! Single-player only: like the AI opponents (MP-4) police are not
//! replicated, so a networked session fields none.

use bevy::prelude::*;
use mm2_assets::Vfs;
use mm2_game::{
    DamageSignals, ObjectIdentity, PoliceRoster, PoliceSpec, Session, SessionAuthority,
    SessionEntity,
};
use mm2_vehicle::{VehicleInput, vehicle_bundle};
use tracing::{info, warn};

use crate::opponents::{equip_authored_vehicle, reanchor_lift};

/// One fielded police car — the authored row it came from.
#[derive(Component, Debug, Clone)]
pub struct PoliceCar {
    /// Index into the roster the car was spawned from (authored order).
    pub index: usize,
    /// The authored `[Police]` row, verbatim.
    pub spec: PoliceSpec,
}

/// What [`spawn_police`] did with the roster: the denominator and every
/// way a row can fail to become a car. Session-scoped (inserted at
/// load, removed at teardown) — the smoke record's `police=` reads it.
#[derive(Resource, Debug, Clone, Default, PartialEq, Eq)]
pub struct PoliceFleet {
    /// Authored `[Police]` rows in the roster.
    pub authored: usize,
    /// Cars spawned.
    pub spawned: usize,
    /// Rows refused because the position or heading is unusable
    /// ([`PoliceSpec::placeable`]).
    pub unplaceable: usize,
    /// Rows whose vehicle did not load; the authored slot is skipped.
    pub load_failed: usize,
}

impl PoliceFleet {
    /// Whether the session had any authored police at all — the smoke
    /// record stays silent for the (majority) no-cop sessions.
    pub fn any(&self) -> bool {
        self.authored > 0
    }

    /// The smoke record's `pol=` value: `<spawned>/<authored>`, then
    /// `,uns<N>` (unplaceable rows) and `,fail<N>` (vehicles that did
    /// not load) only when nonzero. A networked session reads `0/<n>`.
    pub fn smoke_detail(&self) -> String {
        let mut s = format!("{}/{}", self.spawned, self.authored);
        if self.unplaceable > 0 {
            s.push_str(&format!(",uns{}", self.unplaceable));
        }
        if self.load_failed > 0 {
            s.push_str(&format!(",fail{}", self.load_failed));
        }
        s
    }
}

/// The yaw a cop is staged with: the authored heading in degrees as a
/// vehicle yaw (forward `(−sin h, −cos h)`); a row with no heading
/// faces the world's default (0).
pub fn staging_yaw(spec: &PoliceSpec) -> f32 {
    spec.heading_deg.unwrap_or(0.0).to_radians()
}

/// Field the roster's police as session-owned cars; returns the fleet
/// report (also what the caller inserts as a resource). An
/// unplaceable row or an unloadable vehicle is skipped and counted —
/// the roster is never padded, and the authored order is kept so a
/// skipped row never renumbers the rest. Networked sessions field none
/// (see the module docs); the report still carries the authored count
/// so the omission is visible.
#[allow(clippy::too_many_arguments)]
pub fn spawn_police(
    commands: &mut Commands,
    vfs: &Vfs,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
    roster: &PoliceRoster,
    authority: SessionAuthority,
    owner: SessionEntity,
    session: &mut Session,
) -> PoliceFleet {
    let mut fleet = PoliceFleet {
        authored: roster.entries.len(),
        ..PoliceFleet::default()
    };
    if authority != SessionAuthority::Local {
        return fleet;
    }
    for issue in &roster.issues {
        warn!(issue = %issue, "police roster issue");
    }
    let role = session.authority_role();
    for (index, spec) in roster.entries.iter().enumerate() {
        if !spec.placeable() {
            fleet.unplaceable += 1;
            warn!(
                line = spec.line,
                "police row unplaceable — authored slot skipped"
            );
            continue;
        }
        let def = match mm2_content::load_opponent(vfs, &spec.vehicle, 0) {
            Ok(def) => def,
            Err(e) => {
                fleet.load_failed += 1;
                warn!(
                    vehicle = %spec.vehicle,
                    error = %e,
                    "police vehicle failed to load — authored slot skipped"
                );
                continue;
            }
        };
        let yaw = staging_yaw(spec);
        let mut pos = spec.position;
        pos.y += reanchor_lift(&def.config);
        let object = session.mint_object_id();
        let vehicle = commands
            .spawn((
                owner,
                ObjectIdentity(object),
                role,
                DamageSignals::default(),
                PoliceCar {
                    index,
                    spec: spec.clone(),
                },
                vehicle_bundle(&def.config),
                Transform::from_translation(pos).with_rotation(Quat::from_rotation_y(yaw)),
                avian3d::prelude::TransformInterpolation,
                Visibility::Visible,
            ))
            .id();
        // Held in place: nothing drives a cop yet, and a released car
        // would roll off its authored spot on any slope.
        commands.entity(vehicle).insert(VehicleInput {
            handbrake: 1.0,
            ..VehicleInput::default()
        });
        equip_authored_vehicle(
            commands, vfs, meshes, images, materials, vehicle, &def, object, pos, yaw,
        );
        fleet.spawned += 1;
    }
    if fleet.spawned > 0 {
        info!(
            police = fleet.spawned,
            authored = fleet.authored,
            "police roster fielded"
        );
    }
    fleet
}
