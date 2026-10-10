//! F27-B.3 — the Cops & Robbers match on the wire (protocol v21).
//!
//! The host's [`CnrHost`] is the only place the gold's ownership, the
//! scores and the winner are decided. Clients never send a pickup or a
//! score — the host measures every car's position itself — so the wire
//! has one direction: [`publish_cnr`] copies the match's observable
//! state ([`GoldView`]) into a `Message::Cnr` frame, and a client's
//! [`apply_cnr`] folds the newest frame into a [`CnrReplica`] the HUD
//! and the markers read. The frame is *state*, not an event: a lost
//! one self-corrects on the next, a repeated or reordered one is
//! dropped by `(revision, elapsed)`, and a late joiner lands on the
//! current match from the periodic resend alone.
//!
//! Nothing a client receives is trusted blindly: [`decode_view`]
//! refuses an unnamed discriminant, a non-finite position, a duplicated
//! participant, a side the variant does not have, a carrier who is not
//! a participant and a winner who is not one — counted, never repaired
//! into something plausible — and a refused frame cannot move the
//! freshness watermark, so one corrupt frame does not turn every honest
//! one after it into a stale drop.
//!
//! **Not covered yet.** Nothing starts a match in the shipped app (the
//! lobby that picks variant, sides and limits is F27-B.4), so the
//! publisher idles there; the two-process leg (separate host and client
//! processes, the impairment matrix) is F27-C. A *rematch* inside one
//! session generation restarts `revision` at 0, which the stage would
//! drop as stale: B.4 must either mint a new generation per match or
//! add a match epoch to the frame.

use bevy::prelude::*;
use mm2_game::gold::{
    CnrVariant, EndReason, EndRule, GoldState, GoldView, Outcome, Side, Sites, Standing, Winner,
};
use mm2_game::{PlayerId, Session, SessionPhase};
use mm2_net::{
    MAX_SNAP_CNR_SEATS, Message, SNAP_CNR_NO_PLAYER, SnapCnr, SnapCnrOutcome, SnapCnrSeat,
};

use crate::cnr::CnrHost;
use crate::net::HostLink;
use crate::netdrive::{NetDriveReport, RemoteSnaps};

/// Match ticks between the host's unprompted frames: about a second at
/// the fixed rate. A change of state publishes at once; this is the
/// repair for a lost frame and the way a late joiner learns the match.
pub const PUBLISH_EVERY_TICKS: u64 = mm2_game::RACE_TICK_HZ as u64;

/// Runs of [`publish_cnr`] (one per rendered frame) between repeats of a
/// *decided* match's final frame. A decided match's clock has stopped,
/// so [`PUBLISH_EVERY_TICKS`] can never come due again and the one
/// change-driven frame would be the client's only chance to learn the
/// result — a lost or reordered-away copy would leave it on the live HUD
/// for good. About two seconds at 60 Hz; stops with the session.
pub const DECIDED_REPEAT_RUNS: u32 = 120;

/// Generations the client-side inbox tracks at once — a frame of a
/// generation that is not the session's is held only so it can be
/// refused counted, never so it can poison the session's own.
const STAGED_GENERATIONS: usize = 4;

// Wire discriminants. They name the *wire's* encoding, fixed by this
// protocol version; the Rust enums' own order is not part of it.

const VARIANT_FREE_FOR_ALL: u8 = 0;
const VARIANT_COPS_VS_ROBBERS: u8 = 1;
const VARIANT_ROBBERS_VS_ROBBERS: u8 = 2;

const END_NONE: u8 = 0;
const END_TICKS: u8 = 1;
const END_POINTS: u8 = 2;

const GOLD_RESTING: u8 = 0;
const GOLD_CARRIED: u8 = 1;
const GOLD_DROPPED: u8 = 2;

const SIDE_SOLO: u8 = 0;
const SIDE_ROBBERS: u8 = 1;
const SIDE_COPS: u8 = 2;
const SIDE_RED: u8 = 3;
const SIDE_BLUE: u8 = 4;

const REASON_POINT_LIMIT: u8 = 0;
const REASON_TIME_LIMIT: u8 = 1;

const WINNER_PLAYER: u8 = 0;
const WINNER_SIDE: u8 = 1;
const WINNER_TIE: u8 = 2;

/// Why a received frame was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CnrWireError {
    /// A discriminant the wire does not name (the field).
    Unnamed(&'static str),
    /// A position that is not finite (the field).
    NonFinite(&'static str),
    /// Two rows for one participant.
    DuplicateSeat(u16),
    /// A row names the "no participant" id.
    ReservedSeat,
    /// A seat's side does not exist in the frame's variant.
    ForeignSide(u16),
    /// The gold's holder contradicts its ownership state (the state).
    BadHolder(&'static str),
    /// The outcome names a winner the match does not have.
    BadWinner,
}

/// The wire's name for a variant.
fn variant_code(v: CnrVariant) -> u8 {
    match v {
        CnrVariant::FreeForAll => VARIANT_FREE_FOR_ALL,
        CnrVariant::CopsVsRobbers => VARIANT_COPS_VS_ROBBERS,
        CnrVariant::RobbersVsRobbers => VARIANT_ROBBERS_VS_ROBBERS,
    }
}

fn variant_from(code: u8) -> Option<CnrVariant> {
    Some(match code {
        VARIANT_FREE_FOR_ALL => CnrVariant::FreeForAll,
        VARIANT_COPS_VS_ROBBERS => CnrVariant::CopsVsRobbers,
        VARIANT_ROBBERS_VS_ROBBERS => CnrVariant::RobbersVsRobbers,
        _ => return None,
    })
}

fn side_code(s: Side) -> u8 {
    match s {
        Side::Solo => SIDE_SOLO,
        Side::Robbers => SIDE_ROBBERS,
        Side::Cops => SIDE_COPS,
        Side::Red => SIDE_RED,
        Side::Blue => SIDE_BLUE,
    }
}

fn side_from(code: u8) -> Option<Side> {
    Some(match code {
        SIDE_SOLO => Side::Solo,
        SIDE_ROBBERS => Side::Robbers,
        SIDE_COPS => Side::Cops,
        SIDE_RED => Side::Red,
        SIDE_BLUE => Side::Blue,
        _ => return None,
    })
}

/// A frame for `view` — the host's match flattened for the wire.
pub fn encode_view(view: &GoldView) -> SnapCnr {
    let (end, end_value) = match view.end {
        EndRule::None => (END_NONE, 0),
        EndRule::Ticks(t) => (END_TICKS, t),
        EndRule::Points(p) => (END_POINTS, u64::from(p)),
    };
    let (gold, holder, at, free_at) = match view.state {
        GoldState::Resting { at } => (GOLD_RESTING, SNAP_CNR_NO_PLAYER, at, 0),
        GoldState::Carried { by } => (GOLD_CARRIED, by.0, Vec3::ZERO, 0),
        GoldState::Dropped { at, by, free_at } => (
            GOLD_DROPPED,
            by.map_or(SNAP_CNR_NO_PLAYER, |p| p.0),
            at,
            free_at,
        ),
    };
    let outcome = view.outcome.map(|o| {
        let (winner, who) = match o.winner {
            Winner::Player(p) => (WINNER_PLAYER, p.0),
            Winner::Side(s) => (WINNER_SIDE, u16::from(side_code(s))),
            Winner::Tie => (WINNER_TIE, 0),
        };
        SnapCnrOutcome {
            reason: match o.reason {
                EndReason::PointLimit => REASON_POINT_LIMIT,
                EndReason::TimeLimit => REASON_TIME_LIMIT,
            },
            winner,
            who,
            at_tick: o.at_tick,
        }
    });
    SnapCnr {
        variant: variant_code(view.variant),
        end,
        end_value,
        round: view.round,
        revision: view.revision,
        elapsed: view.elapsed,
        gold,
        holder,
        at: at.to_array(),
        free_at,
        sites: [
            view.sites.gold.to_array(),
            view.sites.hideout.to_array(),
            view.sites.bank.to_array(),
        ],
        outcome,
        seats: view
            .standings
            .iter()
            .map(|s| SnapCnrSeat {
                player: s.player.0,
                side: side_code(s.side),
                connected: s.connected,
                score: s.score,
            })
            .collect(),
    }
}

fn finite(v: [f32; 3], field: &'static str) -> Result<Vec3, CnrWireError> {
    let v = Vec3::from_array(v);
    if v.is_finite() {
        Ok(v)
    } else {
        Err(CnrWireError::NonFinite(field))
    }
}

/// The match a received frame describes, or why it is refused. `wire`
/// is the generation the message carried; the frame has none of its own.
pub fn decode_view(wire: u64, frame: &SnapCnr) -> Result<GoldView, CnrWireError> {
    let variant = variant_from(frame.variant).ok_or(CnrWireError::Unnamed("variant"))?;
    let end = match frame.end {
        END_NONE => EndRule::None,
        END_TICKS => EndRule::Ticks(frame.end_value),
        END_POINTS => EndRule::Points(
            u32::try_from(frame.end_value).map_err(|_| CnrWireError::Unnamed("end_value"))?,
        ),
        _ => return Err(CnrWireError::Unnamed("end")),
    };
    // The decoder bounds the row count; this keeps the invariant local.
    if frame.seats.len() > MAX_SNAP_CNR_SEATS as usize {
        return Err(CnrWireError::Unnamed("seats"));
    }
    let mut standings: Vec<Standing> = Vec::with_capacity(frame.seats.len());
    for seat in &frame.seats {
        if seat.player == SNAP_CNR_NO_PLAYER {
            return Err(CnrWireError::ReservedSeat);
        }
        if standings.iter().any(|s| s.player.0 == seat.player) {
            return Err(CnrWireError::DuplicateSeat(seat.player));
        }
        let side = side_from(seat.side).ok_or(CnrWireError::Unnamed("side"))?;
        if !variant.sides().contains(&side) {
            return Err(CnrWireError::ForeignSide(seat.player));
        }
        standings.push(Standing {
            player: PlayerId(seat.player),
            side,
            score: seat.score,
            connected: seat.connected,
        });
    }
    let is_seat = |id: u16| standings.iter().any(|s| s.player.0 == id);

    let state = match frame.gold {
        GOLD_RESTING => {
            if frame.holder != SNAP_CNR_NO_PLAYER {
                return Err(CnrWireError::BadHolder("resting"));
            }
            GoldState::Resting {
                at: finite(frame.at, "at")?,
            }
        }
        GOLD_CARRIED => {
            if !is_seat(frame.holder) {
                return Err(CnrWireError::BadHolder("carried"));
            }
            GoldState::Carried {
                by: PlayerId(frame.holder),
            }
        }
        GOLD_DROPPED => {
            // A dropper who left is still a row (points stay on the
            // board); one that names no seat at all is not coherent.
            let by = match frame.holder {
                SNAP_CNR_NO_PLAYER => None,
                id if is_seat(id) => Some(PlayerId(id)),
                _ => return Err(CnrWireError::BadHolder("dropped")),
            };
            GoldState::Dropped {
                at: finite(frame.at, "at")?,
                by,
                free_at: frame.free_at,
            }
        }
        _ => return Err(CnrWireError::Unnamed("gold")),
    };
    let sites = Sites {
        gold: finite(frame.sites[0], "gold site")?,
        hideout: finite(frame.sites[1], "hideout site")?,
        bank: finite(frame.sites[2], "bank site")?,
    };
    let outcome = frame
        .outcome
        .map(|o| -> Result<Outcome, CnrWireError> {
            let reason = match o.reason {
                REASON_POINT_LIMIT => EndReason::PointLimit,
                REASON_TIME_LIMIT => EndReason::TimeLimit,
                _ => return Err(CnrWireError::Unnamed("reason")),
            };
            let winner = match o.winner {
                WINNER_PLAYER if is_seat(o.who) => Winner::Player(PlayerId(o.who)),
                WINNER_SIDE => {
                    let side = u8::try_from(o.who)
                        .ok()
                        .and_then(side_from)
                        .filter(|s| variant.sides().contains(s))
                        .ok_or(CnrWireError::BadWinner)?;
                    Winner::Side(side)
                }
                WINNER_TIE => Winner::Tie,
                WINNER_PLAYER => return Err(CnrWireError::BadWinner),
                _ => return Err(CnrWireError::Unnamed("winner")),
            };
            Ok(Outcome {
                reason,
                winner,
                at_tick: o.at_tick,
            })
        })
        .transpose()?;
    Ok(GoldView {
        generation: wire,
        variant,
        end,
        round: frame.round,
        revision: frame.revision,
        elapsed: frame.elapsed,
        state,
        sites,
        outcome,
        standings,
    })
}

/// The client's copy of the host's match: what the HUD reads. Present
/// from the first accepted frame of the session's generation until the
/// session ends; never written on the authority, which has the
/// [`CnrHost`] itself.
#[derive(Resource, Debug, Clone, PartialEq)]
pub struct CnrReplica(pub GoldView);

/// One generation's newest accepted frame.
#[derive(Debug, Clone)]
struct Staged {
    generation: u64,
    view: GoldView,
    /// Accepted but not yet applied.
    fresh: bool,
}

/// The client-side inbox, a field of [`RemoteSnaps`] so the stream's
/// authority boundaries (an accepted `Start`, the link's `Closed`) reset
/// it with everything else.
///
/// Holds the freshest frame *per generation*: an older or equal one
/// adds nothing and is dropped counted, and a frame of another
/// generation can neither displace nor stale-mark the session's own.
#[derive(Default)]
pub struct CnrStage {
    /// Newest accepted frame per generation, at most
    /// [`STAGED_GENERATIONS`] — the oldest generation is evicted.
    frames: Vec<Staged>,
    stale: u64,
    refused: u64,
    landed: u64,
}

impl CnrStage {
    /// Queue one received frame.
    pub fn push(&mut self, generation: u64, frame: &SnapCnr) {
        // Refused before it can move a watermark.
        let view = match decode_view(generation, frame) {
            Ok(view) => view,
            Err(_) => {
                self.refused += 1;
                return;
            }
        };
        if let Some(entry) = self.frames.iter_mut().find(|f| f.generation == generation) {
            if view.freshness() <= entry.view.freshness() {
                self.stale += 1;
            } else {
                entry.view = view;
                entry.fresh = true;
            }
            return;
        }
        if self.frames.len() >= STAGED_GENERATIONS
            && let Some(oldest) = (0..self.frames.len()).min_by_key(|&i| self.frames[i].generation)
        {
            self.frames.swap_remove(oldest);
        }
        self.frames.push(Staged {
            generation,
            view,
            fresh: true,
        });
    }

    /// Drop everything staged and every watermark — the authority's
    /// stream ended. Counters are evidence, not stream state, and
    /// survive.
    pub fn reset(&mut self) {
        self.frames.clear();
    }

    /// Frames dropped as no fresher than one already seen.
    pub fn stale(&self) -> u64 {
        self.stale
    }

    /// Frames refused: malformed, or another generation's at apply
    /// time.
    pub fn refused(&self) -> u64 {
        self.refused
    }

    /// Frames folded into the replica.
    pub fn landed(&self) -> u64 {
        self.landed
    }

    /// The view waiting to be applied for `wire`'s generation; fresh
    /// frames of any other generation are dropped refused.
    fn take_for(&mut self, wire: u64) -> Option<GoldView> {
        let mut found = None;
        for entry in &mut self.frames {
            if !entry.fresh {
                continue;
            }
            entry.fresh = false;
            if entry.generation == wire {
                found = Some(entry.view.clone());
            } else {
                self.refused += 1;
            }
        }
        found
    }
}

/// Host: tell the clients where the match stands.
///
/// Sends only on a hosted session with a [`CnrHost`], in a phase where
/// the match exists for the clients (`Countdown`..`Results`), on every
/// change of the match's state and otherwise every
/// [`PUBLISH_EVERY_TICKS`] of match time — so a pause, which stops the
/// match clock, also stops the repeats. A decided match's clock is
/// stopped for good, so its final frame repeats every
/// [`DECIDED_REPEAT_RUNS`] runs instead (the client's stage drops the
/// repeat as stale once it has the frame). A frame the link refused is
/// not counted as sent and is tried again next run.
pub fn publish_cnr(
    host: Res<HostLink>,
    session: Res<Session>,
    cnr: Option<Res<CnrHost>>,
    mut last: Local<Option<(u64, (u64, u64))>>,
    mut quiet_runs: Local<u32>,
    report: Option<ResMut<NetDriveReport>>,
) {
    let Some(cnr) = cnr else {
        return;
    };
    if !session.authority_role().is_authority()
        || !matches!(
            session.phase(),
            SessionPhase::Countdown | SessionPhase::Playing | SessionPhase::Results
        )
    {
        return;
    }
    let generation = session.wire_generation();
    let view = cnr.game.view();
    let key = view.freshness();
    let due = match *last {
        Some((g, (rev, elapsed))) if g == generation => {
            // A different revision is a change; a clock that went
            // backwards is a new match — send either at once.
            key.0 != rev || key.1 < elapsed || key.1 - elapsed >= PUBLISH_EVERY_TICKS || {
                *quiet_runs = quiet_runs.saturating_add(1);
                view.outcome.is_some() && *quiet_runs >= DECIDED_REPEAT_RUNS
            }
        }
        _ => true,
    };
    if !due {
        return;
    }
    let frame = Message::Cnr {
        generation,
        frame: encode_view(&view),
    };
    if host.ctl().broadcast(&frame).is_ok() {
        *last = Some((generation, key));
        *quiet_runs = 0;
        if let Some(mut report) = report {
            report.cnr_sent += 1;
        }
    }
}

/// Client: fold the newest staged frame into [`CnrReplica`]. An
/// authority never applies (it has the match), a `Loading` or `Paused`
/// session holds the frame, and a session that is gone drops it and the
/// replica with it.
pub fn apply_cnr(
    mut commands: Commands,
    mut snaps: ResMut<RemoteSnaps>,
    session: Res<Session>,
    replica: Option<Res<CnrReplica>>,
    report: Option<ResMut<NetDriveReport>>,
) {
    if session.authority_role().is_authority() {
        return;
    }
    match session.phase() {
        SessionPhase::Loading | SessionPhase::Paused => return,
        SessionPhase::Ready
        | SessionPhase::Countdown
        | SessionPhase::Playing
        | SessionPhase::Results => {}
        _ => {
            snaps.cnr.reset();
            if replica.is_some() {
                commands.remove_resource::<CnrReplica>();
            }
            return;
        }
    }
    let stage = &mut snaps.cnr;
    if let Some(view) = stage.take_for(session.wire_generation()) {
        stage.landed += 1;
        commands.insert_resource(CnrReplica(view));
    }
    if let Some(mut report) = report {
        report.cnr_landed = stage.landed;
        report.cnr_stale = stage.stale;
        report.cnr_refused = stage.refused;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mm2_game::ObjectId;
    use mm2_game::gold::{CarrierLoad, Contact, GoldMatch, GoldRules};

    const A: PlayerId = PlayerId(1);
    const B: PlayerId = PlayerId(2);

    fn rules(variant: CnrVariant, end: EndRule) -> GoldRules {
        GoldRules {
            variant,
            end,
            load: CarrierLoad::NONE,
            pickup_points: 25,
            delivery_points: 100,
            pickup_radius: 4.0,
            delivery_radius: 12.0,
            drop_lockout_ticks: 10,
        }
    }

    fn game(variant: CnrVariant, end: EndRule, parts: &[(PlayerId, Side)]) -> GoldMatch {
        let pool = (0..6)
            .map(|i| Vec3::new(i as f32 * 50.0, 0.0, -(i as f32) * 30.0))
            .collect();
        GoldMatch::new(
            3,
            ObjectId {
                generation: 3,
                slot: 0,
            },
            rules(variant, end),
            pool,
            7,
            parts,
        )
        .unwrap()
    }

    fn ffa() -> GoldMatch {
        game(
            CnrVariant::FreeForAll,
            EndRule::Points(300),
            &[(A, Side::Solo), (B, Side::Solo)],
        )
    }

    fn take(m: &mut GoldMatch, p: PlayerId) {
        let at = m.gold_position().unwrap();
        let c = Contact {
            player: p,
            round: m.round(),
            position: at,
        };
        m.resolve_pickups(&[c]);
    }

    /// Every ownership state of every variant crosses the wire intact.
    #[test]
    fn a_view_round_trips_through_the_wire_in_every_ownership_state() {
        for variant in CnrVariant::ALL {
            let sides = variant.sides();
            let parts = [(A, sides[0]), (B, sides[sides.len() - 1])];
            let mut m = game(variant, EndRule::Ticks(5_000), &parts);
            let mut seen = vec![m.view()];
            take(&mut m, A);
            seen.push(m.view());
            m.dislodge(
                A,
                Vec3::new(1.0, 2.0, 3.0),
                mm2_game::gold::DropCause::Knocked { by: Some(B) },
            )
            .unwrap();
            m.tick();
            seen.push(m.view());
            m.leave(B, Vec3::ZERO).unwrap();
            seen.push(m.view());
            for view in seen {
                let frame = encode_view(&view);
                let back = decode_view(view.generation, &frame).unwrap();
                assert_eq!(back, view, "{variant:?} {:?}", view.state);
                let wire = Message::Cnr {
                    generation: view.generation,
                    frame: frame.clone(),
                };
                let Message::Cnr { frame: again, .. } =
                    Message::decode(&wire.encode().unwrap()).unwrap()
                else {
                    panic!("a cnr frame decodes as one");
                };
                assert_eq!(again, frame, "the bytes are the frame");
            }
        }
    }

    #[test]
    fn a_finished_match_carries_its_outcome_in_every_winner_shape() {
        // Time limit, tie.
        let mut m = game(
            CnrVariant::FreeForAll,
            EndRule::Ticks(2),
            &[(A, Side::Solo), (B, Side::Solo)],
        );
        m.tick();
        m.tick();
        let v = m.view();
        assert_eq!(v.outcome.unwrap().reason, EndReason::TimeLimit);
        assert_eq!(decode_view(3, &encode_view(&v)).unwrap(), v);

        // Point limit, individual winner.
        let mut m = game(
            CnrVariant::FreeForAll,
            EndRule::Points(25),
            &[(A, Side::Solo), (B, Side::Solo)],
        );
        take(&mut m, A);
        let v = m.view();
        assert_eq!(v.outcome.unwrap().winner, Winner::Player(A));
        assert_eq!(decode_view(3, &encode_view(&v)).unwrap(), v);

        // Point limit, team winner.
        let mut m = game(
            CnrVariant::CopsVsRobbers,
            EndRule::Points(25),
            &[(A, Side::Robbers), (B, Side::Cops)],
        );
        take(&mut m, B);
        let v = m.view();
        assert_eq!(v.outcome.unwrap().winner, Winner::Side(Side::Cops));
        assert_eq!(decode_view(3, &encode_view(&v)).unwrap(), v);
    }

    fn base() -> SnapCnr {
        encode_view(&ffa().view())
    }

    #[test]
    fn a_frame_that_contradicts_itself_is_refused_not_repaired() {
        let refuse = |edit: &dyn Fn(&mut SnapCnr), want: CnrWireError| {
            let mut f = base();
            edit(&mut f);
            assert_eq!(decode_view(3, &f).unwrap_err(), want);
        };
        refuse(&|f| f.variant = 9, CnrWireError::Unnamed("variant"));
        refuse(&|f| f.end = 7, CnrWireError::Unnamed("end"));
        refuse(
            &|f| {
                f.end = END_POINTS;
                f.end_value = u64::MAX;
            },
            CnrWireError::Unnamed("end_value"),
        );
        refuse(&|f| f.gold = 5, CnrWireError::Unnamed("gold"));
        refuse(&|f| f.seats[0].side = 9, CnrWireError::Unnamed("side"));
        refuse(
            &|f| f.seats[0].side = SIDE_COPS,
            CnrWireError::ForeignSide(f_first_player()),
        );
        refuse(
            &|f| f.seats[1].player = f.seats[0].player,
            CnrWireError::DuplicateSeat(f_first_player()),
        );
        refuse(
            &|f| f.seats[0].player = SNAP_CNR_NO_PLAYER,
            CnrWireError::ReservedSeat,
        );
        // Resting gold with a holder.
        refuse(&|f| f.holder = 1, CnrWireError::BadHolder("resting"));
        // Carried by nobody, and by a stranger.
        refuse(
            &|f| {
                f.gold = GOLD_CARRIED;
                f.holder = SNAP_CNR_NO_PLAYER;
            },
            CnrWireError::BadHolder("carried"),
        );
        refuse(
            &|f| {
                f.gold = GOLD_CARRIED;
                f.holder = 77;
            },
            CnrWireError::BadHolder("carried"),
        );
        // Dropped by a stranger.
        refuse(
            &|f| {
                f.gold = GOLD_DROPPED;
                f.holder = 77;
            },
            CnrWireError::BadHolder("dropped"),
        );
        refuse(&|f| f.at[1] = f32::NAN, CnrWireError::NonFinite("at"));
        refuse(
            &|f| f.sites[2][0] = f32::INFINITY,
            CnrWireError::NonFinite("bank site"),
        );
        refuse(
            &|f| {
                f.outcome = Some(SnapCnrOutcome {
                    reason: 9,
                    winner: WINNER_TIE,
                    who: 0,
                    at_tick: 1,
                })
            },
            CnrWireError::Unnamed("reason"),
        );
        refuse(
            &|f| {
                f.outcome = Some(SnapCnrOutcome {
                    reason: REASON_POINT_LIMIT,
                    winner: WINNER_PLAYER,
                    who: 99,
                    at_tick: 1,
                })
            },
            CnrWireError::BadWinner,
        );
        // A free-for-all has no team to win.
        refuse(
            &|f| {
                f.outcome = Some(SnapCnrOutcome {
                    reason: REASON_POINT_LIMIT,
                    winner: WINNER_SIDE,
                    who: u16::from(SIDE_COPS),
                    at_tick: 1,
                })
            },
            CnrWireError::BadWinner,
        );
        refuse(
            &|f| {
                f.outcome = Some(SnapCnrOutcome {
                    reason: REASON_POINT_LIMIT,
                    winner: 8,
                    who: 0,
                    at_tick: 1,
                })
            },
            CnrWireError::Unnamed("winner"),
        );
    }

    fn f_first_player() -> u16 {
        base().seats[0].player
    }

    #[test]
    fn the_stage_keeps_the_freshest_frame_per_generation() {
        let mut m = ffa();
        let mut stage = CnrStage::default();
        let first = encode_view(&m.view());
        stage.push(3, &first);
        stage.push(3, &first);
        assert_eq!(stage.stale(), 1, "a repeat adds nothing");
        assert_eq!(stage.take_for(3).unwrap().revision, 0);
        assert!(stage.take_for(3).is_none(), "a frame applies once");

        m.tick();
        take(&mut m, A);
        let newer = encode_view(&m.view());
        stage.push(3, &newer);
        // The reordered older frame arrives after the newer one.
        stage.push(3, &first);
        assert_eq!(stage.stale(), 2);
        let view = stage.take_for(3).unwrap();
        assert_eq!(view.carrier(), Some(A));
        // Within one revision the later clock wins.
        let mut later = newer.clone();
        later.elapsed += 5;
        stage.push(3, &later);
        assert_eq!(stage.take_for(3).unwrap().elapsed, view.elapsed + 5);
    }

    #[test]
    fn another_generation_is_refused_and_never_marks_the_sessions_own_stale() {
        let mut m = ffa();
        let mut stage = CnrStage::default();
        m.tick();
        take(&mut m, A);
        stage.push(4, &encode_view(&m.view()));
        stage.push(3, &base());
        assert_eq!(stage.stale(), 0);
        assert_eq!(stage.take_for(3).unwrap().revision, 0);
        assert_eq!(stage.refused(), 1, "generation 4's frame is not ours");
    }

    #[test]
    fn a_malformed_frame_cannot_become_the_watermark() {
        let mut stage = CnrStage::default();
        let mut bad = base();
        bad.revision = u64::MAX;
        bad.gold = 5;
        stage.push(3, &bad);
        assert_eq!(stage.refused(), 1);
        stage.push(3, &base());
        assert_eq!(stage.stale(), 0, "the honest frame still lands");
        assert!(stage.take_for(3).is_some());
    }

    #[test]
    fn the_stage_tracks_a_bounded_set_of_generations() {
        let mut stage = CnrStage::default();
        for g in 0..64 {
            stage.push(g, &base());
        }
        assert_eq!(stage.frames.len(), STAGED_GENERATIONS);
        assert!(
            stage.frames.iter().all(|f| f.generation >= 60),
            "the oldest generations are the ones evicted"
        );
    }

    #[test]
    fn a_reset_forgets_the_stream_but_not_the_evidence() {
        let mut stage = CnrStage::default();
        stage.push(1, &base());
        stage.push(1, &base());
        stage.reset();
        assert!(stage.take_for(1).is_none());
        stage.push(1, &base());
        assert_eq!(stage.stale(), 1, "counters survive");
        assert!(stage.take_for(1).is_some(), "the watermark does not");
    }
}
