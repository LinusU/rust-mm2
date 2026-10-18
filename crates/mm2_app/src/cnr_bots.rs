//! CNRBOT-A — local Cops & Robbers bots: the roster, its placement and
//! its seats in the gold match.
//!
//! A designed single-player enhancement, **not** a claim about the
//! retail mode (which seats humans only). A session whose
//! [`SessionConfig::cnr_bots`](mm2_game::SessionConfig::cnr_bots) is `n`
//! fields `n` extra simulated cars beside the one human. They are real
//! participants: each is a separate entity with its own `ObjectId` and
//! `PlayerId`, `PlayerControl::Ai`, the session's authority role, the
//! authored vehicle bundle and damage rig, and the session owner so
//! teardown (exit, restart, rematch) removes them with everything else.
//! [`crate::cnr::enroll_cnr_participants`] seats exactly the cars
//! carrying [`CnrBot`] — ordinary traffic, police and race opponents
//! never carry it — and the gold host step then treats a bot like the
//! human: the same pickup, ram, delivery and score pipeline.
//!
//! This module does not drive the cars. A bot stands where it was put
//! until the navigation and planner tasks (CNRBOT-B/C) give it inputs;
//! it deliberately has no [`crate::opponents::OpponentDriver`] and no
//! `RaceProgress`, so no race system can mistake it for a finisher.
//!
//! Placement is a pure function of the city, the seed and the roster
//! size: candidate poses are the midpoints of the city's vehicle lanes
//! (on the road surface, facing the direction of travel), minus any in
//! deadly water, near the human's start or on a gold-site marker, then
//! drawn in a seeded order with a minimum spacing. A placement that
//! cannot be satisfied is an error with counts per rejection reason —
//! the session fails rather than fielding a short roster unannounced.

use avian3d::prelude::*;
use bevy::prelude::*;
use mm2_assets::Vfs;
use mm2_content::{EntryStatus, VehicleCatalog, VehicleClass, VehicleDef};
use mm2_game::{
    DamageSignals, LaneKind, MAX_CNR_BOTS, NavGraph, NavRng, ObjectIdentity, Player, PlayerControl,
    RecoveryPolicy, Session, SessionEntity, VehicleRecovery,
};
use mm2_vehicle::vehicle_bundle;
use tracing::{info, warn};

use crate::opponents::{equip_authored_vehicle, reanchor_lift};
use crate::water::CityWater;

/// Tags a car as one of the session's Cops & Robbers bots. The only
/// AI car [`crate::cnr::enroll_cnr_participants`] seats.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct CnrBot {
    /// Position in the roster, `0..n`.
    pub index: u8,
}

/// Shortest vehicle lane a bot may stand on, metres: a stub between
/// junction boxes is no place to park a car.
pub const MIN_LANE_LENGTH: f32 = 14.0;
/// Least distance between two bots' starts, metres.
pub const BOT_SPACING: f32 = 20.0;
/// Least distance from a bot's start to the human's start, metres.
pub const HUMAN_CLEARANCE: f32 = 25.0;
/// Least distance from a bot's start to a gold site (marker), metres.
pub const SITE_CLEARANCE: f32 = 15.0;

/// Where a bot starts: world position on the road surface and yaw.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BotSlot {
    /// World position on the lane, before the hull-clearance lift.
    pub position: Vec3,
    /// Yaw facing the lane's direction of travel.
    pub yaw: f32,
}

/// Why a roster could not be fielded.
#[derive(Debug, Clone, PartialEq)]
pub enum BotError {
    /// More bots than [`MAX_CNR_BOTS`].
    Count(u8),
    /// The city ships no usable navigation graph to place cars on.
    NoGraph(String),
    /// Not enough safe, spaced poses; the counts say where they went.
    Placement {
        /// Bots asked for.
        wanted: usize,
        /// Poses found.
        found: usize,
        /// Lane midpoints considered.
        candidates: usize,
        /// Rejected: in deadly water.
        water: usize,
        /// Rejected: within [`HUMAN_CLEARANCE`] of the human start.
        near_human: usize,
        /// Rejected: within [`SITE_CLEARANCE`] of a gold site.
        near_site: usize,
        /// Rejected: within [`BOT_SPACING`] of an earlier bot.
        crowded: usize,
    },
    /// Fewer loadable stock vehicles than bots needing one.
    Vehicles {
        /// Bots asked for.
        wanted: usize,
        /// Distinct loadable vehicles.
        loadable: usize,
        /// `id: reason` per vehicle that failed to load.
        failures: Vec<String>,
    },
}

impl std::fmt::Display for BotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Count(n) => write!(f, "{n} bots is above the {MAX_CNR_BOTS} allowed"),
            Self::NoGraph(e) => write!(f, "no navigation graph to place bots on: {e}"),
            Self::Placement {
                wanted,
                found,
                candidates,
                water,
                near_human,
                near_site,
                crowded,
            } => write!(
                f,
                "only {found} of {wanted} bot starts found among {candidates} lane poses \
                 (rejected: {water} in water, {near_human} near the player, \
                 {near_site} on a gold site, {crowded} too close to another bot)"
            ),
            Self::Vehicles {
                wanted,
                loadable,
                failures,
            } => write!(
                f,
                "{wanted} bots need vehicles but only {loadable} stock cars loaded ({})",
                failures.join("; ")
            ),
        }
    }
}

impl std::error::Error for BotError {}

/// A planned bot: the car it drives and where it starts.
pub struct PlannedBot {
    /// The authored vehicle (opponent tuning preferred, as race
    /// opponents use).
    pub def: VehicleDef,
    /// The start pose.
    pub slot: BotSlot,
}

/// Every vehicle-lane midpoint of the city that is not in deadly
/// water, in graph order (deterministic). Position is on the lane,
/// yaw faces its travel direction.
pub fn lane_slots(graph: &NavGraph, water: Option<&CityWater>) -> Vec<(BotSlot, bool)> {
    graph
        .lanes()
        .iter()
        .filter(|l| l.id.kind == LaneKind::Vehicle && l.length >= MIN_LANE_LENGTH)
        .filter_map(|l| {
            let s = graph.sample_lane(l.id, l.length * 0.5)?;
            let position = Vec3::from(s.position);
            let tangent = Vec3::from(s.tangent);
            if !position.is_finite() || !tangent.is_finite() {
                return None;
            }
            let flat = Vec3::new(tangent.x, 0.0, tangent.z).normalize_or_zero();
            if flat == Vec3::ZERO {
                return None;
            }
            let yaw = (-flat.x).atan2(-flat.z);
            let wet = water.is_some_and(|w| w.is_deadly(position));
            Some((BotSlot { position, yaw }, wet))
        })
        .collect()
}

/// Draw `n` start poses from `candidates` (`(slot, in_water)`), in a
/// seeded order, honouring the spacing and clearances. `human` is the
/// player's start and `sites` the gold-site markers.
pub fn choose_slots(
    candidates: &[(BotSlot, bool)],
    n: usize,
    seed: u64,
    human: Vec3,
    sites: &[Vec3],
) -> Result<Vec<BotSlot>, BotError> {
    let mut order: Vec<usize> = (0..candidates.len()).collect();
    let mut rng = NavRng::new(seed ^ 0xC0B5_B075);
    for i in (1..order.len()).rev() {
        let j = (rng.next_u64() % (i as u64 + 1)) as usize;
        order.swap(i, j);
    }
    let (mut water, mut near_human, mut near_site, mut crowded) = (0, 0, 0, 0);
    let mut chosen: Vec<BotSlot> = Vec::with_capacity(n);
    for &i in &order {
        if chosen.len() == n {
            break;
        }
        let (slot, wet) = candidates[i];
        let p = slot.position;
        if wet {
            water += 1;
        } else if p.distance(human) < HUMAN_CLEARANCE {
            near_human += 1;
        } else if sites.iter().any(|s| p.distance(*s) < SITE_CLEARANCE) {
            near_site += 1;
        } else if chosen.iter().any(|c| c.position.distance(p) < BOT_SPACING) {
            crowded += 1;
        } else {
            chosen.push(slot);
        }
    }
    if chosen.len() < n {
        return Err(BotError::Placement {
            wanted: n,
            found: chosen.len(),
            candidates: candidates.len(),
            water,
            near_human,
            near_site,
            crowded,
        });
    }
    Ok(chosen)
}

/// The ids of the stock vehicles bots may drive: complete, retail
/// content (no mod-supplied car), sorted by id.
pub fn stock_vehicle_ids(vfs: &Vfs) -> Vec<String> {
    VehicleCatalog::scan(vfs)
        .entries
        .into_iter()
        .filter(|e| e.class == VehicleClass::Stock && matches!(e.status, EntryStatus::Ready))
        .map(|e| e.id)
        .collect()
}

/// Load `n` distinct vehicles from `ids`, starting at a seeded offset
/// and stepping in id order; one that fails to load is skipped and
/// reported only if the roster then comes up short.
pub fn load_vehicles(
    vfs: &Vfs,
    ids: &[String],
    n: usize,
    seed: u64,
) -> Result<Vec<VehicleDef>, BotError> {
    let mut defs = Vec::with_capacity(n);
    let mut failures = Vec::new();
    if !ids.is_empty() {
        let start = (NavRng::new(seed ^ 0x0CA2_5EED).next_u64() % ids.len() as u64) as usize;
        for k in 0..ids.len() {
            if defs.len() == n {
                break;
            }
            let id = &ids[(start + k) % ids.len()];
            match mm2_content::load_opponent(vfs, id, 0) {
                Ok(def) => defs.push(def),
                Err(e) => failures.push(format!("{id}: {e}")),
            }
        }
    }
    if defs.len() < n {
        // Beyond the number of distinct cars the roster repeats them in
        // the same order, so a small install still fields every bot.
        if defs.is_empty() {
            return Err(BotError::Vehicles {
                wanted: n,
                loadable: 0,
                failures,
            });
        }
        let base = defs.len();
        for i in base..n {
            match mm2_content::load_opponent(vfs, &defs[i % base].id, 0) {
                Ok(def) => defs.push(def),
                Err(e) => {
                    failures.push(format!("{}: {e}", defs[i % base].id));
                    return Err(BotError::Vehicles {
                        wanted: n,
                        loadable: base,
                        failures,
                    });
                }
            }
        }
    }
    for f in &failures {
        warn!(failure = %f, "bot vehicle skipped");
    }
    Ok(defs)
}

/// Plan the whole roster: starts, then cars. Pure given the VFS.
pub fn plan_bots(
    vfs: &Vfs,
    candidates: &[(BotSlot, bool)],
    n: u8,
    seed: u64,
    human: Vec3,
    sites: &[Vec3],
) -> Result<Vec<PlannedBot>, BotError> {
    if n > MAX_CNR_BOTS {
        return Err(BotError::Count(n));
    }
    let n = n as usize;
    let slots = choose_slots(candidates, n, seed, human, sites)?;
    let defs = load_vehicles(vfs, &stock_vehicle_ids(vfs), n, seed)?;
    Ok(defs
        .into_iter()
        .zip(slots)
        .map(|(def, slot)| PlannedBot { def, slot })
        .collect())
}

/// Spawn the planned roster as session-owned AI cars, in index order
/// (so `PlayerId`s ascend with the index and the side split is
/// deterministic). Returns the entities.
#[allow(clippy::too_many_arguments)]
pub fn spawn_bots(
    commands: &mut Commands,
    vfs: &Vfs,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
    plan: Vec<PlannedBot>,
    owner: SessionEntity,
    session: &mut Session,
) -> Vec<Entity> {
    let role = session.authority_role();
    let mut spawned = Vec::with_capacity(plan.len());
    for (index, bot) in plan.into_iter().enumerate() {
        let PlannedBot { def, slot } = bot;
        let mut pos = slot.position;
        pos.y += reanchor_lift(&def.config);
        let object = session.mint_object_id();
        let player_id = session.mint_player_id();
        let vehicle = commands
            .spawn((
                owner,
                CnrBot { index: index as u8 },
                ObjectIdentity(object),
                Player {
                    id: player_id,
                    control: PlayerControl::Ai,
                },
                role,
                DamageSignals::default(),
                VehicleRecovery::with_anchor(RecoveryPolicy::default(), pos, slot.yaw),
                vehicle_bundle(&def.config),
                Transform::from_translation(pos).with_rotation(Quat::from_rotation_y(slot.yaw)),
                TransformInterpolation,
                Visibility::Visible,
            ))
            .id();
        equip_authored_vehicle(
            commands, vfs, meshes, images, materials, vehicle, &def, object, pos, slot.yaw,
        );
        spawned.push(vehicle);
    }
    info!(bots = spawned.len(), "cops and robbers bots spawned");
    spawned
}

/// Load the city's navigation graph for placement, strictly: a city
/// without one cannot field bots.
pub fn load_graph(vfs: &Vfs, city: &str) -> Result<NavGraph, BotError> {
    mm2_content::load_routing_nav_graph(vfs, city)
        .map(|b| b.graph)
        .map_err(|e| BotError::NoGraph(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 30 x 30 grid of lane poses 30 m apart.
    fn grid() -> Vec<(BotSlot, bool)> {
        (0..900)
            .map(|i| {
                let (x, z) = ((i % 30) as f32 * 30.0, (i / 30) as f32 * 30.0);
                (
                    BotSlot {
                        position: Vec3::new(x, 0.0, z),
                        yaw: 0.0,
                    },
                    false,
                )
            })
            .collect()
    }

    #[test]
    fn placement_is_deterministic_spaced_and_seed_dependent() {
        let c = grid();
        let human = Vec3::new(0.0, 0.0, 0.0);
        let a = choose_slots(&c, 15, 3, human, &[]).unwrap();
        assert_eq!(a, choose_slots(&c, 15, 3, human, &[]).unwrap());
        assert_ne!(a, choose_slots(&c, 15, 4, human, &[]).unwrap());
        for (i, s) in a.iter().enumerate() {
            assert!(s.position.distance(human) >= HUMAN_CLEARANCE);
            assert!(
                a[i + 1..]
                    .iter()
                    .all(|o| o.position.distance(s.position) >= BOT_SPACING)
            );
        }
    }

    #[test]
    fn water_sites_and_the_human_are_rejected_with_counts() {
        let mut c = grid();
        for (_, wet) in c.iter_mut().take(400) {
            *wet = true;
        }
        let sites = [Vec3::new(600.0, 0.0, 600.0)];
        let chosen = choose_slots(&c, 20, 1, Vec3::ZERO, &sites).unwrap();
        assert!(
            chosen
                .iter()
                .all(|s| s.position.distance(sites[0]) >= SITE_CLEARANCE)
        );
        let wet: Vec<_> = c
            .iter()
            .filter(|(_, w)| *w)
            .map(|(s, _)| s.position)
            .collect();
        assert!(chosen.iter().all(|s| !wet.contains(&s.position)));

        let err = choose_slots(&c[..3], 3, 1, Vec3::new(30.0, 0.0, 0.0), &[]).unwrap_err();
        let BotError::Placement {
            wanted,
            found,
            candidates,
            ..
        } = err
        else {
            panic!("{err:?}")
        };
        assert_eq!((wanted, found, candidates), (3, 0, 3));
        assert!(err_text(&c[..3]).contains("in water"));
    }

    fn err_text(c: &[(BotSlot, bool)]) -> String {
        choose_slots(c, 3, 1, Vec3::ZERO, &[])
            .unwrap_err()
            .to_string()
    }

    #[test]
    fn a_count_over_the_bound_is_refused() {
        let vfs = Vfs::new();
        assert!(matches!(
            plan_bots(&vfs, &grid(), MAX_CNR_BOTS + 1, 0, Vec3::ZERO, &[]),
            Err(BotError::Count(_))
        ));
    }
}
