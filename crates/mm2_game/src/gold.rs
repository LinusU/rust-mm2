//! Authoritative Cops & Robbers gold state machine (F27-B.1).
//!
//! One [`GoldMatch`] is one match generation: a single gold object with
//! exactly one ownership state at a time ([`GoldState`]), the match
//! participants and their scores, and the end-of-match decision. It is
//! pure game-domain state — no Bevy world, no sockets, no clock of its
//! own — so the host can drive it from its authoritative simulation and
//! a test can drive it from a script. A client never calls it: clients
//! display what the host's [`GoldEvent`]s say (F27 requirement 5).
//!
//! **Rule provenance.** What the original is known to do is in
//! `docs/research/cnr.md` and the ledger (CNR-6…CNR-10); the numbers
//! arrive through [`GoldRules`], which `mm2_content::cnr` fills from the
//! recovered tables. Everything the original leaves unrecovered —
//! what knocks gold loose, who may recover, a dropper's lockout, the
//! disconnect and out-of-bounds outcomes, the third-site mapping in
//! Robbers vs. Robbers — is an *Enhanced policy / Implementation choice*
//! made here and marked at its definition, never presented as original.
//!
//! **Determinism.** Contested pickups are decided by the host-measured
//! distance, then by [`PlayerId`], so the answer does not depend on the
//! order requests arrived in. Site draws use a seeded [`NavRng`]. Both
//! make a match replayable from its inputs.
//!
//! **Handling load (F27-AC04).** The carrier's added mass and handling
//! scalar are *derived* from the state ([`GoldMatch::load_for`]), never
//! stored on the vehicle: whoever is the carrier has the load, nobody
//! else does, and a fresh match starts with none. A consumer reconciles
//! its per-vehicle component against that function each tick, so the
//! load is applied exactly once and cannot outlive the carrying.

use std::collections::BTreeMap;

use bevy::prelude::*;

use crate::ids::{ObjectId, PlayerId};
use crate::nav::NavRng;

/// The three variants the host chooses between, in the order the
/// executable's packed settings word numbers them (`word >> 6`,
/// `0x501510`). *Inferred* numbering: 0 scores individually and 2 builds
/// the red/blue marker pair (both read from code), so 1 is Cops vs.
/// Robbers by elimination against the three variants the help names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CnrVariant {
    /// Every player scores alone (`0x425db7` compares the player's own
    /// points against the limit).
    FreeForAll,
    /// Cops deliver to the bank, robbers to the hideout (help text;
    /// `0x42666c` picks the respawn marker by role).
    CopsVsRobbers,
    /// Two robber teams, red and blue, each with its own hideout marker
    /// (`pt_red`/`pt_blue`, `0x423e5a`); the match limit compares team
    /// totals.
    RobbersVsRobbers,
}

impl CnrVariant {
    /// Every variant, in the executable's numbering.
    pub const ALL: [CnrVariant; 3] = [
        CnrVariant::FreeForAll,
        CnrVariant::CopsVsRobbers,
        CnrVariant::RobbersVsRobbers,
    ];

    /// Whether the variant is scored by team totals rather than by
    /// individual points (*verified_original*: the limit check at
    /// `0x425dbc` branches on `variant != 0`).
    pub fn team_scored(self) -> bool {
        !matches!(self, CnrVariant::FreeForAll)
    }

    /// The sides a participant may take in this variant, first side
    /// first. [`Side::Solo`] for free-for-all; cops/robbers and
    /// red/blue otherwise.
    pub fn sides(self) -> &'static [Side] {
        match self {
            CnrVariant::FreeForAll => &[Side::Solo],
            CnrVariant::CopsVsRobbers => &[Side::Robbers, Side::Cops],
            CnrVariant::RobbersVsRobbers => &[Side::Red, Side::Blue],
        }
    }
}

/// Which side a participant plays. Which sides exist is decided by the
/// [`CnrVariant`]; the original's team labels are `COPS`/`ROBBERS` and
/// `RED`/`BLUE` (`mmlang.dll` 0x7a–0x81, 0x108–0x10b).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Side {
    /// Free-for-all: no team.
    Solo,
    /// Robbers in Cops vs. Robbers.
    Robbers,
    /// Cops in Cops vs. Robbers.
    Cops,
    /// First robber team in Robbers vs. Robbers.
    Red,
    /// Second robber team in Robbers vs. Robbers.
    Blue,
}

/// Which marker a side delivers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryTarget {
    /// The hideout marker (`pt_hideout`).
    Hideout,
    /// The bank marker (`pt_bank`).
    Bank,
}

impl Side {
    /// The marker this side delivers to. *Documented* for free-for-all
    /// (own hideout), robbers (hideout) and cops (bank). For Robbers vs.
    /// Robbers the original builds a `pt_red`/`pt_blue` pair; which of
    /// the three drawn positions each occupies was not recovered, so red
    /// takes the hideout draw and blue the bank draw — an
    /// *Implementation choice* that keeps the two teams' targets
    /// distinct and drawn from the same pool.
    pub fn delivery_target(self) -> DeliveryTarget {
        match self {
            Side::Solo | Side::Robbers | Side::Red => DeliveryTarget::Hideout,
            Side::Cops | Side::Blue => DeliveryTarget::Bank,
        }
    }
}

/// The side to give the next player: the variant's smaller side, first
/// side on a tie. An *Implementation choice* (the original's team
/// assignment messages, `0x25c`/`0x25d`, were not decoded); the host may
/// still honour a lobby pick instead.
pub fn balanced_side(variant: CnrVariant, current: impl IntoIterator<Item = Side>) -> Side {
    let sides = variant.sides();
    let mut counts = vec![0usize; sides.len()];
    for s in current {
        if let Some(i) = sides.iter().position(|&x| x == s) {
            counts[i] += 1;
        }
    }
    // `min_by_key` keeps the first of equal minima: the first side wins ties.
    sides
        .iter()
        .zip(counts)
        .min_by_key(|&(_, n)| n)
        .map_or(Side::Solo, |(&s, _)| s)
}

/// How a match ends. The host's limit choice, in ticks/points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndRule {
    /// Runs until the host ends it.
    None,
    /// Ends once this many ticks have elapsed.
    Ticks(u64),
    /// Ends when a player (free-for-all) or team (team variants)
    /// reaches this many points.
    Points(u32),
}

/// Mass and handling the carrier takes on, derived from the host's gold
/// mass option.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CarrierLoad {
    /// Mass added to the carrier's body, kilograms. By the *documented*
    /// reading of the option labels (¼ / ½ ton); the executable's own
    /// unit is not established.
    pub added_mass_kg: f32,
    /// Handling scalar for the carrier, 1.0 = unchanged. The original
    /// writes 1.0 / 0.9 / 0.81 for the local car; what it scales is
    /// unidentified, so a consumer applying it is *provisional*.
    pub handling_scalar: f32,
}

impl CarrierLoad {
    /// No load: the car is unchanged.
    pub const NONE: CarrierLoad = CarrierLoad {
        added_mass_kg: 0.0,
        handling_scalar: 1.0,
    };
}

/// Every number a match's rules need. `mm2_content::cnr` builds the
/// stock set from the recovered tables; tests and mods may vary it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GoldRules {
    /// Variant being played.
    pub variant: CnrVariant,
    /// How the match ends.
    pub end: EndRule,
    /// What the carrier takes on.
    pub load: CarrierLoad,
    /// Points awarded for taking the gold (see `PICKUP_POINTS`).
    pub pickup_points: u32,
    /// Points awarded for a delivery.
    pub delivery_points: u32,
    /// Distance within which a car takes the gold, metres.
    pub pickup_radius: f32,
    /// Distance from the target marker within which a carrier
    /// delivers, metres.
    pub delivery_radius: f32,
    /// Ticks the car that just lost the gold cannot take it back, so a
    /// ram that knocks it loose is not undone by the victim still
    /// sitting on top of it. *Enhanced policy*; the original's
    /// behaviour is unrecovered.
    pub drop_lockout_ticks: u64,
}

/// The three positions a round is played between, drawn from the pool.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sites {
    /// Where the gold lies at the start of a round.
    pub gold: Vec3,
    /// The hideout marker.
    pub hideout: Vec3,
    /// The bank marker.
    pub bank: Vec3,
}

impl Sites {
    /// Position of the marker a side delivers to.
    pub fn target(&self, t: DeliveryTarget) -> Vec3 {
        match t {
            DeliveryTarget::Hideout => self.hideout,
            DeliveryTarget::Bank => self.bank,
        }
    }
}

/// The gold's single ownership state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GoldState {
    /// Lying at its drawn site, not yet taken this round.
    Resting {
        /// Where it lies.
        at: Vec3,
    },
    /// In a car.
    Carried {
        /// The carrier.
        by: PlayerId,
    },
    /// Lying where a carrier lost it.
    Dropped {
        /// Where it lies.
        at: Vec3,
        /// Who lost it, if a participant did.
        by: Option<PlayerId>,
        /// First tick at which `by` may take it back.
        free_at: u64,
    },
}

/// Why a carrier lost the gold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropCause {
    /// Knocked loose by another car's impact (what threshold counts is
    /// the caller's policy; the original's is unrecovered).
    Knocked {
        /// The car that struck the carrier, if one is known.
        by: Option<PlayerId>,
    },
    /// The carrier's car was destroyed or disabled.
    Destroyed,
    /// The carrier left the match.
    Disconnected,
}

/// Why the gold was lost without a drop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LossCause {
    /// It lay outside the playable bounds, so the host re-placed it.
    OutOfBounds,
}

/// A pickup request as the host assembles it: the player and the round
/// the client believed it was in (from the client's message) with the
/// position the *host's* simulation holds for that player's car. A
/// client's claimed position is never an input.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Contact {
    /// Who wants the gold.
    pub player: PlayerId,
    /// The round the request was made against.
    pub round: u32,
    /// The host's position for the player's car.
    pub position: Vec3,
}

/// What became of one pickup request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickupVerdict {
    /// The player now carries the gold.
    Granted {
        /// Whether it was dropped gold (`RECOVERLOOT` in the original's
        /// vocabulary) rather than fresh gold (`GETLOOT`).
        recovered: bool,
        /// Points awarded.
        points: u32,
    },
    /// Someone else is carrying it.
    AlreadyCarried {
        /// The carrier.
        carrier: PlayerId,
    },
    /// Another request in the same resolution won.
    Contested {
        /// The winner.
        winner: PlayerId,
    },
    /// The same player asked twice in one resolution; the first counts.
    Duplicate,
    /// Not a participant, or no longer connected.
    NotParticipant,
    /// The request was made against an earlier round of the gold.
    Stale,
    /// Farther than the pickup radius, or a non-finite position.
    OutOfRange,
    /// The player just lost this gold and may not retake it yet.
    Locked,
    /// The match has ended.
    MatchOver,
}

/// What became of a delivery attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryVerdict {
    /// Scored; the gold is re-placed for the next round.
    Delivered {
        /// Points awarded.
        points: u32,
        /// The round now in play.
        round: u32,
    },
    /// The player is not carrying the gold.
    NotCarrier,
    /// The request was made against an earlier round of the gold.
    Stale,
    /// Not within the delivery radius of the side's marker.
    OutOfRange,
    /// The match has ended.
    MatchOver,
}

/// Why a match refused an operation that has no per-request verdict.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GoldError {
    /// The match has ended.
    MatchOver,
    /// The named player is not a participant.
    UnknownPlayer(PlayerId),
    /// The player is not the carrier.
    NotCarrier(PlayerId),
    /// The gold is being carried, so it cannot be re-placed.
    Carried,
    /// The player id is already in the match.
    DuplicatePlayer(PlayerId),
    /// The side is not one of the variant's.
    WrongSide(PlayerId, Side),
    /// The gold's [`ObjectId`] was minted by another generation.
    ForeignGold,
    /// The pool has fewer than the three sites a round needs.
    PoolTooSmall(usize),
    /// A pool position is not finite.
    NonFiniteSite(usize),
    /// A rules field is not usable (named).
    BadRules(&'static str),
}

/// Why a match ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndReason {
    /// A player or team reached the point limit.
    PointLimit,
    /// The time limit elapsed.
    TimeLimit,
}

/// Who won.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Winner {
    /// The free-for-all winner.
    Player(PlayerId),
    /// The winning team.
    Side(Side),
    /// The top scores were level (or nobody played).
    Tie,
}

/// The decided end of a match.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// Why it ended.
    pub reason: EndReason,
    /// Who won.
    pub winner: Winner,
    /// Tick the match ended on.
    pub at_tick: u64,
}

/// A participant's row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Standing {
    /// Who.
    pub player: PlayerId,
    /// Which side.
    pub side: Side,
    /// Individual points.
    pub score: u32,
    /// Whether still in the match. A leaver's points stay on the board
    /// (and in a team's total).
    pub connected: bool,
}

/// What happened, for the host to broadcast, the HUD to show and audio
/// to announce. The comment names the original's commentary family
/// where one exists (`COMMENTARY_CUES` in `mm2_content::cnr`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GoldEvent {
    /// A car took the gold (`GETLOOT`, or `RECOVERLOOT` when
    /// `recovered`).
    Picked {
        /// The new carrier.
        player: PlayerId,
        /// Their side.
        side: Side,
        /// Whether it had been dropped.
        recovered: bool,
        /// Points awarded.
        points: u32,
        /// The round.
        round: u32,
    },
    /// The carrier lost it (`DROPLOOT`).
    Dropped {
        /// Who lost it.
        player: PlayerId,
        /// Where it lies.
        at: Vec3,
        /// Why.
        cause: DropCause,
        /// The round.
        round: u32,
    },
    /// A carrier delivered (`STASHLOOT`).
    Delivered {
        /// Who delivered.
        player: PlayerId,
        /// Their side.
        side: Side,
        /// Points awarded.
        points: u32,
        /// The round that was completed.
        round: u32,
    },
    /// New sites were drawn and the gold re-placed; the round advanced.
    SitesDrawn {
        /// The new sites.
        sites: Sites,
        /// The round now in play.
        round: u32,
    },
    /// The gold was lost without a carrier and re-placed.
    Lost {
        /// Why.
        cause: LossCause,
        /// The gold's new site.
        gold: Vec3,
        /// The round now in play.
        round: u32,
    },
    /// A player joined.
    Joined {
        /// Who.
        player: PlayerId,
        /// Their side.
        side: Side,
    },
    /// A player left.
    Left {
        /// Who.
        player: PlayerId,
    },
    /// The match ended.
    Ended(Outcome),
}

/// What the announcer calls out — the four situations the original's
/// commentary tables distinguish per role (`GETLOOT`, `DROPLOOT`,
/// `STASHLOOT`, `RECOVERLOOT`; `mm2_content::cnr::COMMENTARY_CUES`).
/// *Which* in-game moment triggers each is unrecovered (ledger CNR-13);
/// this is the designed mapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallKind {
    /// A car took the gold off its site.
    Get,
    /// The carrier lost it.
    Drop,
    /// A carrier delivered it.
    Stash,
    /// A car took gold that had been dropped.
    Recover,
}

/// One announcement: what happened, and to which side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Call {
    /// What happened.
    pub kind: CallKind,
    /// The side it happened to — the carrier's.
    pub side: Side,
}

impl GoldEvent {
    /// The announcement this event earns, if any. `side_of` names a
    /// participant's side for the events that carry only a player
    /// (a drop). Round changes, a re-placed gold, roster changes and
    /// the match end are not announced.
    pub fn call(&self, side_of: impl Fn(PlayerId) -> Option<Side>) -> Option<Call> {
        match *self {
            GoldEvent::Picked {
                side, recovered, ..
            } => Some(Call {
                kind: if recovered {
                    CallKind::Recover
                } else {
                    CallKind::Get
                },
                side,
            }),
            GoldEvent::Dropped { player, .. } => Some(Call {
                kind: CallKind::Drop,
                side: side_of(player)?,
            }),
            GoldEvent::Delivered { side, .. } => Some(Call {
                kind: CallKind::Stash,
                side,
            }),
            GoldEvent::SitesDrawn { .. }
            | GoldEvent::Lost { .. }
            | GoldEvent::Joined { .. }
            | GoldEvent::Left { .. }
            | GoldEvent::Ended(_) => None,
        }
    }
}

/// A copy of a match's observable state: the host builds one from its
/// [`GoldMatch`] and a client rebuilds one from the wire. State, not an
/// event — a replica holds the newest by `(revision, elapsed)` and a
/// dropped frame costs nothing the next does not repair.
#[derive(Clone, Debug, PartialEq)]
pub struct GoldView {
    /// Session generation the match belongs to.
    pub generation: u64,
    /// Variant being played.
    pub variant: CnrVariant,
    /// How the match ends.
    pub end: EndRule,
    /// Round in play.
    pub round: u32,
    /// State-change counter ([`GoldMatch::revision`]).
    pub revision: u64,
    /// Match clock, ticks.
    pub elapsed: u64,
    /// The gold's single ownership state.
    pub state: GoldState,
    /// The round's three sites.
    pub sites: Sites,
    /// The decided outcome, once the match has ended.
    pub outcome: Option<Outcome>,
    /// Everyone, best first.
    pub standings: Vec<Standing>,
}

impl GoldView {
    /// The order a replica ranks frames by: a newer revision, and
    /// within one revision a later clock.
    pub fn freshness(&self) -> (u64, u64) {
        (self.revision, self.elapsed)
    }

    /// The current carrier, if any.
    pub fn carrier(&self) -> Option<PlayerId> {
        match self.state {
            GoldState::Carried { by } => Some(by),
            _ => None,
        }
    }

    /// A participant's side, from the standings.
    pub fn side_of(&self, player: PlayerId) -> Option<Side> {
        self.standings
            .iter()
            .find(|s| s.player == player)
            .map(|s| s.side)
    }

    /// The announcement the change from `prev` to `self` earns — what a
    /// client, which receives state rather than events, derives so it
    /// can voice the same commentary the authority's events earn. Only
    /// what the two views show: a gold taken (recovered when it lay
    /// dropped, which a hand-over inside one frame also reads as), a
    /// carrier's drop, or a stash (the round advanced with the carrier
    /// gone). `None` across a generation change, a round that went
    /// backwards, a repeated or older frame, and when nothing visible
    /// changed — so a first frame after a late join, or a lost frame,
    /// is silent rather than a stale line.
    pub fn call_since(&self, prev: &GoldView) -> Option<Call> {
        if self.generation != prev.generation
            || self.round < prev.round
            || self.freshness() <= prev.freshness()
        {
            return None;
        }
        match (prev.state, self.state) {
            (GoldState::Carried { by }, GoldState::Carried { by: now }) if by == now => None,
            (GoldState::Carried { .. }, GoldState::Carried { by: now }) => Some(Call {
                kind: CallKind::Recover,
                side: self.side_of(now)?,
            }),
            (before, GoldState::Carried { by }) => Some(Call {
                kind: if matches!(before, GoldState::Dropped { .. }) {
                    CallKind::Recover
                } else {
                    CallKind::Get
                },
                side: self.side_of(by)?,
            }),
            (GoldState::Carried { by }, GoldState::Dropped { .. }) => Some(Call {
                kind: CallKind::Drop,
                side: self.side_of(by).or_else(|| prev.side_of(by))?,
            }),
            (GoldState::Carried { by }, GoldState::Resting { .. }) if self.round > prev.round => {
                Some(Call {
                    kind: CallKind::Stash,
                    side: self.side_of(by).or_else(|| prev.side_of(by))?,
                })
            }
            _ => None,
        }
    }

    /// Where the gold lies, when nobody carries it.
    pub fn gold_position(&self) -> Option<Vec3> {
        match self.state {
            GoldState::Resting { at } | GoldState::Dropped { at, .. } => Some(at),
            GoldState::Carried { .. } => None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Member {
    side: Side,
    score: u32,
    connected: bool,
}

/// One Cops & Robbers match: the authoritative gold state.
#[derive(Debug)]
pub struct GoldMatch {
    generation: u64,
    gold: ObjectId,
    rules: GoldRules,
    pool: Vec<Vec3>,
    rng: NavRng,
    /// Pool indices of the current gold/hideout/bank sites.
    site_idx: [usize; 3],
    members: BTreeMap<PlayerId, Member>,
    state: GoldState,
    round: u32,
    elapsed: u64,
    revision: u64,
    outcome: Option<Outcome>,
    events: Vec<GoldEvent>,
}

impl GoldMatch {
    /// Start a match. `generation` is the session generation the gold's
    /// `gold` id was minted in; `pool` is the authored site pool
    /// (`multicopwaypoints.csv`), at least three finite positions;
    /// `seed` drives every site draw. The gold starts resting at its
    /// first drawn site and the clock at zero.
    pub fn new(
        generation: u64,
        gold: ObjectId,
        rules: GoldRules,
        pool: Vec<Vec3>,
        seed: u64,
        participants: &[(PlayerId, Side)],
    ) -> Result<Self, GoldError> {
        if gold.generation != generation {
            return Err(GoldError::ForeignGold);
        }
        if !(rules.pickup_radius.is_finite() && rules.pickup_radius > 0.0) {
            return Err(GoldError::BadRules("pickup_radius"));
        }
        if !(rules.delivery_radius.is_finite() && rules.delivery_radius > 0.0) {
            return Err(GoldError::BadRules("delivery_radius"));
        }
        if !(rules.load.added_mass_kg.is_finite() && rules.load.handling_scalar.is_finite()) {
            return Err(GoldError::BadRules("load"));
        }
        if pool.len() < 3 {
            return Err(GoldError::PoolTooSmall(pool.len()));
        }
        if let Some(i) = pool.iter().position(|p| !p.is_finite()) {
            return Err(GoldError::NonFiniteSite(i));
        }
        let mut members = BTreeMap::new();
        for &(player, side) in participants {
            if !rules.variant.sides().contains(&side) {
                return Err(GoldError::WrongSide(player, side));
            }
            let fresh = members
                .insert(
                    player,
                    Member {
                        side,
                        score: 0,
                        connected: true,
                    },
                )
                .is_none();
            if !fresh {
                return Err(GoldError::DuplicatePlayer(player));
            }
        }
        let mut rng = NavRng::new(seed);
        let site_idx = draw_three(&mut rng, pool.len());
        let state = GoldState::Resting {
            at: pool[site_idx[0]],
        };
        Ok(Self {
            generation,
            gold,
            rules,
            pool,
            rng,
            site_idx,
            members,
            state,
            round: 0,
            elapsed: 0,
            revision: 0,
            outcome: None,
            events: Vec::new(),
        })
    }

    /// Session generation this match belongs to.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The gold object's stable id.
    pub fn gold_id(&self) -> ObjectId {
        self.gold
    }

    /// The rules in force.
    pub fn rules(&self) -> &GoldRules {
        &self.rules
    }

    /// The gold's single ownership state.
    pub fn state(&self) -> GoldState {
        self.state
    }

    /// Round of the gold: starts at 0 and advances on every delivery
    /// and every re-placement, so a request made against an earlier
    /// round is recognisably stale.
    pub fn round(&self) -> u32 {
        self.round
    }

    /// Count of state changes so far; a replica can drop any state
    /// frame whose revision is not newer than the one it holds.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Ticks elapsed on the match clock.
    pub fn elapsed_ticks(&self) -> u64 {
        self.elapsed
    }

    /// The decided outcome, once the match has ended.
    pub fn outcome(&self) -> Option<Outcome> {
        self.outcome
    }

    /// The three sites `seed`'s opening draw takes from `pool` —
    /// gold, hideout, bank — without keeping a match around. The same
    /// draw [`GoldMatch::new`] makes, so an audit can name the round a
    /// seed starts. `None` for a pool that cannot seed a round.
    pub fn opening_sites(rules: GoldRules, pool: &[[f32; 3]], seed: u64) -> Option<[[f32; 3]; 3]> {
        let id = ObjectId {
            generation: 1,
            slot: 1,
        };
        let pool = pool.iter().map(|p| Vec3::from(*p)).collect();
        let sites = Self::new(1, id, rules, pool, seed, &[]).ok()?.sites();
        Some([sites.gold, sites.hideout, sites.bank].map(|p| p.to_array()))
    }

    /// The current sites.
    pub fn sites(&self) -> Sites {
        Sites {
            gold: self.pool[self.site_idx[0]],
            hideout: self.pool[self.site_idx[1]],
            bank: self.pool[self.site_idx[2]],
        }
    }

    /// The current carrier, if any.
    pub fn carrier(&self) -> Option<PlayerId> {
        match self.state {
            GoldState::Carried { by } => Some(by),
            _ => None,
        }
    }

    /// Where the gold lies, when nobody carries it.
    pub fn gold_position(&self) -> Option<Vec3> {
        match self.state {
            GoldState::Resting { at } | GoldState::Dropped { at, .. } => Some(at),
            GoldState::Carried { .. } => None,
        }
    }

    /// What `player` carries right now: the rules' load for the carrier
    /// and nothing for anyone else. Derived from the state each call,
    /// which is what makes the load apply once and end with the
    /// carrying (F27-AC04).
    pub fn load_for(&self, player: PlayerId) -> Option<CarrierLoad> {
        (self.carrier() == Some(player)).then_some(self.rules.load)
    }

    /// The side a participant plays.
    pub fn side_of(&self, player: PlayerId) -> Option<Side> {
        self.members.get(&player).map(|m| m.side)
    }

    /// A participant's individual points.
    pub fn score(&self, player: PlayerId) -> Option<u32> {
        self.members.get(&player).map(|m| m.score)
    }

    /// A side's total: the sum of its members' points, leavers
    /// included.
    pub fn side_total(&self, side: Side) -> u32 {
        self.members
            .values()
            .filter(|m| m.side == side)
            .fold(0u32, |a, m| a.saturating_add(m.score))
    }

    /// Everyone, best first (ties by player id) — the results table.
    pub fn standings(&self) -> Vec<Standing> {
        let mut v: Vec<Standing> = self
            .members
            .iter()
            .map(|(&player, m)| Standing {
                player,
                side: m.side,
                score: m.score,
                connected: m.connected,
            })
            .collect();
        v.sort_by(|a, b| b.score.cmp(&a.score).then(a.player.cmp(&b.player)));
        v
    }

    /// Everything a replica of this match shows, copied out — what the
    /// host puts on the wire (F27-B.3) and a client's HUD and markers
    /// read back. The rules' numbers are not in it beyond the variant
    /// and the end rule: those the replica needs to label a score and a
    /// clock, while the radii and load stay the host's to enforce.
    pub fn view(&self) -> GoldView {
        GoldView {
            generation: self.generation,
            variant: self.rules.variant,
            end: self.rules.end,
            round: self.round,
            revision: self.revision,
            elapsed: self.elapsed,
            state: self.state,
            sites: self.sites(),
            outcome: self.outcome,
            standings: self.standings(),
        }
    }

    /// Take the events produced since the last call, oldest first.
    pub fn drain_events(&mut self) -> Vec<GoldEvent> {
        std::mem::take(&mut self.events)
    }

    fn push(&mut self, e: GoldEvent) {
        self.revision += 1;
        self.events.push(e);
    }

    /// Advance the match clock one tick; ends the match when a time
    /// limit is reached. A no-op once the match has ended.
    pub fn tick(&mut self) {
        if self.outcome.is_some() {
            return;
        }
        self.elapsed += 1;
        if let EndRule::Ticks(limit) = self.rules.end
            && self.elapsed >= limit
        {
            self.end(EndReason::TimeLimit);
        }
    }

    /// The side the next participant should take: the variant's side
    /// with the fewest connected members, ties to the first side
    /// listed ([`CnrVariant::sides`]). *Designed* — the original lets a
    /// player pick a team in its lobby, which this slice does not model
    /// (ledger CNR-12) — so a roster fills the sides alternately and
    /// every process that applies it to the same arrival order agrees.
    pub fn balanced_side(&self) -> Side {
        let sides = self.rules.variant.sides();
        let count = |side: Side| {
            self.members
                .values()
                .filter(|m| m.connected && m.side == side)
                .count()
        };
        // `min_by_key` returns the first minimum, which is the tie rule.
        sides
            .iter()
            .copied()
            .min_by_key(|&s| count(s))
            .unwrap_or(Side::Solo)
    }

    /// Add a participant mid-match (late join). They start at zero
    /// points; the gold's state is unaffected.
    pub fn join(&mut self, player: PlayerId, side: Side) -> Result<(), GoldError> {
        if self.outcome.is_some() {
            return Err(GoldError::MatchOver);
        }
        if !self.rules.variant.sides().contains(&side) {
            return Err(GoldError::WrongSide(player, side));
        }
        if self.members.contains_key(&player) {
            return Err(GoldError::DuplicatePlayer(player));
        }
        self.members.insert(
            player,
            Member {
                side,
                score: 0,
                connected: true,
            },
        );
        self.push(GoldEvent::Joined { player, side });
        Ok(())
    }

    /// A participant who left is back (their car spawned again — a remote
    /// pick change respawns it). They resume the side and the points they
    /// left with, so leaving and returning never resets a score or moves
    /// anyone to the other team. *Implementation choice*: the original's
    /// reconnect handling is unrecovered. Refused once the match is over,
    /// for someone who never joined, and for someone still connected.
    pub fn rejoin(&mut self, player: PlayerId) -> Result<Side, GoldError> {
        if self.outcome.is_some() {
            return Err(GoldError::MatchOver);
        }
        let Some(m) = self.members.get_mut(&player) else {
            return Err(GoldError::UnknownPlayer(player));
        };
        if m.connected {
            return Err(GoldError::DuplicatePlayer(player));
        }
        m.connected = true;
        let side = m.side;
        self.push(GoldEvent::Joined { player, side });
        Ok(side)
    }

    /// A participant left. Their points stay on the board; if they were
    /// carrying, the gold drops where `position` (the host's last
    /// position for their car) says — *Enhanced policy*, since the
    /// original's handling at that moment is unrecovered. A non-finite
    /// `position` falls back to the round's gold site so the gold is
    /// never lost.
    pub fn leave(&mut self, player: PlayerId, position: Vec3) -> Result<(), GoldError> {
        let Some(m) = self.members.get_mut(&player) else {
            return Err(GoldError::UnknownPlayer(player));
        };
        if !m.connected {
            return Err(GoldError::UnknownPlayer(player));
        }
        m.connected = false;
        self.push(GoldEvent::Left { player });
        if self.carrier() == Some(player) && self.outcome.is_none() {
            let at = if position.is_finite() {
                position
            } else {
                self.sites().gold
            };
            self.drop_gold(player, at, DropCause::Disconnected);
        }
        Ok(())
    }

    /// The carrier lost the gold — rammed loose, or its car was
    /// destroyed. The host decides *that* it happened (impact threshold,
    /// damage state); the gold lands at `at`. The dropper cannot retake
    /// it for [`GoldRules::drop_lockout_ticks`].
    pub fn dislodge(
        &mut self,
        carrier: PlayerId,
        at: Vec3,
        cause: DropCause,
    ) -> Result<(), GoldError> {
        if self.outcome.is_some() {
            return Err(GoldError::MatchOver);
        }
        if self.carrier() != Some(carrier) {
            return Err(GoldError::NotCarrier(carrier));
        }
        let at = if at.is_finite() {
            at
        } else {
            self.sites().gold
        };
        self.drop_gold(carrier, at, cause);
        Ok(())
    }

    fn drop_gold(&mut self, player: PlayerId, at: Vec3, cause: DropCause) {
        let locks = !matches!(cause, DropCause::Disconnected);
        self.state = GoldState::Dropped {
            at,
            by: locks.then_some(player),
            free_at: self.elapsed.saturating_add(self.rules.drop_lockout_ticks),
        };
        self.push(GoldEvent::Dropped {
            player,
            at,
            cause,
            round: self.round,
        });
    }

    /// Decide a batch of pickup requests made in the same tick.
    ///
    /// Exactly one request can win: among those eligible (a connected
    /// participant, current round, within the pickup radius, not locked
    /// out) the nearest car takes the gold, equal distances going to the
    /// lower [`PlayerId`] — so the result is the same whatever order the
    /// requests arrived in. The rest are [`Contested`](PickupVerdict::Contested).
    /// Verdicts come back in the order of `contacts`.
    pub fn resolve_pickups(&mut self, contacts: &[Contact]) -> Vec<PickupVerdict> {
        if self.outcome.is_some() {
            return vec![PickupVerdict::MatchOver; contacts.len()];
        }
        if let GoldState::Carried { by } = self.state {
            return vec![PickupVerdict::AlreadyCarried { carrier: by }; contacts.len()];
        }
        let gold_at = self.gold_position().unwrap_or(Vec3::ZERO);
        let mut verdicts: Vec<Option<PickupVerdict>> = vec![None; contacts.len()];
        // (distance, player, index) of each eligible request.
        let mut eligible: Vec<(f32, PlayerId, usize)> = Vec::new();
        for (i, c) in contacts.iter().enumerate() {
            let connected = self.members.get(&c.player).is_some_and(|m| m.connected);
            let d = c.position.distance(gold_at);
            verdicts[i] = if !connected {
                Some(PickupVerdict::NotParticipant)
            } else if c.round != self.round {
                Some(PickupVerdict::Stale)
            } else if !(d.is_finite() && d <= self.rules.pickup_radius) {
                Some(PickupVerdict::OutOfRange)
            } else if matches!(
                self.state,
                GoldState::Dropped { by: Some(b), free_at, .. }
                    if b == c.player && self.elapsed < free_at
            ) {
                Some(PickupVerdict::Locked)
            } else {
                eligible.push((d, c.player, i));
                None
            };
        }
        let Some(&(_, winner, win_idx)) = eligible
            .iter()
            .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)))
        else {
            return verdicts.into_iter().map(|v| v.expect("set")).collect();
        };
        for &(_, player, i) in &eligible {
            verdicts[i] = Some(if i == win_idx {
                self.grant(winner)
            } else if player == winner {
                PickupVerdict::Duplicate
            } else {
                PickupVerdict::Contested { winner }
            });
        }
        verdicts.into_iter().map(|v| v.expect("set")).collect()
    }

    fn grant(&mut self, player: PlayerId) -> PickupVerdict {
        let recovered = matches!(self.state, GoldState::Dropped { .. });
        self.state = GoldState::Carried { by: player };
        let points = self.rules.pickup_points;
        let side = self.award(player, points);
        self.push(GoldEvent::Picked {
            player,
            side,
            recovered,
            points,
            round: self.round,
        });
        self.check_point_limit();
        PickupVerdict::Granted { recovered, points }
    }

    /// A carrier tries to deliver. `position` is the host's position
    /// for their car; the side's marker is `rules.variant`'s target for
    /// it. On success the carrier is cleared, the delivery points are
    /// awarded once, new sites are drawn and the round advances.
    pub fn deliver(&mut self, player: PlayerId, round: u32, position: Vec3) -> DeliveryVerdict {
        if self.outcome.is_some() {
            return DeliveryVerdict::MatchOver;
        }
        if self.carrier() != Some(player) {
            return DeliveryVerdict::NotCarrier;
        }
        if round != self.round {
            return DeliveryVerdict::Stale;
        }
        let side = self.members[&player].side;
        let target = self.sites().target(side.delivery_target());
        let d = position.distance(target);
        if !(d.is_finite() && d <= self.rules.delivery_radius) {
            return DeliveryVerdict::OutOfRange;
        }
        let points = self.rules.delivery_points;
        self.award(player, points);
        let done = self.round;
        self.push(GoldEvent::Delivered {
            player,
            side,
            points,
            round: done,
        });
        self.round += 1;
        self.site_idx = draw_three(&mut self.rng, self.pool.len());
        let sites = self.sites();
        self.state = GoldState::Resting { at: sites.gold };
        self.push(GoldEvent::SitesDrawn {
            sites,
            round: self.round,
        });
        self.check_point_limit();
        DeliveryVerdict::Delivered {
            points,
            round: self.round,
        }
    }

    /// The gold lies outside the playable bounds (the host's check):
    /// re-place it at a fresh pool site, no score, no carrier, round
    /// advanced so a request against the lost position is stale. Not
    /// allowed while carried — a carrier out of bounds is the car's
    /// recovery problem, and the gold goes with it. *Enhanced policy*
    /// (original unrecovered): the objective is never lost for good.
    pub fn gold_out_of_bounds(&mut self) -> Result<(), GoldError> {
        if self.outcome.is_some() {
            return Err(GoldError::MatchOver);
        }
        if self.carrier().is_some() {
            return Err(GoldError::Carried);
        }
        // New gold site distinct from the standing hideout and bank.
        let keep = [self.site_idx[1], self.site_idx[2]];
        let mut i = (self.rng.next_u64() % self.pool.len() as u64) as usize;
        while keep.contains(&i) {
            i = (i + 1) % self.pool.len();
        }
        self.site_idx[0] = i;
        self.round += 1;
        let gold = self.pool[i];
        self.state = GoldState::Resting { at: gold };
        self.push(GoldEvent::Lost {
            cause: LossCause::OutOfBounds,
            gold,
            round: self.round,
        });
        Ok(())
    }

    fn award(&mut self, player: PlayerId, points: u32) -> Side {
        let m = self.members.get_mut(&player).expect("participant");
        m.score = m.score.saturating_add(points);
        m.side
    }

    fn check_point_limit(&mut self) {
        let EndRule::Points(limit) = self.rules.end else {
            return;
        };
        if self.outcome.is_some() {
            return;
        }
        let reached = if self.rules.variant.team_scored() {
            self.rules
                .variant
                .sides()
                .iter()
                .any(|&s| self.side_total(s) >= limit)
        } else {
            self.members.values().any(|m| m.score >= limit)
        };
        if reached {
            self.end(EndReason::PointLimit);
        }
    }

    fn end(&mut self, reason: EndReason) {
        let winner = self.decide_winner();
        let outcome = Outcome {
            reason,
            winner,
            at_tick: self.elapsed,
        };
        self.outcome = Some(outcome);
        self.push(GoldEvent::Ended(outcome));
    }

    fn decide_winner(&self) -> Winner {
        if self.rules.variant.team_scored() {
            let mut best: Option<(u32, Side)> = None;
            let mut tied = false;
            for &s in self.rules.variant.sides() {
                let t = self.side_total(s);
                match best {
                    Some((b, _)) if t == b => tied = true,
                    Some((b, _)) if t < b => {}
                    _ => {
                        best = Some((t, s));
                        tied = false;
                    }
                }
            }
            match best {
                Some((_, s)) if !tied => Winner::Side(s),
                _ => Winner::Tie,
            }
        } else {
            let top = self.members.values().map(|m| m.score).max();
            let leaders: Vec<PlayerId> = self
                .members
                .iter()
                .filter(|(_, m)| Some(m.score) == top)
                .map(|(&p, _)| p)
                .collect();
            match leaders.as_slice() {
                [only] => Winner::Player(*only),
                _ => Winner::Tie,
            }
        }
    }
}

/// Three distinct pool indices: gold, hideout, bank. Seeded, so a match
/// replays; the original used `rand() % (rows - 1)` per site with a
/// distinctness re-draw behind an unidentified flag (CNR-6), so this is
/// an *Enhanced policy* that always keeps the three apart.
fn draw_three(rng: &mut NavRng, n: usize) -> [usize; 3] {
    debug_assert!(n >= 3);
    let a = (rng.next_u64() % n as u64) as usize;
    let mut b = (rng.next_u64() % n as u64) as usize;
    while b == a {
        b = (b + 1) % n;
    }
    let mut c = (rng.next_u64() % n as u64) as usize;
    while c == a || c == b {
        c = (c + 1) % n;
    }
    [a, b, c]
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: PlayerId = PlayerId(1);
    const B: PlayerId = PlayerId(2);
    const C: PlayerId = PlayerId(3);

    fn rules(variant: CnrVariant, end: EndRule) -> GoldRules {
        GoldRules {
            variant,
            end,
            load: CarrierLoad {
                added_mass_kg: 250.0,
                handling_scalar: 0.9,
            },
            pickup_points: 25,
            delivery_points: 100,
            pickup_radius: 5.0,
            delivery_radius: 12.0,
            drop_lockout_ticks: 120,
        }
    }

    fn pool(n: usize) -> Vec<Vec3> {
        (0..n)
            .map(|i| Vec3::new(i as f32 * 100.0, 0.0, (i * i) as f32 * 7.0))
            .collect()
    }

    fn ffa(end: EndRule) -> GoldMatch {
        GoldMatch::new(
            4,
            ObjectId {
                generation: 4,
                slot: 9,
            },
            rules(CnrVariant::FreeForAll, end),
            pool(8),
            77,
            &[(A, Side::Solo), (B, Side::Solo), (C, Side::Solo)],
        )
        .unwrap()
    }

    fn at(m: &GoldMatch, p: PlayerId, off: f32) -> Contact {
        Contact {
            player: p,
            round: m.round(),
            position: m.gold_position().unwrap() + Vec3::new(off, 0.0, 0.0),
        }
    }

    fn take(m: &mut GoldMatch, p: PlayerId) {
        let c = at(m, p, 0.0);
        assert!(matches!(
            m.resolve_pickups(&[c])[0],
            PickupVerdict::Granted { .. }
        ));
    }

    #[test]
    fn construction_is_checked() {
        let r = rules(CnrVariant::FreeForAll, EndRule::None);
        let g = ObjectId {
            generation: 1,
            slot: 0,
        };
        let new = |gen_, r, p, parts: &[(PlayerId, Side)]| GoldMatch::new(gen_, g, r, p, 1, parts);
        assert_eq!(
            new(2, r, pool(4), &[]).unwrap_err(),
            GoldError::ForeignGold,
            "the gold id must come from this generation"
        );
        assert_eq!(
            new(1, r, pool(2), &[]).unwrap_err(),
            GoldError::PoolTooSmall(2)
        );
        let mut bad = pool(4);
        bad[2].x = f32::NAN;
        assert_eq!(
            new(1, r, bad, &[]).unwrap_err(),
            GoldError::NonFiniteSite(2)
        );
        assert_eq!(
            new(1, r, pool(4), &[(A, Side::Cops)]).unwrap_err(),
            GoldError::WrongSide(A, Side::Cops)
        );
        assert_eq!(
            new(1, r, pool(4), &[(A, Side::Solo), (A, Side::Solo)]).unwrap_err(),
            GoldError::DuplicatePlayer(A)
        );
        let mut zero = r;
        zero.pickup_radius = 0.0;
        assert_eq!(
            new(1, zero, pool(4), &[]).unwrap_err(),
            GoldError::BadRules("pickup_radius")
        );
        let mut nan = r;
        nan.delivery_radius = f32::NAN;
        assert_eq!(
            new(1, nan, pool(4), &[]).unwrap_err(),
            GoldError::BadRules("delivery_radius")
        );
    }

    #[test]
    fn joiners_fill_the_sides_alternately() {
        let g = ObjectId {
            generation: 1,
            slot: 0,
        };
        for (variant, first, second) in [
            (CnrVariant::CopsVsRobbers, Side::Robbers, Side::Cops),
            (CnrVariant::RobbersVsRobbers, Side::Red, Side::Blue),
        ] {
            let mut m =
                GoldMatch::new(1, g, rules(variant, EndRule::None), pool(4), 1, &[]).unwrap();
            let mut got = Vec::new();
            for i in 1..=5u16 {
                let side = m.balanced_side();
                m.join(PlayerId(i), side).unwrap();
                got.push(side);
            }
            assert_eq!(got, [first, second, first, second, first]);
            // A leaver frees its place: the short side is taken next.
            m.leave(PlayerId(1), Vec3::ZERO).unwrap();
            m.leave(PlayerId(3), Vec3::ZERO).unwrap();
            assert_eq!(m.balanced_side(), first);
        }
        let mut m = ffa(EndRule::None);
        assert_eq!(m.balanced_side(), Side::Solo);
        m.join(PlayerId(9), Side::Solo).unwrap();
        assert_eq!(m.balanced_side(), Side::Solo);
    }

    #[test]
    fn a_round_draws_three_distinct_sites_from_the_pool() {
        for seed in 0..200 {
            let m = GoldMatch::new(
                1,
                ObjectId {
                    generation: 1,
                    slot: 0,
                },
                rules(CnrVariant::FreeForAll, EndRule::None),
                pool(3),
                seed,
                &[],
            )
            .unwrap();
            let s = m.sites();
            assert!(s.gold != s.hideout && s.gold != s.bank && s.hideout != s.bank);
        }
    }

    #[test]
    fn nearest_car_takes_fresh_gold_and_is_the_only_carrier() {
        let mut m = ffa(EndRule::None);
        let v = m.resolve_pickups(&[at(&m, A, 3.0), at(&m, B, 1.0)]);
        assert_eq!(
            v[0],
            PickupVerdict::Contested { winner: B },
            "the farther request loses"
        );
        assert_eq!(
            v[1],
            PickupVerdict::Granted {
                recovered: false,
                points: 25
            }
        );
        assert_eq!(m.carrier(), Some(B));
        assert_eq!(m.score(B), Some(25));
        assert_eq!(m.score(A), Some(0), "the loser scores nothing");
        let ev = m.drain_events();
        assert_eq!(
            ev.iter()
                .filter(|e| matches!(e, GoldEvent::Picked { .. }))
                .count(),
            1,
            "exactly one pickup event for two requests"
        );
    }

    #[test]
    fn simultaneous_pickups_resolve_the_same_in_every_arrival_order() {
        // Equidistant requests: the lower player id wins, whatever order
        // they arrived in; exactly one carrier, one award.
        let orders: [[PlayerId; 3]; 6] = [
            [A, B, C],
            [A, C, B],
            [B, A, C],
            [B, C, A],
            [C, A, B],
            [C, B, A],
        ];
        for order in orders {
            let mut m = ffa(EndRule::None);
            let contacts: Vec<Contact> = order.iter().map(|&p| at(&m, p, 2.0)).collect();
            let v = m.resolve_pickups(&contacts);
            assert_eq!(m.carrier(), Some(A), "order {order:?}");
            let granted = v
                .iter()
                .filter(|x| matches!(x, PickupVerdict::Granted { .. }))
                .count();
            assert_eq!(granted, 1);
            let total: u32 = m.standings().iter().map(|s| s.score).sum();
            assert_eq!(total, 25, "gold is never duplicated or double-scored");
        }
    }

    #[test]
    fn a_player_asking_twice_is_counted_once() {
        let mut m = ffa(EndRule::None);
        let c = at(&m, A, 1.0);
        let v = m.resolve_pickups(&[c, c]);
        assert!(matches!(v[0], PickupVerdict::Granted { .. }));
        assert_eq!(v[1], PickupVerdict::Duplicate);
        assert_eq!(m.score(A), Some(25));
    }

    #[test]
    fn ineligible_requests_are_refused_with_a_reason() {
        let mut m = ffa(EndRule::None);
        let mut far = at(&m, A, 0.0);
        far.position.x += 5.01;
        let mut nan = at(&m, B, 0.0);
        nan.position.y = f32::NAN;
        let mut stale = at(&m, C, 0.0);
        stale.round = 9;
        let stranger = at(&m, PlayerId(99), 0.0);
        let v = m.resolve_pickups(&[far, nan, stale, stranger]);
        assert_eq!(
            v,
            vec![
                PickupVerdict::OutOfRange,
                PickupVerdict::OutOfRange,
                PickupVerdict::Stale,
                PickupVerdict::NotParticipant,
            ]
        );
        assert!(matches!(m.state(), GoldState::Resting { .. }));
        assert_eq!(m.revision(), 0, "refusals change nothing");
        // The radius is inclusive.
        let mut edge = at(&m, A, 0.0);
        edge.position.x += 5.0;
        assert!(matches!(
            m.resolve_pickups(&[edge])[0],
            PickupVerdict::Granted { .. }
        ));
    }

    #[test]
    fn gold_in_a_car_cannot_be_taken_again() {
        let mut m = ffa(EndRule::None);
        take(&mut m, A);
        let rev = m.revision();
        let v = m.resolve_pickups(&[
            Contact {
                player: B,
                round: 0,
                position: Vec3::ZERO,
            },
            Contact {
                player: A,
                round: 0,
                position: Vec3::ZERO,
            },
        ]);
        assert_eq!(v, vec![PickupVerdict::AlreadyCarried { carrier: A }; 2]);
        assert_eq!(m.score(A), Some(25), "a repeat request does not re-award");
        assert_eq!(m.revision(), rev);
    }

    #[test]
    fn a_dislodged_carrier_drops_the_gold_and_anyone_else_may_recover_it() {
        let mut m = ffa(EndRule::None);
        take(&mut m, A);
        let spot = Vec3::new(500.0, 1.0, 40.0);
        m.dislodge(A, spot, DropCause::Knocked { by: Some(B) })
            .unwrap();
        assert_eq!(m.carrier(), None);
        assert_eq!(m.load_for(A), None, "the load ends with the carrying");
        assert_eq!(m.gold_position(), Some(spot));
        // The dropper sits on the gold but cannot retake it yet; the
        // rammer can.
        let v = m.resolve_pickups(&[
            Contact {
                player: A,
                round: 0,
                position: spot,
            },
            Contact {
                player: B,
                round: 0,
                position: spot + Vec3::X,
            },
        ]);
        assert_eq!(v[0], PickupVerdict::Locked);
        assert_eq!(
            v[1],
            PickupVerdict::Granted {
                recovered: true,
                points: 25
            }
        );
        assert_eq!(m.carrier(), Some(B));
        assert_eq!(m.load_for(B), Some(m.rules().load));
        assert_eq!(m.load_for(A), None);
        let ev = m.drain_events();
        assert!(ev.iter().any(|e| matches!(
            e,
            GoldEvent::Dropped {
                player,
                cause: DropCause::Knocked { by: Some(by) },
                ..
            } if *player == A && *by == B
        )));
        assert!(ev.iter().any(|e| matches!(
            e,
            GoldEvent::Picked {
                recovered: true,
                ..
            }
        )));
    }

    #[test]
    fn the_droppers_lockout_ends_after_its_ticks() {
        let mut m = ffa(EndRule::None);
        take(&mut m, A);
        let spot = Vec3::new(10.0, 0.0, 10.0);
        m.dislodge(A, spot, DropCause::Destroyed).unwrap();
        let c = Contact {
            player: A,
            round: 0,
            position: spot,
        };
        for _ in 0..119 {
            m.tick();
        }
        assert_eq!(m.resolve_pickups(&[c])[0], PickupVerdict::Locked);
        m.tick();
        assert!(matches!(
            m.resolve_pickups(&[c])[0],
            PickupVerdict::Granted {
                recovered: true,
                ..
            }
        ));
    }

    #[test]
    fn only_the_carrier_can_be_dislodged() {
        let mut m = ffa(EndRule::None);
        assert_eq!(
            m.dislodge(A, Vec3::ZERO, DropCause::Destroyed),
            Err(GoldError::NotCarrier(A)),
            "nothing to drop while the gold rests"
        );
        take(&mut m, A);
        assert_eq!(
            m.dislodge(B, Vec3::ZERO, DropCause::Destroyed),
            Err(GoldError::NotCarrier(B)),
            "a stale impact report against the old carrier"
        );
        m.dislodge(A, Vec3::new(1.0, 2.0, 3.0), DropCause::Destroyed)
            .unwrap();
        assert_eq!(
            m.dislodge(A, Vec3::ZERO, DropCause::Destroyed),
            Err(GoldError::NotCarrier(A)),
            "a repeated report cannot drop it twice"
        );
    }

    #[test]
    fn a_non_finite_drop_position_never_loses_the_gold() {
        let mut m = ffa(EndRule::None);
        take(&mut m, A);
        m.dislodge(A, Vec3::new(f32::NAN, 0.0, 0.0), DropCause::Destroyed)
            .unwrap();
        assert_eq!(m.gold_position(), Some(m.sites().gold));
    }

    #[test]
    fn delivery_scores_once_clears_the_carrier_and_redraws_the_round() {
        let mut m = ffa(EndRule::None);
        take(&mut m, A);
        let old = m.sites();
        let hideout = old.hideout;
        // Outside the radius: refused, still carrying.
        assert_eq!(
            m.deliver(A, 0, hideout + Vec3::new(12.1, 0.0, 0.0)),
            DeliveryVerdict::OutOfRange
        );
        assert_eq!(m.carrier(), Some(A));
        // The bank is not a free-for-all player's target.
        assert_eq!(m.deliver(A, 0, old.bank), DeliveryVerdict::OutOfRange);
        // Not the carrier.
        assert_eq!(m.deliver(B, 0, hideout), DeliveryVerdict::NotCarrier);
        // A stale round.
        assert_eq!(m.deliver(A, 7, hideout), DeliveryVerdict::Stale);
        m.drain_events();
        assert_eq!(
            m.deliver(A, 0, hideout + Vec3::new(12.0, 0.0, 0.0)),
            DeliveryVerdict::Delivered {
                points: 100,
                round: 1
            },
            "the radius is inclusive"
        );
        assert_eq!(m.score(A), Some(125), "25 for taking it, 100 for the stash");
        assert_eq!(m.carrier(), None);
        assert_eq!(m.load_for(A), None, "the load is gone with the gold");
        assert_eq!(m.round(), 1);
        let s = m.sites();
        assert!(s.gold != s.hideout && s.gold != s.bank && s.hideout != s.bank);
        assert_eq!(m.state(), GoldState::Resting { at: s.gold });
        // A second attempt cannot score again.
        assert_eq!(m.deliver(A, 1, s.hideout), DeliveryVerdict::NotCarrier);
        assert_eq!(m.score(A), Some(125));
        let ev = m.drain_events();
        assert!(matches!(ev[0], GoldEvent::Delivered { points: 100, .. }));
        assert!(matches!(ev[1], GoldEvent::SitesDrawn { round: 1, .. }));
    }

    #[test]
    fn pickup_and_delivery_in_the_same_tick_cannot_double_up() {
        let mut m = ffa(EndRule::None);
        take(&mut m, A);
        let old_round = m.round();
        let hideout = m.sites().hideout;
        // B asked for the gold against round 0 in the same tick A
        // delivers it; delivery is processed first.
        assert!(matches!(
            m.deliver(A, old_round, hideout),
            DeliveryVerdict::Delivered { .. }
        ));
        let v = m.resolve_pickups(&[Contact {
            player: B,
            round: old_round,
            position: m.gold_position().unwrap(),
        }]);
        assert_eq!(v, vec![PickupVerdict::Stale]);
        // Asked against the new round, B takes the fresh gold.
        let c = at(&m, B, 0.0);
        assert!(matches!(
            m.resolve_pickups(&[c])[0],
            PickupVerdict::Granted {
                recovered: false,
                ..
            }
        ));
    }

    #[test]
    fn cops_deliver_to_the_bank_and_robbers_to_the_hideout() {
        let mut m = GoldMatch::new(
            1,
            ObjectId {
                generation: 1,
                slot: 1,
            },
            rules(CnrVariant::CopsVsRobbers, EndRule::None),
            pool(6),
            5,
            &[(A, Side::Cops), (B, Side::Robbers)],
        )
        .unwrap();
        take(&mut m, A);
        let s = m.sites();
        assert_eq!(
            m.deliver(A, 0, s.hideout),
            DeliveryVerdict::OutOfRange,
            "a cop does not stash at the hideout"
        );
        assert!(matches!(
            m.deliver(A, 0, s.bank),
            DeliveryVerdict::Delivered { .. }
        ));
        take(&mut m, B);
        let s = m.sites();
        assert_eq!(m.deliver(B, 1, s.bank), DeliveryVerdict::OutOfRange);
        assert!(matches!(
            m.deliver(B, 1, s.hideout),
            DeliveryVerdict::Delivered { .. }
        ));
        assert_eq!(m.side_total(Side::Cops), 125);
        assert_eq!(m.side_total(Side::Robbers), 125);
    }

    #[test]
    fn the_point_limit_ends_a_free_for_all_and_freezes_the_match() {
        let mut m = ffa(EndRule::Points(125));
        take(&mut m, A);
        let h = m.sites().hideout;
        m.deliver(A, 0, h);
        let out = m.outcome().expect("125 reached");
        assert_eq!(out.reason, EndReason::PointLimit);
        assert_eq!(out.winner, Winner::Player(A));
        assert!(matches!(m.drain_events().last(), Some(GoldEvent::Ended(_))));
        // Nothing moves after the end.
        let c = at(&m, B, 0.0);
        assert_eq!(m.resolve_pickups(&[c]), vec![PickupVerdict::MatchOver]);
        assert_eq!(m.deliver(A, 1, h), DeliveryVerdict::MatchOver);
        assert_eq!(m.gold_out_of_bounds(), Err(GoldError::MatchOver));
        assert_eq!(m.join(PlayerId(8), Side::Solo), Err(GoldError::MatchOver));
        let rev = m.revision();
        m.tick();
        assert_eq!(m.revision(), rev);
        assert_eq!(m.elapsed_ticks(), 0, "the clock stops with the match");
    }

    #[test]
    fn team_variants_end_on_the_team_total_not_an_individual() {
        let mut m = GoldMatch::new(
            1,
            ObjectId {
                generation: 1,
                slot: 1,
            },
            rules(CnrVariant::RobbersVsRobbers, EndRule::Points(200)),
            pool(6),
            5,
            &[(A, Side::Red), (B, Side::Red), (C, Side::Blue)],
        )
        .unwrap();
        take(&mut m, A);
        let h = m.sites().hideout;
        m.deliver(A, 0, h); // red 125
        assert!(m.outcome().is_none());
        take(&mut m, B);
        let h = m.sites().hideout;
        m.deliver(B, 1, h); // red 250 across two players, neither at 200
        let out = m.outcome().expect("team total reached");
        assert_eq!(out.winner, Winner::Side(Side::Red));
        assert_eq!(m.side_total(Side::Blue), 0);
        assert!(m.score(A).unwrap() < 200 && m.score(B).unwrap() < 200);
    }

    #[test]
    fn blue_delivers_to_the_bank_draw() {
        let mut m = GoldMatch::new(
            1,
            ObjectId {
                generation: 1,
                slot: 1,
            },
            rules(CnrVariant::RobbersVsRobbers, EndRule::None),
            pool(6),
            5,
            &[(A, Side::Red), (C, Side::Blue)],
        )
        .unwrap();
        take(&mut m, C);
        let s = m.sites();
        assert_eq!(m.deliver(C, 0, s.hideout), DeliveryVerdict::OutOfRange);
        assert!(matches!(
            m.deliver(C, 0, s.bank),
            DeliveryVerdict::Delivered { .. }
        ));
    }

    #[test]
    fn the_time_limit_ends_the_match_and_level_scores_tie() {
        let mut m = ffa(EndRule::Ticks(3));
        m.tick();
        m.tick();
        assert!(m.outcome().is_none());
        m.tick();
        let out = m.outcome().unwrap();
        assert_eq!(out.reason, EndReason::TimeLimit);
        assert_eq!(out.winner, Winner::Tie, "nobody scored");
        assert_eq!(out.at_tick, 3);

        let mut m = ffa(EndRule::Ticks(2));
        take(&mut m, A);
        m.tick();
        m.tick();
        assert_eq!(m.outcome().unwrap().winner, Winner::Player(A));
    }

    #[test]
    fn level_team_totals_tie() {
        let mut m = GoldMatch::new(
            1,
            ObjectId {
                generation: 1,
                slot: 1,
            },
            rules(CnrVariant::CopsVsRobbers, EndRule::Ticks(1)),
            pool(6),
            5,
            &[(A, Side::Cops), (B, Side::Robbers)],
        )
        .unwrap();
        m.tick();
        assert_eq!(m.outcome().unwrap().winner, Winner::Tie);
    }

    #[test]
    fn a_leaving_carrier_drops_the_gold_and_keeps_the_points() {
        let mut m = ffa(EndRule::None);
        take(&mut m, A);
        let spot = Vec3::new(40.0, 0.0, -9.0);
        m.leave(A, spot).unwrap();
        assert_eq!(m.carrier(), None);
        assert_eq!(m.load_for(A), None);
        assert_eq!(m.gold_position(), Some(spot));
        assert_eq!(m.score(A), Some(25));
        let st = m.standings();
        assert!(st.iter().any(|s| s.player == A && !s.connected));
        // The leaver cannot retake it; no dropper lockout applies to
        // the others, and the gold is recoverable at once.
        let v = m.resolve_pickups(&[
            Contact {
                player: A,
                round: 0,
                position: spot,
            },
            Contact {
                player: B,
                round: 0,
                position: spot,
            },
        ]);
        assert_eq!(v[0], PickupVerdict::NotParticipant);
        assert!(matches!(
            v[1],
            PickupVerdict::Granted {
                recovered: true,
                ..
            }
        ));
        assert_eq!(m.leave(A, spot), Err(GoldError::UnknownPlayer(A)));
        assert_eq!(
            m.leave(PlayerId(50), spot),
            Err(GoldError::UnknownPlayer(PlayerId(50)))
        );
    }

    #[test]
    fn a_leavers_points_stay_in_the_team_total() {
        let mut m = GoldMatch::new(
            1,
            ObjectId {
                generation: 1,
                slot: 1,
            },
            rules(CnrVariant::CopsVsRobbers, EndRule::None),
            pool(6),
            5,
            &[(A, Side::Cops), (B, Side::Robbers)],
        )
        .unwrap();
        take(&mut m, A);
        let bank = m.sites().bank;
        m.deliver(A, 0, bank);
        m.leave(A, Vec3::ZERO).unwrap();
        assert_eq!(m.side_total(Side::Cops), 125);
    }

    #[test]
    fn late_joiners_start_clean_and_cannot_duplicate_an_id() {
        let mut m = GoldMatch::new(
            1,
            ObjectId {
                generation: 1,
                slot: 1,
            },
            rules(CnrVariant::CopsVsRobbers, EndRule::None),
            pool(6),
            5,
            &[(A, Side::Cops)],
        )
        .unwrap();
        take(&mut m, A);
        m.join(B, Side::Robbers).unwrap();
        assert_eq!(m.carrier(), Some(A), "joining never disturbs the gold");
        assert_eq!(m.score(B), Some(0));
        assert_eq!(m.join(B, Side::Cops), Err(GoldError::DuplicatePlayer(B)));
        assert_eq!(
            m.join(C, Side::Red),
            Err(GoldError::WrongSide(C, Side::Red))
        );
    }

    #[test]
    fn a_leaver_who_returns_resumes_their_side_and_points() {
        let mut m = GoldMatch::new(
            1,
            ObjectId {
                generation: 1,
                slot: 1,
            },
            rules(CnrVariant::CopsVsRobbers, EndRule::None),
            pool(6),
            5,
            &[(A, Side::Cops), (B, Side::Robbers)],
        )
        .unwrap();
        take(&mut m, A);
        m.leave(A, Vec3::ZERO).unwrap();
        assert_eq!(m.balanced_side(), Side::Cops, "a leaver frees their slot");
        let before = m.revision();
        assert_eq!(m.rejoin(A), Ok(Side::Cops));
        assert_eq!(m.score(A), Some(25), "points survive the absence");
        assert_eq!(m.side_of(A), Some(Side::Cops));
        assert_ne!(m.revision(), before, "replicas must see the return");
        assert!(m.standings().iter().all(|s| s.connected));
        // Back in the match, a second return is a repeat.
        assert_eq!(m.rejoin(A), Err(GoldError::DuplicatePlayer(A)));
        assert_eq!(m.rejoin(C), Err(GoldError::UnknownPlayer(C)));
    }

    #[test]
    fn nobody_returns_to_a_finished_match() {
        let mut m = ffa(EndRule::Ticks(1));
        m.leave(A, Vec3::ZERO).unwrap();
        m.tick();
        assert!(m.outcome().is_some());
        assert_eq!(m.rejoin(A), Err(GoldError::MatchOver));
    }

    #[test]
    fn gold_out_of_bounds_is_replaced_without_score_or_a_lost_objective() {
        let mut m = ffa(EndRule::None);
        let before = m.sites();
        m.gold_out_of_bounds().unwrap();
        let after = m.sites();
        assert_eq!(after.hideout, before.hideout);
        assert_eq!(after.bank, before.bank);
        assert!(after.gold != after.hideout && after.gold != after.bank);
        assert_eq!(m.state(), GoldState::Resting { at: after.gold });
        assert_eq!(m.round(), 1);
        assert!(
            m.standings().iter().all(|s| s.score == 0),
            "no score for a loss"
        );
        // A request against the lost position is stale.
        let stale = Contact {
            player: A,
            round: 0,
            position: after.gold,
        };
        assert_eq!(m.resolve_pickups(&[stale]), vec![PickupVerdict::Stale]);
        // Dropped gold out of bounds is re-placed too.
        take(&mut m, A);
        m.dislodge(A, Vec3::new(9e5, 0.0, 0.0), DropCause::Destroyed)
            .unwrap();
        m.gold_out_of_bounds().unwrap();
        assert_eq!(m.round(), 2);
        assert!(matches!(m.state(), GoldState::Resting { .. }));
        // But not while a car holds it.
        let c = at(&m, A, 0.0);
        m.resolve_pickups(&[c]);
        assert_eq!(m.gold_out_of_bounds(), Err(GoldError::Carried));
    }

    #[test]
    fn the_load_follows_the_carrier_only_and_a_new_match_starts_without_one() {
        let mut m = ffa(EndRule::None);
        for p in [A, B, C] {
            assert_eq!(m.load_for(p), None);
        }
        take(&mut m, B);
        assert_eq!(m.load_for(B), Some(m.rules().load));
        assert_eq!(m.load_for(A), None);
        assert_eq!(m.load_for(C), None);
        let h = m.sites().hideout;
        m.deliver(B, 0, h);
        for p in [A, B, C] {
            assert_eq!(m.load_for(p), None, "nobody keeps a load after a stash");
        }
        // Round restart: a carrier of the old match is nobody in the new.
        take(&mut m, C);
        let fresh = GoldMatch::new(
            5,
            ObjectId {
                generation: 5,
                slot: 9,
            },
            rules(CnrVariant::FreeForAll, EndRule::None),
            pool(8),
            78,
            &[(A, Side::Solo), (B, Side::Solo), (C, Side::Solo)],
        )
        .unwrap();
        assert_eq!(fresh.load_for(C), None);
        assert_eq!(fresh.carrier(), None);
        assert_eq!(fresh.round(), 0);
        assert_eq!(fresh.revision(), 0);
    }

    #[test]
    fn the_same_inputs_replay_the_same_match_and_a_seed_matters() {
        let script = |seed: u64| {
            let mut m = GoldMatch::new(
                1,
                ObjectId {
                    generation: 1,
                    slot: 0,
                },
                rules(CnrVariant::FreeForAll, EndRule::None),
                pool(40),
                seed,
                &[(A, Side::Solo), (B, Side::Solo)],
            )
            .unwrap();
            let mut seen = Vec::new();
            for _ in 0..5 {
                let c = at(&m, A, 0.0);
                m.resolve_pickups(&[c]);
                let h = m.sites().hideout;
                m.deliver(A, m.round(), h);
                seen.push(m.sites().gold.to_array().map(f32::to_bits));
            }
            (seen, m.drain_events().len(), m.revision())
        };
        assert_eq!(script(11), script(11));
        assert_ne!(script(11).0, script(12).0);
    }

    #[test]
    fn opening_sites_name_the_draw_a_match_makes() {
        let raw: Vec<[f32; 3]> = pool(9).iter().map(|p| p.to_array()).collect();
        let r = rules(CnrVariant::FreeForAll, EndRule::None);
        for seed in [0, 1, 7, 1291] {
            let game = GoldMatch::new(
                1,
                ObjectId {
                    generation: 1,
                    slot: 1,
                },
                r,
                pool(9),
                seed,
                &[],
            )
            .unwrap();
            let s = game.sites();
            assert_eq!(
                GoldMatch::opening_sites(r, &raw, seed),
                Some([s.gold, s.hideout, s.bank].map(|p| p.to_array())),
                "seed {seed}"
            );
        }
        // A pool that cannot seed a round names no draw.
        assert_eq!(GoldMatch::opening_sites(r, &raw[..2], 0), None);
    }

    #[test]
    fn the_revision_counts_state_changes_and_events_drain_once() {
        let mut m = ffa(EndRule::None);
        assert_eq!(m.revision(), 0);
        take(&mut m, A);
        assert_eq!(m.revision(), 1);
        m.dislodge(A, Vec3::ZERO, DropCause::Destroyed).unwrap();
        assert_eq!(m.revision(), 2);
        assert_eq!(m.drain_events().len(), 2);
        assert!(m.drain_events().is_empty());
        assert_eq!(m.revision(), 2);
    }

    #[test]
    fn a_view_copies_what_a_replica_shows_and_ranks_by_freshness() {
        let mut m = ffa(EndRule::Ticks(50));
        let first = m.view();
        assert_eq!(first.generation, m.generation());
        assert_eq!(first.variant, CnrVariant::FreeForAll);
        assert_eq!(first.end, EndRule::Ticks(50));
        assert_eq!(first.sites, m.sites());
        assert_eq!(first.state, m.state());
        assert_eq!(first.gold_position(), m.gold_position());
        assert_eq!(first.carrier(), None);
        assert_eq!(first.standings, m.standings());
        assert_eq!(first.outcome, None);

        m.tick();
        let ticked = m.view();
        assert!(
            ticked.freshness() > first.freshness(),
            "a later clock within one revision is fresher"
        );
        take(&mut m, A);
        let held = m.view();
        assert_eq!(held.carrier(), Some(A));
        assert_eq!(held.gold_position(), None);
        assert!(held.freshness() > ticked.freshness());
        assert_eq!(held.standings[0].score, m.score(A).unwrap());
    }

    #[test]
    fn balanced_sides_fill_the_smaller_team_first() {
        use CnrVariant::*;
        assert_eq!(balanced_side(FreeForAll, []), Side::Solo);
        assert_eq!(balanced_side(CopsVsRobbers, []), Side::Robbers);
        assert_eq!(
            balanced_side(CopsVsRobbers, [Side::Robbers]),
            Side::Cops,
            "cops are the minority"
        );
        assert_eq!(
            balanced_side(CopsVsRobbers, [Side::Robbers, Side::Cops]),
            Side::Robbers
        );
        assert_eq!(
            balanced_side(RobbersVsRobbers, [Side::Red, Side::Red, Side::Blue]),
            Side::Blue
        );
        // A foreign side is ignored rather than counted.
        assert_eq!(
            balanced_side(RobbersVsRobbers, [Side::Cops, Side::Cops]),
            Side::Red
        );
    }

    #[test]
    fn variants_name_their_sides() {
        for v in CnrVariant::ALL {
            assert_eq!(v.sides().len() > 1, v.team_scored());
        }
    }

    fn cops_and_robbers() -> GoldMatch {
        GoldMatch::new(
            4,
            ObjectId {
                generation: 4,
                slot: 9,
            },
            rules(CnrVariant::CopsVsRobbers, EndRule::None),
            pool(8),
            77,
            &[(A, Side::Robbers), (B, Side::Cops)],
        )
        .unwrap()
    }

    fn calls(m: &mut GoldMatch) -> Vec<Call> {
        let sides: Vec<Standing> = m.standings();
        m.drain_events()
            .iter()
            .filter_map(|e| e.call(|p| sides.iter().find(|s| s.player == p).map(|s| s.side)))
            .collect()
    }

    #[test]
    fn events_earn_the_call_for_the_carriers_side() {
        let mut m = cops_and_robbers();
        m.drain_events();
        take(&mut m, A);
        assert_eq!(
            calls(&mut m),
            [Call {
                kind: CallKind::Get,
                side: Side::Robbers
            }]
        );
        let at = m.gold_position().unwrap_or(Vec3::ZERO);
        m.dislodge(A, at, DropCause::Destroyed).unwrap();
        assert_eq!(
            calls(&mut m),
            [Call {
                kind: CallKind::Drop,
                side: Side::Robbers
            }]
        );
        // B recovers what A lost (A is locked out of it).
        take(&mut m, B);
        assert_eq!(
            calls(&mut m),
            [Call {
                kind: CallKind::Recover,
                side: Side::Cops
            }]
        );
        // The cops deliver to the bank; the stash is announced once and
        // the round redraw, which follows it, is not.
        let bank = m.sites().bank;
        m.deliver(B, 0, bank);
        assert_eq!(
            calls(&mut m),
            [Call {
                kind: CallKind::Stash,
                side: Side::Cops
            }]
        );
    }

    #[test]
    fn an_unknown_dropper_earns_no_call() {
        let drop = GoldEvent::Dropped {
            player: C,
            at: Vec3::ZERO,
            cause: DropCause::Disconnected,
            round: 0,
        };
        assert_eq!(drop.call(|_| None), None);
        for quiet in [
            GoldEvent::Left { player: A },
            GoldEvent::Joined {
                player: A,
                side: Side::Solo,
            },
        ] {
            assert_eq!(quiet.call(|_| Some(Side::Solo)), None);
        }
    }

    #[test]
    fn a_replicas_change_earns_the_call_the_events_did() {
        let mut m = cops_and_robbers();
        let start = m.view();
        take(&mut m, A);
        let carried = m.view();
        let got = |next: &GoldView, prev: &GoldView| next.call_since(prev);
        assert_eq!(
            got(&carried, &start),
            Some(Call {
                kind: CallKind::Get,
                side: Side::Robbers
            })
        );
        // A repeat of the same frame, or an older one, is silent.
        assert_eq!(got(&carried, &carried), None);
        assert_eq!(got(&start, &carried), None);
        let at = m.gold_position().unwrap_or(Vec3::ZERO);
        m.dislodge(A, at, DropCause::Knocked { by: Some(B) })
            .unwrap();
        let dropped = m.view();
        assert_eq!(
            got(&dropped, &carried),
            Some(Call {
                kind: CallKind::Drop,
                side: Side::Robbers
            })
        );
        take(&mut m, B);
        let recovered = m.view();
        assert_eq!(
            got(&recovered, &dropped),
            Some(Call {
                kind: CallKind::Recover,
                side: Side::Cops
            })
        );
        let bank = m.sites().bank;
        m.deliver(B, 0, bank);
        let stashed = m.view();
        assert_eq!(
            got(&stashed, &recovered),
            Some(Call {
                kind: CallKind::Stash,
                side: Side::Cops
            })
        );
        // Frames that skip a step still read from what they show: the
        // carrier is gone and the round moved on.
        assert_eq!(
            got(&stashed, &carried),
            Some(Call {
                kind: CallKind::Stash,
                side: Side::Robbers
            })
        );
    }

    #[test]
    fn a_replica_stays_silent_across_a_new_match_or_a_backwards_round() {
        let mut m = cops_and_robbers();
        take(&mut m, A);
        let carried = m.view();
        let mut other = carried.clone();
        other.generation += 1;
        other.state = GoldState::Resting { at: Vec3::ZERO };
        other.revision += 5;
        assert_eq!(other.call_since(&carried), None, "a new generation");
        let mut earlier = carried.clone();
        earlier.round = carried.round + 1;
        let mut later = carried.clone();
        later.revision += 1;
        later.state = GoldState::Dropped {
            at: Vec3::ZERO,
            by: Some(A),
            free_at: 0,
        };
        assert_eq!(later.call_since(&earlier), None, "a round that went back");
        // A carrier missing from the standings (never seated) cannot be
        // placed on a side, so there is no line to voice.
        let mut ghost = carried.clone();
        ghost.revision += 1;
        ghost.state = GoldState::Carried { by: C };
        let mut resting = carried.clone();
        resting.state = GoldState::Resting { at: Vec3::ZERO };
        assert_eq!(ghost.call_since(&resting), None);
    }
}
