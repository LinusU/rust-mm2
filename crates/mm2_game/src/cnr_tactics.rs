//! Objective planner for local Cops & Robbers bots (CNRBOT-B).
//!
//! A pure *what to do next* function: from a compact observable snapshot
//! ([`GoldView`] plus the participants' car positions) it says what each
//! bot should be trying to do, as a small typed [`Objective`]. It spawns
//! nothing, drives nothing and never writes gold ownership, scores or
//! positions; the caller (navigation and driving, CNRBOT-C) turns an
//! objective into steering and the authoritative [`GoldMatch`] still
//! decides every pickup, steal and delivery.
//!
//! **Provenance.** The original game's Cops & Robbers bot tactics were
//! not recovered. Everything here is a *designed enhancement* for
//! single-player, not parity with the retail AI: no coordinate is
//! hard-coded, every position comes from the snapshot's sites, gold and
//! cars.
//!
//! **Rules of thumb.**
//! - the carrier delivers to its side's marker ([`Side::delivery_target`]);
//! - loose gold is collected by the nearer half of a side's bots, the
//!   rest intercept the nearest enemy car;
//! - an enemy carrier is chased by the nearer half of a side's bots, the
//!   rest cut it off at the marker it must reach;
//! - a friendly carrier is escorted by the nearer half, the rest
//!   intercept the enemy closest to it. Nobody tries to take a
//!   teammate's gold;
//! - in free-for-all ([`Side::Solo`]) there are no teammates, so every
//!   other carrier is an enemy and every bot is its own group.
//!
//! **Determinism and hysteresis.** Rankings are by squared distance then
//! [`PlayerId`], so a fixed snapshot gives a fixed plan. The nearer-half
//! split is held for [`TacticsConfig::hold_ticks`] while the gold's
//! `revision` and the set of planned bots are unchanged, so two bots
//! swapping places do not swap jobs every frame; any pickup, drop,
//! delivery, new round, join or disconnect re-plans at once. Target
//! *positions* always follow the live snapshot, so a moving carrier is
//! tracked.
//!
//! ```
//! use bevy::prelude::*;
//! use mm2_game::cnr_tactics::{CarPose, Objective, Planner, Snapshot, TacticsConfig};
//! use mm2_game::gold::{CnrVariant, EndRule, GoldState, GoldView, Side, Sites, Standing};
//! use mm2_game::ids::PlayerId;
//!
//! let (a, b) = (PlayerId(1), PlayerId(2));
//! let view = GoldView {
//!     generation: 1,
//!     variant: CnrVariant::CopsVsRobbers,
//!     end: EndRule::Points(3),
//!     round: 1,
//!     revision: 1,
//!     elapsed: 0,
//!     state: GoldState::Resting { at: Vec3::new(10.0, 0.0, 0.0) },
//!     sites: Sites { gold: Vec3::new(10.0, 0.0, 0.0), hideout: Vec3::ZERO, bank: Vec3::X * 50.0 },
//!     outcome: None,
//!     standings: vec![
//!         Standing { player: a, side: Side::Robbers, score: 0, connected: true },
//!         Standing { player: b, side: Side::Cops, score: 0, connected: true },
//!     ],
//! };
//! let cars = [
//!     CarPose { player: a, position: Vec3::new(8.0, 0.0, 0.0) },
//!     CarPose { player: b, position: Vec3::new(30.0, 0.0, 0.0) },
//! ];
//! let mut planner = Planner::new(TacticsConfig::default());
//! let plan = planner.plan(&Snapshot { view: &view, cars: &cars, bounds: None }, &[a]);
//! assert!(matches!(plan[0].1, Objective::AcquireLooseGold { .. }));
//! ```

use std::collections::BTreeMap;

use bevy::prelude::*;

use crate::gold::{GoldState, GoldView, Side};
use crate::ids::PlayerId;

/// A participant's car as the planner sees it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CarPose {
    /// Whose car.
    pub player: PlayerId,
    /// World position.
    pub position: Vec3,
}

/// Axis-aligned play-area limits on the ground plane (x and z). Objective
/// positions are clamped into it; `y` is passed through.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    /// Smallest x and z.
    pub min: Vec2,
    /// Largest x and z.
    pub max: Vec2,
}

impl Bounds {
    fn clamp(&self, p: Vec3) -> Vec3 {
        Vec3::new(
            p.x.clamp(self.min.x.min(self.max.x), self.max.x.max(self.min.x)),
            p.y,
            p.z.clamp(self.min.y.min(self.max.y), self.max.y.max(self.min.y)),
        )
    }
}

/// Everything the planner reads. Cars missing from `cars` (not spawned,
/// wrecked, disconnected) or at a non-finite position count as absent.
#[derive(Clone, Copy, Debug)]
pub struct Snapshot<'a> {
    /// The live match state, from [`GoldMatch::view`](crate::gold::GoldMatch::view).
    pub view: &'a GoldView,
    /// Car positions of the participants that currently have a car.
    pub cars: &'a [CarPose],
    /// Optional play-area limits to clamp objective positions into.
    pub bounds: Option<Bounds>,
}

/// What a bot should be doing. Every position is derived from the
/// snapshot and re-derived on each call, so it tracks a moving target.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Objective {
    /// Gold lies loose: go and take it.
    AcquireLooseGold {
        /// Where it lies.
        at: Vec3,
    },
    /// This bot carries: take the gold to its side's marker.
    DeliverOwnGold {
        /// The marker position.
        at: Vec3,
    },
    /// An enemy carries: ram it to knock the gold loose.
    ChaseEnemyCarrier {
        /// The carrier.
        target: PlayerId,
        /// Its position now.
        at: Vec3,
    },
    /// A teammate carries: stay with it.
    SupportFriendlyCarrier {
        /// The carrier.
        target: PlayerId,
        /// Its position now.
        at: Vec3,
    },
    /// Cut off `target`, either where it is heading (the enemy carrier's
    /// delivery marker) or where it is (an enemy near the gold).
    Intercept {
        /// The car being intercepted.
        target: PlayerId,
        /// Where to be.
        at: Vec3,
    },
    /// Nothing better to do: wait at a position (the bot's own marker).
    Guard {
        /// Where.
        at: Vec3,
    },
}

impl Objective {
    /// The position the bot should head for.
    pub fn position(&self) -> Vec3 {
        match *self {
            Objective::AcquireLooseGold { at }
            | Objective::DeliverOwnGold { at }
            | Objective::ChaseEnemyCarrier { at, .. }
            | Objective::SupportFriendlyCarrier { at, .. }
            | Objective::Intercept { at, .. }
            | Objective::Guard { at } => at,
        }
    }
}

/// Planner tuning. A designed enhancement; none of it is original.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TacticsConfig {
    /// Ticks the near/far split of a side's bots is held while nothing
    /// about the gold or the roster changes.
    pub hold_ticks: u64,
}

impl Default for TacticsConfig {
    fn default() -> Self {
        // Half a second at the 60 Hz fixed step.
        Self { hold_ticks: 30 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    /// Goes for the objective itself (gold, carrier).
    Primary,
    /// Supports it from the side (cut-off, defence).
    Secondary,
}

#[derive(Debug)]
struct Held {
    revision: u64,
    generation: u64,
    since: u64,
    bots: Vec<PlayerId>,
    roles: BTreeMap<PlayerId, Role>,
}

/// Stateful planner: the only state is the held role split.
#[derive(Debug)]
pub struct Planner {
    cfg: TacticsConfig,
    held: Option<Held>,
}

impl Planner {
    /// A planner with no history.
    pub fn new(cfg: TacticsConfig) -> Self {
        Self { cfg, held: None }
    }

    /// An objective for each of `bots`, in `bots`' order. A bot with no
    /// car, a disconnected bot, an unknown bot and every bot once the
    /// match has ended get no entry. Safe to call every frame.
    pub fn plan(&mut self, snap: &Snapshot, bots: &[PlayerId]) -> Vec<(PlayerId, Objective)> {
        let view = snap.view;
        let mut active: Vec<(PlayerId, Side, Vec3)> = Vec::new();
        if view.outcome.is_none() {
            for &id in bots {
                let connected = view.standings.iter().any(|s| s.player == id && s.connected);
                let seen = active.iter().any(|a| a.0 == id);
                if let (true, false, Some(side), Some(pos)) =
                    (connected, seen, view.side_of(id), car(snap, id))
                {
                    active.push((id, side, pos));
                }
            }
        }
        let mut ids: Vec<PlayerId> = active.iter().map(|a| a.0).collect();
        ids.sort();

        let reuse = self.held.as_ref().is_some_and(|h| {
            h.revision == view.revision
                && h.generation == view.generation
                && h.bots == ids
                && view.elapsed >= h.since
                && view.elapsed - h.since < self.cfg.hold_ticks
        });
        if !reuse {
            let roles = assign_roles(snap, &active);
            self.held = Some(Held {
                revision: view.revision,
                generation: view.generation,
                since: view.elapsed,
                bots: ids,
                roles,
            });
        }
        let roles = &self.held.as_ref().map(|h| &h.roles);
        let mut out = Vec::with_capacity(active.len());
        for &id in bots {
            if let Some(&(_, side, pos)) = active.iter().find(|a| a.0 == id) {
                if out.iter().any(|(p, _)| *p == id) {
                    continue;
                }
                let role = roles
                    .and_then(|r| r.get(&id).copied())
                    .unwrap_or(Role::Primary);
                out.push((id, objective(snap, id, side, pos, role)));
            }
        }
        out
    }

    /// Forget the held split, e.g. when a match is torn down.
    pub fn reset(&mut self) {
        self.held = None;
    }
}

fn finite(p: Vec3) -> bool {
    p.is_finite()
}

fn car(snap: &Snapshot, id: PlayerId) -> Option<Vec3> {
    snap.cars
        .iter()
        .find(|c| c.player == id && finite(c.position))
        .map(|c| c.position)
}

fn site(snap: &Snapshot, side: Side) -> Vec3 {
    snap.view.sites.target(side.delivery_target())
}

fn clamp(snap: &Snapshot, p: Vec3) -> Vec3 {
    let p = if finite(p) { p } else { Vec3::ZERO };
    snap.bounds.map_or(p, |b| b.clamp(p))
}

/// Whether `other` is an enemy of `side`: in free-for-all everyone else.
fn is_enemy(view: &GoldView, side: Side, other: PlayerId) -> bool {
    match view.side_of(other) {
        Some(s) => s == Side::Solo || s != side,
        None => false,
    }
}

/// The point the nearer-half ranking is measured against, if any.
fn focus(snap: &Snapshot) -> Option<Vec3> {
    match snap.view.state {
        GoldState::Carried { by } => car(snap, by),
        GoldState::Resting { at } | GoldState::Dropped { at, .. } => finite(at).then_some(at),
    }
}

/// A side, plus the bot itself in free-for-all where nobody is a teammate.
type GroupKey = (Side, Option<PlayerId>);

/// Split each group of teammates (or a lone free-for-all bot) into the
/// nearer half, which takes the primary job, and the rest.
fn assign_roles(snap: &Snapshot, active: &[(PlayerId, Side, Vec3)]) -> BTreeMap<PlayerId, Role> {
    let mut roles = BTreeMap::new();
    let focus = focus(snap);
    let carrier = snap.view.carrier();
    let mut groups: BTreeMap<GroupKey, Vec<(f32, PlayerId)>> = BTreeMap::new();
    for &(id, side, pos) in active {
        if Some(id) == carrier {
            continue;
        }
        // Free-for-all bots have no teammates: one group each.
        let key = (side, (side == Side::Solo).then_some(id));
        let d = focus.map_or(0.0, |f| f.distance_squared(pos));
        groups.entry(key).or_default().push((d, id));
    }
    for members in groups.values_mut() {
        members.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let primaries = members.len().div_ceil(2);
        for (i, &(_, id)) in members.iter().enumerate() {
            roles.insert(
                id,
                if i < primaries {
                    Role::Primary
                } else {
                    Role::Secondary
                },
            );
        }
    }
    roles
}

/// The enemy car nearest `to`, ties by id, excluding `except`.
fn nearest_enemy(
    snap: &Snapshot,
    side: Side,
    to: Vec3,
    except: Option<PlayerId>,
) -> Option<(PlayerId, Vec3)> {
    snap.cars
        .iter()
        .filter(|c| finite(c.position) && Some(c.player) != except)
        .filter(|c| {
            snap.view
                .standings
                .iter()
                .any(|s| s.player == c.player && s.connected)
                && is_enemy(snap.view, side, c.player)
        })
        .min_by(|a, b| {
            a.position
                .distance_squared(to)
                .total_cmp(&b.position.distance_squared(to))
                .then(a.player.cmp(&b.player))
        })
        .map(|c| (c.player, c.position))
}

fn objective(snap: &Snapshot, id: PlayerId, side: Side, pos: Vec3, role: Role) -> Objective {
    let guard = Objective::Guard {
        at: clamp(snap, site(snap, side)),
    };
    let obj = match snap.view.state {
        GoldState::Carried { by } if by == id => Objective::DeliverOwnGold {
            at: clamp(snap, site(snap, side)),
        },
        GoldState::Carried { by } => {
            let Some(at) = car(snap, by) else {
                // The carrier has no car to follow: hold the marker.
                return guard;
            };
            if is_enemy(snap.view, side, by) {
                match role {
                    Role::Primary => Objective::ChaseEnemyCarrier { target: by, at },
                    Role::Secondary => {
                        let their = snap.view.side_of(by).unwrap_or(Side::Solo);
                        Objective::Intercept {
                            target: by,
                            at: site(snap, their),
                        }
                    }
                }
            } else {
                match (role, nearest_enemy(snap, side, at, Some(id))) {
                    (Role::Secondary, Some((target, at))) => Objective::Intercept { target, at },
                    _ => Objective::SupportFriendlyCarrier { target: by, at },
                }
            }
        }
        GoldState::Resting { at } | GoldState::Dropped { at, .. } if finite(at) => match role {
            Role::Primary => Objective::AcquireLooseGold { at },
            Role::Secondary => match nearest_enemy(snap, side, at, Some(id)) {
                Some((target, at)) => Objective::Intercept { target, at },
                None => Objective::AcquireLooseGold { at },
            },
        },
        _ => return guard,
    };
    let _ = pos;
    with_position(obj, clamp(snap, obj.position()))
}

fn with_position(o: Objective, at: Vec3) -> Objective {
    match o {
        Objective::AcquireLooseGold { .. } => Objective::AcquireLooseGold { at },
        Objective::DeliverOwnGold { .. } => Objective::DeliverOwnGold { at },
        Objective::ChaseEnemyCarrier { target, .. } => Objective::ChaseEnemyCarrier { target, at },
        Objective::SupportFriendlyCarrier { target, .. } => {
            Objective::SupportFriendlyCarrier { target, at }
        }
        Objective::Intercept { target, .. } => Objective::Intercept { target, at },
        Objective::Guard { .. } => Objective::Guard { at },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gold::{CnrVariant, EndReason, EndRule, Outcome, Sites, Standing, Winner};

    const HIDE: Vec3 = Vec3::new(0.0, 0.0, 0.0);
    const BANK: Vec3 = Vec3::new(500.0, 0.0, 0.0);
    const GOLD: Vec3 = Vec3::new(100.0, 0.0, 0.0);

    fn p(n: u16) -> PlayerId {
        PlayerId(n)
    }

    fn view(variant: CnrVariant, state: GoldState, roster: &[(u16, Side)]) -> GoldView {
        GoldView {
            generation: 1,
            variant,
            end: EndRule::Points(3),
            round: 1,
            revision: 1,
            elapsed: 0,
            state,
            sites: Sites {
                gold: GOLD,
                hideout: HIDE,
                bank: BANK,
            },
            outcome: None,
            standings: roster
                .iter()
                .map(|&(n, side)| Standing {
                    player: p(n),
                    side,
                    score: 0,
                    connected: true,
                })
                .collect(),
        }
    }

    fn cars(xs: &[(u16, f32)]) -> Vec<CarPose> {
        xs.iter()
            .map(|&(n, x)| CarPose {
                player: p(n),
                position: Vec3::new(x, 0.0, 0.0),
            })
            .collect()
    }

    fn plan(v: &GoldView, c: &[CarPose], bots: &[u16]) -> Vec<(PlayerId, Objective)> {
        let bots: Vec<PlayerId> = bots.iter().map(|&n| p(n)).collect();
        Planner::new(TacticsConfig::default()).plan(
            &Snapshot {
                view: v,
                cars: c,
                bounds: None,
            },
            &bots,
        )
    }

    fn kind(o: &Objective) -> &'static str {
        match o {
            Objective::AcquireLooseGold { .. } => "acquire",
            Objective::DeliverOwnGold { .. } => "deliver",
            Objective::ChaseEnemyCarrier { .. } => "chase",
            Objective::SupportFriendlyCarrier { .. } => "support",
            Objective::Intercept { .. } => "intercept",
            Objective::Guard { .. } => "guard",
        }
    }

    fn kinds(v: &GoldView, c: &[CarPose], bots: &[u16]) -> Vec<(u16, &'static str)> {
        plan(v, c, bots)
            .iter()
            .map(|(i, o)| (i.0, kind(o)))
            .collect()
    }

    #[test]
    fn carrier_delivers_to_its_side_marker() {
        let cases = [
            (CnrVariant::FreeForAll, Side::Solo, HIDE),
            (CnrVariant::CopsVsRobbers, Side::Robbers, HIDE),
            (CnrVariant::CopsVsRobbers, Side::Cops, BANK),
            (CnrVariant::RobbersVsRobbers, Side::Red, HIDE),
            (CnrVariant::RobbersVsRobbers, Side::Blue, BANK),
        ];
        for (variant, side, want) in cases {
            let v = view(
                variant,
                GoldState::Carried { by: p(1) },
                &[(1, side), (2, side)],
            );
            let out = plan(&v, &cars(&[(1, 5.0), (2, 9.0)]), &[1]);
            assert_eq!(
                out,
                vec![(p(1), Objective::DeliverOwnGold { at: want })],
                "{side:?}"
            );
        }
    }

    #[test]
    fn loose_gold_splits_a_team_between_gold_and_enemy() {
        for state in [
            GoldState::Resting { at: GOLD },
            GoldState::Dropped {
                at: GOLD,
                by: Some(p(3)),
                free_at: 0,
            },
        ] {
            let v = view(
                CnrVariant::CopsVsRobbers,
                state,
                &[(1, Side::Robbers), (2, Side::Robbers), (3, Side::Cops)],
            );
            let c = cars(&[(1, 90.0), (2, 400.0), (3, 120.0)]);
            assert_eq!(
                kinds(&v, &c, &[1, 2]),
                vec![(1, "acquire"), (2, "intercept")]
            );
        }
    }

    #[test]
    fn free_for_all_bots_all_go_for_loose_gold() {
        let v = view(
            CnrVariant::FreeForAll,
            GoldState::Resting { at: GOLD },
            &[(1, Side::Solo), (2, Side::Solo), (3, Side::Solo)],
        );
        let c = cars(&[(1, 0.0), (2, 200.0), (3, 300.0)]);
        assert_eq!(
            kinds(&v, &c, &[1, 2, 3]),
            vec![(1, "acquire"), (2, "acquire"), (3, "acquire")]
        );
    }

    #[test]
    fn enemy_carrier_is_chased_and_cut_off() {
        // Human robber 9 carries; cop bots 1 and 2 hunt it.
        let v = view(
            CnrVariant::CopsVsRobbers,
            GoldState::Carried { by: p(9) },
            &[(9, Side::Robbers), (1, Side::Cops), (2, Side::Cops)],
        );
        let c = cars(&[(9, 50.0), (1, 60.0), (2, 300.0)]);
        let out = plan(&v, &c, &[1, 2]);
        assert_eq!(
            out[0].1,
            Objective::ChaseEnemyCarrier {
                target: p(9),
                at: Vec3::new(50.0, 0.0, 0.0)
            }
        );
        // The second cop heads for the robbers' hideout.
        assert_eq!(
            out[1].1,
            Objective::Intercept {
                target: p(9),
                at: HIDE
            }
        );
    }

    #[test]
    fn free_for_all_every_carrier_is_an_enemy() {
        let v = view(
            CnrVariant::FreeForAll,
            GoldState::Carried { by: p(9) },
            &[(9, Side::Solo), (1, Side::Solo), (2, Side::Solo)],
        );
        let c = cars(&[(9, 50.0), (1, 60.0), (2, 300.0)]);
        assert_eq!(kinds(&v, &c, &[1, 2]), vec![(1, "chase"), (2, "chase")]);
    }

    #[test]
    fn friendly_carrier_is_escorted_never_chased() {
        for variant_sides in [
            (CnrVariant::CopsVsRobbers, Side::Robbers, Side::Cops),
            (CnrVariant::RobbersVsRobbers, Side::Red, Side::Blue),
        ] {
            let (variant, mine, theirs) = variant_sides;
            let v = view(
                variant,
                GoldState::Carried { by: p(9) },
                &[(9, mine), (1, mine), (2, mine), (3, theirs)],
            );
            let c = cars(&[(9, 50.0), (1, 60.0), (2, 300.0), (3, 80.0)]);
            let out = plan(&v, &c, &[9, 1, 2]);
            assert_eq!(kind(&out[0].1), "deliver");
            assert_eq!(kind(&out[1].1), "support");
            assert_eq!(
                out[2].1,
                Objective::Intercept {
                    target: p(3),
                    at: Vec3::new(80.0, 0.0, 0.0)
                }
            );
        }
    }

    #[test]
    fn friendly_carrier_with_no_enemy_leaves_everyone_escorting() {
        let v = view(
            CnrVariant::CopsVsRobbers,
            GoldState::Carried { by: p(9) },
            &[(9, Side::Cops), (1, Side::Cops), (2, Side::Cops)],
        );
        let c = cars(&[(9, 50.0), (1, 60.0), (2, 300.0)]);
        assert_eq!(kinds(&v, &c, &[1, 2]), vec![(1, "support"), (2, "support")]);
    }

    #[test]
    fn no_bot_is_idle_when_gold_is_in_play() {
        for variant in CnrVariant::ALL {
            let sides = variant.sides();
            let roster: Vec<(u16, Side)> = (1..=6)
                .map(|n| (n, sides[(n as usize) % sides.len()]))
                .collect();
            let c = cars(&(1..=6).map(|n| (n, n as f32 * 40.0)).collect::<Vec<_>>());
            for state in [
                GoldState::Resting { at: GOLD },
                GoldState::Carried { by: p(3) },
            ] {
                let v = view(variant, state, &roster);
                let out = plan(&v, &c, &[1, 2, 3, 4, 5, 6]);
                assert_eq!(out.len(), 6);
                assert!(
                    out.iter().all(|(_, o)| kind(o) != "guard"),
                    "{variant:?} {state:?}"
                );
            }
        }
    }

    #[test]
    fn missing_carrier_car_and_absent_bots_are_safe() {
        let v = view(
            CnrVariant::CopsVsRobbers,
            GoldState::Carried { by: p(9) },
            &[(9, Side::Robbers), (1, Side::Cops), (2, Side::Cops)],
        );
        // Carrier 9 has no car; bot 2 has none either.
        let c = cars(&[(1, 60.0)]);
        let out = plan(&v, &c, &[1, 2, 77]);
        assert_eq!(out, vec![(p(1), Objective::Guard { at: BANK })]);
    }

    #[test]
    fn disconnected_bot_and_ended_match_get_nothing() {
        let mut v = view(
            CnrVariant::FreeForAll,
            GoldState::Resting { at: GOLD },
            &[(1, Side::Solo), (2, Side::Solo)],
        );
        v.standings[1].connected = false;
        let c = cars(&[(1, 0.0), (2, 5.0)]);
        assert_eq!(kinds(&v, &c, &[1, 2]), vec![(1, "acquire")]);
        v.outcome = Some(Outcome {
            reason: EndReason::TimeLimit,
            winner: Winner::Tie,
            at_tick: 9,
        });
        assert!(plan(&v, &c, &[1, 2]).is_empty());
    }

    #[test]
    fn ties_break_by_player_id_whatever_the_bot_order() {
        let v = view(
            CnrVariant::CopsVsRobbers,
            GoldState::Resting { at: GOLD },
            &[(1, Side::Cops), (2, Side::Cops), (3, Side::Robbers)],
        );
        // Bots 1 and 2 are both exactly 100 m from the gold.
        let c = cars(&[(1, 0.0), (2, 200.0), (3, 500.0)]);
        let mut a = kinds(&v, &c, &[1, 2]);
        let mut b = kinds(&v, &c, &[2, 1]);
        a.sort();
        b.sort();
        assert_eq!(a, vec![(1, "acquire"), (2, "intercept")]);
        assert_eq!(a, b);
    }

    #[test]
    fn repeatable_for_a_fixed_snapshot() {
        let v = view(
            CnrVariant::RobbersVsRobbers,
            GoldState::Carried { by: p(4) },
            &[
                (1, Side::Red),
                (2, Side::Blue),
                (3, Side::Blue),
                (4, Side::Red),
                (5, Side::Blue),
            ],
        );
        let c = cars(&[(1, 10.0), (2, 70.0), (3, 20.0), (4, 33.0), (5, 90.0)]);
        let first = plan(&v, &c, &[1, 2, 3, 5]);
        for _ in 0..5 {
            assert_eq!(plan(&v, &c, &[1, 2, 3, 5]), first);
        }
    }

    #[test]
    fn split_is_held_then_replanned_on_change() {
        let mut v = view(
            CnrVariant::CopsVsRobbers,
            GoldState::Resting { at: GOLD },
            &[(1, Side::Cops), (2, Side::Cops), (3, Side::Robbers)],
        );
        let mut planner = Planner::new(TacticsConfig { hold_ticks: 30 });
        let bots = [p(1), p(2)];
        let mut run = |v: &GoldView, c: &[CarPose]| {
            planner
                .plan(
                    &Snapshot {
                        view: v,
                        cars: c,
                        bounds: None,
                    },
                    &bots,
                )
                .iter()
                .map(|(i, o)| (i.0, kind(o)))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            run(&v, &cars(&[(1, 90.0), (2, 200.0), (3, 300.0)])),
            [(1, "acquire"), (2, "intercept")]
        );
        // Bot 2 draws level and ahead: the jobs hold inside the window.
        v.elapsed = 10;
        assert_eq!(
            run(&v, &cars(&[(1, 200.0), (2, 99.0), (3, 300.0)])),
            [(1, "acquire"), (2, "intercept")]
        );
        // Window over: they swap.
        v.elapsed = 30;
        assert_eq!(
            run(&v, &cars(&[(1, 200.0), (2, 99.0), (3, 300.0)])),
            [(1, "intercept"), (2, "acquire")]
        );
        // A pickup re-plans at once, inside a fresh window.
        v.elapsed = 31;
        v.revision = 2;
        v.state = GoldState::Carried { by: p(3) };
        assert_eq!(
            run(&v, &cars(&[(1, 200.0), (2, 99.0), (3, 300.0)])),
            [(1, "chase"), (2, "intercept")]
        );
    }

    #[test]
    fn moving_carrier_is_tracked_within_a_hold() {
        let mut v = view(
            CnrVariant::FreeForAll,
            GoldState::Carried { by: p(9) },
            &[(9, Side::Solo), (1, Side::Solo)],
        );
        let mut planner = Planner::new(TacticsConfig::default());
        for (t, x) in [(0u64, 10.0f32), (5, 20.0), (10, 35.0)] {
            v.elapsed = t;
            let out = planner.plan(
                &Snapshot {
                    view: &v,
                    cars: &cars(&[(9, x), (1, 0.0)]),
                    bounds: None,
                },
                &[p(1)],
            );
            assert_eq!(out[0].1.position(), Vec3::new(x, 0.0, 0.0));
        }
    }

    #[test]
    fn positions_are_finite_and_inside_bounds() {
        let v = view(
            CnrVariant::FreeForAll,
            GoldState::Resting {
                at: Vec3::new(9000.0, 0.0, -9000.0),
            },
            &[(1, Side::Solo)],
        );
        let bounds = Some(Bounds {
            min: Vec2::new(-10.0, -10.0),
            max: Vec2::new(10.0, 10.0),
        });
        let c = cars(&[(1, 0.0)]);
        let out = Planner::new(TacticsConfig::default()).plan(
            &Snapshot {
                view: &v,
                cars: &c,
                bounds,
            },
            &[p(1)],
        );
        assert_eq!(
            out[0].1,
            Objective::AcquireLooseGold {
                at: Vec3::new(10.0, 0.0, -10.0)
            }
        );

        // Non-finite gold falls back to guarding; a non-finite car is absent.
        let mut bad = v.clone();
        bad.state = GoldState::Resting {
            at: Vec3::new(f32::NAN, 0.0, 0.0),
        };
        let out = plan(&bad, &c, &[1]);
        assert_eq!(out, vec![(p(1), Objective::Guard { at: HIDE })]);
        let nan = [CarPose {
            player: p(1),
            position: Vec3::splat(f32::INFINITY),
        }];
        assert!(plan(&v, &nan, &[1]).is_empty());
    }

    #[test]
    fn fresh_round_sends_everyone_for_the_new_gold() {
        let mut v = view(
            CnrVariant::CopsVsRobbers,
            GoldState::Carried { by: p(1) },
            &[(1, Side::Robbers), (2, Side::Robbers), (3, Side::Cops)],
        );
        let c = cars(&[(1, 0.0), (2, 90.0), (3, 300.0)]);
        let mut planner = Planner::new(TacticsConfig::default());
        let bots = [p(1), p(2)];
        let mut snap = |v: &GoldView| {
            planner.plan(
                &Snapshot {
                    view: v,
                    cars: &c,
                    bounds: None,
                },
                &bots,
            )
        };
        assert_eq!(snap(&v)[0].1, Objective::DeliverOwnGold { at: HIDE });
        v.round = 2;
        v.revision = 2;
        v.state = GoldState::Resting { at: GOLD };
        let out = snap(&v);
        // Bot 2 is nearer the gold than bot 1 is.
        assert_eq!(out[1].1, Objective::AcquireLooseGold { at: GOLD });
        assert_eq!(kind(&out[0].1), "intercept");
    }
}
