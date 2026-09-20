//! Stable session identities and authority roles (F01-B).
//!
//! Bevy `Entity` values are recycled and meaningless outside one world —
//! they are never the identity a result, impact or network message may
//! refer to. [`ObjectId`] and [`PlayerId`] are the session-stable
//! identities those contracts use. Both are minted through [`Session`],
//! which namespaces objects by [`generation`](crate::Session::generation)
//! so ids minted by an older session are detectably stale rather than
//! silently colliding with the current one.
//!
//! [`AuthorityRole`] is the per-entity boundary between simulated truth
//! and prediction: offline (`Local`) and hosted (`Host`) sessions mark
//! everything `Authority`; a `Remote` client marks server-owned objects
//! `Predicted`. Rule systems must not look at the network to decide who
//! is in charge — they read the role the session stamped.

use bevy::prelude::*;

/// Stable identity of a player participant within a session — the local
/// driver, a remote driver, or an AI participant. Never an `Entity`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PlayerId(pub u16);

/// Stable identity of a simulated object within one session generation.
/// `generation` matches the session that minted it; `slot` is unique
/// within that generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjectId {
    /// Session [`generation`](crate::Session::generation) that minted
    /// this id.
    pub generation: u64,
    /// Unique slot within the generation.
    pub slot: u32,
}

impl ObjectId {
    /// Identity for unmarked static world geometry — city rooms, ground
    /// planes and props nobody minted an id for. Generation 0 with a slot
    /// no allocator reaches, so it can never collide with a minted id.
    pub const WORLD: Self = Self {
        generation: 0,
        slot: u32::MAX,
    };

    /// Whether this is the [`WORLD`](Self::WORLD) sentinel rather than a
    /// session-minted identity.
    pub fn is_world(self) -> bool {
        self == Self::WORLD
    }
}

/// Component carrying an entity's stable simulation identity. Session
/// objects that participate in contracts (vehicles, dynamic props) are
/// stamped at spawn; unmarked static geometry reads as
/// [`ObjectId::WORLD`].
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjectIdentity(pub ObjectId);

/// Who drives a [`Player`]: locally controlled input, a remote driver
/// (networked — groundwork, no networking exists yet), or an AI
/// participant. Written so queries never assume a single local player
/// (AC05).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerControl {
    /// Input comes from this process's devices.
    Local,
    /// Input arrives over the network.
    Remote,
    /// Input is produced by game AI.
    Ai,
}

/// A player participant. Multiple may coexist — a local driver plus
/// remote/AI drivers — so consumers must query by [`PlayerId`], never by
/// "the" player.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Player {
    /// Stable participant identity.
    pub id: PlayerId,
    /// Who produces this player's input.
    pub control: PlayerControl,
}

/// Per-entity authority boundary: whether this process's simulation is
/// the truth for the entity (`Authority`) or a local prediction/copy of
/// someone else's truth (`Predicted`). Stamped at spawn from
/// [`Session::authority_role`](crate::Session::authority_role).
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorityRole {
    /// This process decides this entity's game rules — single player and
    /// the host of a networked session.
    Authority,
    /// A remote authority decides; this copy predicts/replicates.
    Predicted,
}

impl AuthorityRole {
    /// Whether this role means local rule authority.
    pub fn is_authority(self) -> bool {
        matches!(self, Self::Authority)
    }
}
