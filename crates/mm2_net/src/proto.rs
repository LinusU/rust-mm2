//! The wire protocol: message types, encoding and the compatibility
//! gate the handshake applies.
//!
//! All integers are little-endian. Strings are `u16le`-length-prefixed
//! UTF-8 bounded by [`MAX_STRING`]. Decoding is strict: unknown type
//! bytes, truncated fields, over-long strings and trailing bytes are
//! all errors — a peer speaking something we cannot name is a protocol
//! violation, not a parse to fudge.

/// Wire version. Bumped for any incompatible message change; peers must
/// match exactly. v2: `RosterEntry` gained the driver's `pick` field and
/// the `SetVehicle`/`VehicleRefused` negotiation pair landed. v3:
/// `Start`/`Cancel` (session lifecycle) and `RejectCode::SessionStarted`
/// landed. v4: `Input`/`Snap` — the in-session driving transport
/// (F25-A). v5: `SnapEntry` gained `epoch`, the authority's per-player
/// reset counter (F25-A.5). v6: `ResetRequest` — a driver asking the
/// authority to reset its own seat (F25-B). v7: `SnapEntry` gained the
/// presentation tail — steer/spin/compression/flags a remote copy's
/// wheel and light visuals consume (F25-B). v8: `SnapEntry` gained
/// `damage`, the seat's authoritative damage fraction — a remote
/// copy's (and a predicted own seat's) `VehicleDamage` is replicated
/// state, never locally accumulated (F25-B, F05 req 6). v9: `Snap`
/// gained `trailers` — a bounded list of trailered seats' trailer
/// poses (F25-B); a trailered pick's trailer is replicated state like
/// the seat itself, never simulated on clients. v10: `Snap` gained
/// `impacts` — a bounded list of [`SnapImpact`] rows replicating the
/// authority's filtered `ImpactEvent` stream per participant seat, so
/// remote copies can render per-impact presentation (sparks, impact
/// audio) the damage *state* byte cannot carry (F25-B). v11:
/// `SnapEntry` gained `breaks`, the seat's detached-breakaway-part
/// bitmask — replicated *state* (not an event) so a dropped snap or a
/// late join can never leave a remote copy's rig diverged from the
/// authority's (F25-B, F05 req 5). v12: `SnapImpact` gained
/// `audio_id`, the struck side's authored `AudioId` resolved on the
/// authority at publish, so a replicated row voices the same impact
/// category the authority played (F25-B). v13: `Snap` gained `race`,
/// an optional [`SnapRace`] row carrying the authority's race phase,
/// countdown remainder and clock — a predicted client's race loop
/// never steps under a remote authority, so the wire mirrors it
/// (F25-B). v14: `SnapEntry` gained the per-seat race-progress tail —
/// the seat's participant state, resolution tick, lap/gate counters,
/// cleared-gate bitmask and evidence counters — replicated *state*
/// like `damage`/`breaks`, so a predicted client whose rule pipeline
/// never advances `RaceProgress` mirrors every seat's standing
/// (F25-B). v15: `SnapEntry` gained `rpm`, the authority's engine RPM —
/// the last piece of engine state a remote copy's `EngineVoice` rig
/// needs to mix like the authority's (F25-B). v16: `SnapEntry` gained
/// the surface-contact tail — the dominant grounded wheel's resolved
/// `sound` class plus its slippage and longitudinal speed — so a remote
/// copy's `SurfaceRig` replays the same `SkidSpec`/`RollingSpec` pick
/// through its own surface table (F25-B). v17: `Message::Props` — the
/// authority's world-prop state (knocked, broken and settled bangers)
/// as its own host→client frame, so a client's copy of the world
/// matches the host's (F26-A). v18: the `Props` frame carries the
/// sender's [`SiteTable`] — the placement count and a digest of what
/// the city stamp minted — so a client whose stamped world differs
/// refuses the rows instead of misattributing them (F26-A).
pub const PROTOCOL_VERSION: u16 = 18;

/// Byte cap on any length-prefixed string field.
pub const MAX_STRING: usize = 256;

/// Roster ceiling: MP-1 (documented — `help:Types of Multiplayer
/// Connections`, `help:Multiplayer Screen`) puts TCP/IP play at up to 8
/// players total. The wire keeps the bound so a hostile roster payload
/// cannot claim an unbounded crowd.
pub const MAX_PLAYERS: u8 = 8;

const TAG_HELLO: u8 = 0x01;
const TAG_ACCEPT: u8 = 0x02;
const TAG_REJECT: u8 = 0x03;
const TAG_WELCOME: u8 = 0x04;
const TAG_ROSTER: u8 = 0x05;
const TAG_SET_READY: u8 = 0x06;
const TAG_LEAVE: u8 = 0x07;
const TAG_SESSION: u8 = 0x08;
const TAG_SET_VEHICLE: u8 = 0x09;
const TAG_VEHICLE_REFUSED: u8 = 0x0a;
const TAG_START: u8 = 0x0b;
const TAG_CANCEL: u8 = 0x0c;
const TAG_INPUT: u8 = 0x0d;
const TAG_SNAP: u8 = 0x0e;
const TAG_RESET_REQUEST: u8 = 0x0f;
const TAG_PROPS: u8 = 0x10;

/// Byte cap on a [`SessionAdvertisement`]'s opaque `params` field — the
/// `mm2_app` bridge's serialized session config is a few hundred bytes,
/// so 4 KiB is far above need while still trivially bounded.
pub const MAX_SESSION_PARAMS: usize = 4096;

/// What the lobby is configured to run — host → clients, opaque to the
/// wire. `mm2_net` knows nothing about cities, modes or settings: the
/// `mm2_app` bridge owns the `params` encoding and both peers decode it
/// with identical code at an identical [`PROTOCOL_VERSION`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionAdvertisement {
    /// One-line display summary for lobby UIs/CLIs
    /// (`"sf, cruise, amateur"`).
    pub summary: String,
    /// The engine's own encoding of the session parameters — bounded by
    /// [`MAX_SESSION_PARAMS`]; the lobby bounds and carries it but never
    /// parses it.
    pub params: Vec<u8>,
}

/// The first message a client sends: identity plus the compatibility
/// evidence the host checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    /// Protocol version the client speaks.
    pub protocol: u16,
    /// `mm2_content::fingerprint::gameplay` over the client's resolved
    /// content — tuning, bounds, geometry, city and event data. A
    /// cosmetic-only mod leaves this untouched.
    pub gameplay_fingerprint: u64,
    /// Engine build identifier (diagnostic, not a gate).
    pub build: String,
    /// Driver name for the lobby roster.
    pub driver: String,
}

/// Why a host refused a `Hello`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectCode {
    /// `Hello.protocol` != [`PROTOCOL_VERSION`].
    VersionMismatch = 1,
    /// Gameplay fingerprints differ — different content, so different
    /// rules. Cosmetic-only differences never reach this.
    ContentMismatch = 2,
    /// The first frame was not a well-formed `Hello`.
    Malformed = 3,
    /// The roster was already at capacity when the peer handshook.
    LobbyFull = 4,
    /// The lobby already started a session that does not accept late
    /// joins (MP-5's race rule — the host chose `LateJoin::Closed` at
    /// start). Cruise-style sessions stay joinable and never produce
    /// this.
    SessionStarted = 5,
}

/// A driver's vehicle pick as the lobby carries it: an opaque content
/// id plus a paint index. The wire bounds both (`vehicle` ≤
/// [`MAX_STRING`], `paint` a `u8`) but knows nothing about which ids or
/// paints are legal — that is the consumer's validator
/// (`HostConfig::pick_validator`), which runs on the authoritative side.
/// What `vehicle` *means* is likewise the consumer's: `mm2_app` carries
/// catalog ids (`vpbug`) and uses the empty string for the synthetic dev
/// car.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VehiclePick {
    /// The picked vehicle's content id (consumer-defined).
    pub vehicle: String,
    /// Zero-based paint index.
    pub paint: u8,
}

/// One roster entry — the lobby's view of a connected driver. Player ids
/// are host-minted `u16` slots; `0` is reserved for the host player at the
/// app layer, so wire ids start at 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RosterEntry {
    /// Host-assigned slot.
    pub player_id: u16,
    /// Display name from the peer's `Hello`.
    pub driver: String,
    /// Build identifier from the peer's `Hello` (diagnostic).
    pub build: String,
    /// Whether the peer has marked itself ready to start.
    pub ready: bool,
    /// The driver's current vehicle pick; `None` until the peer sets one.
    pub pick: Option<VehiclePick>,
}

/// One sampled driver input, client → host while a session runs
/// (F25-A). The channels are quantized for the wire — the consumer's
/// `mm2_app` side owns the mapping (0..255 spans the sim's normalized
/// ranges; `steer` is signed so centered steering encodes exactly 0).
/// `generation`/`seq` let the host drop input for a session it is not
/// running and order samples without trusting sender clocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DriveInput {
    /// The session generation this input belongs to.
    pub generation: u64,
    /// Sender-side sample counter, monotonic per client process — the
    /// freshness tag the host's mailbox enforces: a sample that is not
    /// ahead of the stored `seq` is a duplicate or an arrival-order
    /// regression and is refused. Ordered TCP cannot produce either in
    /// practice; the tag is what lets the impairment harness (and any
    /// future unordered transport) reorder or duplicate frames without
    /// regressing the mailbox.
    pub seq: u64,
    /// Quantized throttle, 0..=255.
    pub throttle: u8,
    /// Quantized brake, 0..=255.
    pub brake: u8,
    /// Quantized steering, -127..=127 (0 = centered).
    pub steer: i8,
    /// Quantized handbrake, 0..=255.
    pub handbrake: u8,
}

/// One participant's authoritative rigid state inside a [`Message::Snap`].
/// `player` is the wire roster id — the host's own seat is 0 (it is never
/// a roster entry, but its car is part of the shared sim). Positions and
/// velocities are world-space `f32`s — the same precision the sim runs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SnapEntry {
    /// Wire roster slot (0 = the host seat).
    pub player: u16,
    /// World position, metres.
    pub pos: [f32; 3],
    /// World rotation, quaternion `x,y,z,w`.
    pub rot: [f32; 4],
    /// Linear velocity, m/s.
    pub vel: [f32; 3],
    /// Angular velocity, rad/s.
    pub angvel: [f32; 3],
    /// The authority's reset counter for this seat — bumped every time
    /// the host teleports the participant (damage/stuck/recovery/manual
    /// reset), wrapping at 256. A changed value means the asserted pose
    /// is a teleport, not motion: receivers snap to it rather than
    /// blending, and the owning client reconciles its own seat. Only
    /// *difference* is read, so a wrap is a false snap at worst, never a
    /// missed reset — 256 resets between applied snapshots is far past
    /// any plausible stream gap.
    pub epoch: u8,
    /// Presentation tail (v7, F25-B): the drive state a remote copy's
    /// wheel/light visuals need but cannot derive from pose. These
    /// fields are *informational* — they carry no authority over the
    /// replicated pose, and receivers clamp/derive rather than trust.
    /// Actual steering angle at the front wheels, milliradians
    /// (saturating) — quantized on the authority's `VehicleState`, not
    /// the input, so assists and rate limits are already reflected.
    pub steer: i16,
    /// Mean grounded-wheel angular rate in 0.1 rad/s units, signed
    /// (negative = rolling backwards), saturating. `0` while no wheel
    /// is grounded — the sim's own rule holds a lifted wheel's angle
    /// rather than free-spinning it.
    pub spin: i16,
    /// Mean suspension compression as a fraction of each wheel's own
    /// travel, ×255. Aggregated across wheels by design — per-wheel
    /// droop deltas are presentation detail, not wire cost.
    pub compression: u8,
    /// Presentation flags: bit 0 `SNAP_FLAG_BRAKE`, bit 1
    /// `SNAP_FLAG_REVERSE`, bit 2 `SNAP_FLAG_GROUNDED`.
    pub flags: u8,
    /// Damage fraction (v8, F25-B): the authority's `VehicleDamage`
    /// total as a fraction of the seat's authored `MaxDamage`, ×255 —
    /// `0` intact (also the value a participant with no authored
    /// `vehcardamage` record publishes — undamageable reads as
    /// undamaged), `255` at/over the destruction bound. Informational
    /// like the rest of the tail: receivers reconstitute the total
    /// through their own copy's spec and never accumulate locally.
    pub damage: u8,
    /// Breakaway bitmask (v11, F25-B): bit *i* set = the seat's
    /// `VehicleBreaks` part *i* (authored order — identical rigs on
    /// every process, enforced by the gameplay fingerprint) is off
    /// the rig. Replicated state like `damage`, not an event: a
    /// receiver diffs it against its copy's rig every snap, so a
    /// repair arrives as the bits clearing. `0` on a seat with no
    /// authored break inventory. Parts past bit 31 never ride the
    /// wire — far past any authored count.
    pub breaks: u32,
    /// Race-progress tail (v14, F25-B): the seat's participant
    /// lifecycle state as an opaque discriminant the `mm2_app`
    /// consumer names (`mm2_game::ParticipantState`'s encoding —
    /// 0 awaiting start, 1 racing, 2 finished, 3 timed out). `0` on
    /// a seat the race does not track and on a raceless session.
    pub prog_state: u8,
    /// The resolution's race-clock tick while `prog_state` names a
    /// terminal state; `0` otherwise.
    pub prog_ticks: u64,
    /// `Ordered` rule: completed laps. `0` under `AnyOrder`.
    pub prog_lap: u32,
    /// `Ordered` rule: index of the next required gate. `0` under
    /// `AnyOrder`.
    pub prog_next: u32,
    /// Cleared-gate bitmask — bit *i* set = gate *i* cleared in
    /// authored order (identical definitions on every process, per
    /// the gameplay fingerprint). Gates past bit 63 are
    /// unexpressible — far past any authored count.
    pub prog_cleared: u64,
    /// Total gate crossings credited by trigger sweep — a diagnostic
    /// counter the consumer's race HUD/records read.
    pub prog_crossings: u32,
    /// Gates credited by driven-route position rather than a physical
    /// crossing — the consumer's route-clear evidence counter.
    pub prog_route_clears: u32,
    /// Engine RPM (v15, F25-B): the authority's `VehicleState::rpm`
    /// quantized to whole revolutions, saturating at 65535 — far past
    /// any authored redline. Presentation state like the rest of the
    /// tail: it feeds a remote copy's `EngineVoice` mix and nothing
    /// else, and the `u16` domain is itself the bound a hostile value
    /// cannot exceed.
    pub rpm: u16,
    /// Surface-contact tail (v16, F25-B): the dominant *skid* contact's
    /// resolved surface class — the `SurfaceTables::sound_index` row
    /// into the session's surface table, keyed by authored `sound`
    /// class rather than a receiver's row index so a modded
    /// (`aud/` rides no gameplay fingerprint) table still answers the
    /// same class. [`SNAP_NO_SURFACE`] while no grounded wheel's pick
    /// resolves. The wheel's own quantities ride beside it — every
    /// process re-runs `SkidSpec::pick` under its own spec's unit
    /// (`skid_slip` for `Slippage`, `skid_speed` for `Speed`) and mixes
    /// its own band/gain rather than trusting the authority's choice.
    pub surf_skid: u16,
    /// The winning skid wheel's slippage (`tire_slippage`'s clamped
    /// 0..1 utilization) ×255.
    pub skid_slip: u8,
    /// The winning skid wheel's `vel_long` in 0.1 m/s, signed —
    /// `Speed`-unit picks read `|vel_long|`, and a backwards-rolling
    /// wheel's skid is still a skid.
    pub skid_speed: i16,
    /// The dominant *rolling* contact's surface class — same class
    /// space and [`SNAP_NO_SURFACE`] sentinel as `surf_skid`. The
    /// loop's speed is the car's forward speed, derivable from `vel`
    /// and `rot`, so nothing else rides.
    pub surf_roll: u16,
}

impl Default for SnapEntry {
    /// The zero pose with the surface tail at its "no contact"
    /// sentinel — a defaulted entry must not claim a class-0 skid.
    fn default() -> Self {
        Self {
            player: 0,
            pos: [0.0; 3],
            rot: [0.0; 4],
            vel: [0.0; 3],
            angvel: [0.0; 3],
            epoch: 0,
            steer: 0,
            spin: 0,
            compression: 0,
            flags: 0,
            damage: 0,
            breaks: 0,
            prog_state: 0,
            prog_ticks: 0,
            prog_lap: 0,
            prog_next: 0,
            prog_cleared: 0,
            prog_crossings: 0,
            prog_route_clears: 0,
            rpm: 0,
            surf_skid: SNAP_NO_SURFACE,
            skid_slip: 0,
            skid_speed: 0,
            surf_roll: SNAP_NO_SURFACE,
        }
    }
}

/// [`SnapEntry::flags`] bit 0 — the driver's brake pedal is held (the
/// same threshold the brake/reverse glows read).
pub const SNAP_FLAG_BRAKE: u8 = 0x01;
/// [`SnapEntry::flags`] bit 1 — the drivetrain is engaged in reverse.
pub const SNAP_FLAG_REVERSE: u8 = 0x02;
/// [`SnapEntry::flags`] bit 2 — any wheel has ground contact; clear
/// means the copy hangs its wheels at full droop.
pub const SNAP_FLAG_GROUNDED: u8 = 0x04;

/// The `surf_skid`/`surf_roll` "no contact" sentinel (v16, F25-B) —
/// distinct from every reachable class so a quiet wheel never
/// misreads as surface row 0.
pub const SNAP_NO_SURFACE: u16 = u16::MAX;

/// One participant's trailer inside a [`Message::Snap`] (v9, F25-B).
/// A trailered pick (`vpsemi`, `vpcentury`) tows a real jointed body on
/// the authority; clients hold a kinematic copy the same way they hold
/// the car's. `owner` is the *towing seat's* wire roster id — the
/// trailer rides its tractor's entry's reset epoch, so it carries none
/// of its own: receivers snap it when the owner entry's epoch advances,
/// exactly like the seat it follows. `spin`/`flags` use the
/// [`SnapEntry`] encodings (mean grounded-wheel rate in 0.1 rad/s
/// units; bit 0 = [`SNAP_FLAG_GROUNDED`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SnapTrailer {
    /// Wire roster slot of the towing seat (0 = the host seat).
    pub owner: u16,
    /// World position, metres.
    pub pos: [f32; 3],
    /// World rotation, quaternion `x,y,z,w`.
    pub rot: [f32; 4],
    /// Linear velocity, m/s.
    pub vel: [f32; 3],
    /// Angular velocity, rad/s.
    pub angvel: [f32; 3],
    /// Mean grounded-wheel angular rate in 0.1 rad/s units, signed and
    /// saturating — the `SnapEntry::spin` encoding.
    pub spin: i16,
    /// Bit 0 `SNAP_FLAG_GROUNDED` — the other presentation bits are
    /// seat state (brake/reverse) a trailer does not own.
    pub flags: u8,
}

/// Snapshot bound on replicated impact rows: several frames' worth of
/// the authority's filtered `ImpactEvent` stream (16 events per tick ×
/// up to two seat rows each), so a burst never writes an unbounded
/// tail. The stream is loss-tolerant presentation — a dropped frame's
/// effects are ephemeral, not state.
pub const MAX_SNAP_IMPACTS: u8 = 64;

/// One replicated side of a participant impact inside a
/// [`Message::Snap`] (v10, F25-B). The authority's
/// `mm2_game::ImpactEvent` carries a contact pair; each participant
/// that is a `NetPlayer` seat emits one row, so a car-vs-car hit rides
/// the wire as two rows sharing `id` — `seat` names which side the row
/// presents (the two rows carry mirrored normals). `(seat, id)` is the
/// dedup key for reordered/duplicated frames; the receiver's own seat
/// is skipped — its copy already rendered the impact from the local
/// physics stream. `surface` does not ride the wire: no current
/// consumer reads it, and state consumers resolve the copy's live
/// `SurfaceState` instead. What the wire *does* carry (v12) is the
/// struck side's authored audio selector — the identity a receiver
/// cannot resolve itself because a struck prop's `ObjectId` lives in
/// the authority's local id namespace.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SnapImpact {
    /// Wire roster slot of the seat this side presents (0 = the host
    /// seat).
    pub seat: u16,
    /// The authority-side `ImpactId` — session-unique per generation,
    /// so `(seat, id)` survives reordering and duplication.
    pub id: u64,
    /// Host session tick the impact was emitted on — diagnostic, not an
    /// ordering guarantee (`id` already is).
    pub tick: u64,
    /// World-space contact point, metres.
    pub point: [f32; 3],
    /// Outward contact normal from this seat's side of the contact —
    /// the normal a spark burst or impact voice positions against.
    pub normal: [f32; 3],
    /// Relative impact speed, m/s — `ImpactEvent::severity`.
    pub severity: f32,
    /// The struck (non-seat) side's authored `dgBangerData` `AudioId`,
    /// resolved on the authority — the same lookup `impact_voices`
    /// runs locally (`0` when the struck side is the world, another
    /// seat or carries no banger record). Verbatim authored data: any
    /// value is representable and an unresolvable one reads as a data
    /// failure downstream, exactly like a mod's broken binding.
    pub audio_id: i64,
}

/// The authority's race state inside a [`Message::Snap`] (v13, F25-B).
/// `mm2_net` stays contract-free — the phase encoding is opaque to the
/// wire — so this is an untyped tuple the `mm2_app` consumer names:
/// `phase` is the race lifecycle discriminant (`mm2_game::RacePhase`'s
/// encoding lives on the app side), `countdown` the ticks left while
/// the race counts down, `clock` the race clock in fixed ticks. The
/// row rides every snapshot while the authority's session runs an
/// event — *state*, not an event, so a dropped or reordered frame
/// self-corrects on the next, and the receiver keeps the freshest by
/// `(phase rank, progress)` rather than arrival order. Absent (`None`)
/// on a session with no race — cruise and dev worlds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SnapRace {
    /// Opaque lifecycle discriminant — the consumer names the phases.
    pub phase: u8,
    /// Ticks until control releases while `phase` names the countdown;
    /// `0` once running or complete.
    pub countdown: u32,
    /// The authority's race clock — fixed ticks since the release.
    pub clock: u64,
}

/// Bound on one [`Message::Props`] frame's rows: the banger pool's
/// active bodies (32), a burst of fresh transitions and the rolling
/// resend window of settled/broken state — far above a normal frame,
/// small enough that a hostile count cannot claim an unbounded tail.
pub const MAX_SNAP_PROPS: u8 = 96;

/// A [`SnapProp::fragment`] value meaning "the placement itself" rather
/// than one of its break fragments.
pub const SNAP_NO_FRAGMENT: u8 = u8::MAX;

/// A summary of a process's stamped banger placements, exchanged on
/// every [`Message::Props`] frame (v18, F26-A). A row names its prop by
/// placement ordinal, which only means the same prop on two peers when
/// both stamped the same world in the same order; a model that failed
/// to load on one side, or differing content, shifts every later
/// ordinal. The receiver compares this with its own table and refuses
/// the frame's rows on a mismatch. `digest` is opaque to the wire — the
/// consumer defines what it hashes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SiteTable {
    /// How many placements the sender stamped.
    pub count: u32,
    /// A digest of the stamped placements, ordinal by ordinal.
    pub digest: u64,
}

/// One world-prop's replicated state inside a [`Message::Props`] frame
/// (v17, F26-A). The prop is named by its placement `site` — the
/// ordinal the city stamp minted, identical on every process that
/// loads the same content — never by an `ObjectId`, which lives in one
/// process's local namespace. A break fragment adds its index inside
/// the placement's collidable pieces. `phase` is the lifecycle
/// discriminant, opaque to the wire like [`SnapRace::phase`]; the pose
/// is the body's world transform. *State*, not an event: a repeated or
/// dropped row self-corrects on the next.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SnapProp {
    /// The placement ordinal.
    pub site: u32,
    /// The break fragment index, or [`SNAP_NO_FRAGMENT`].
    pub fragment: u8,
    /// Opaque lifecycle discriminant — the consumer names the phases.
    pub phase: u8,
    /// World-space position, metres.
    pub pos: [f32; 3],
    /// World-space orientation quaternion `[x, y, z, w]`.
    pub rot: [f32; 4],
}

/// One wire message.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    /// Client → host hello; always the first frame.
    Hello(Hello),
    /// Host → client acceptance of the handshake.
    Accept,
    /// Host → client refusal, with a reason the UI/CLI can show.
    Reject {
        /// Machine-readable reason.
        code: RejectCode,
        /// Human-readable detail.
        message: String,
    },
    /// Host → client slot assignment, sent right after `Accept`.
    Welcome {
        /// The roster slot the client now owns.
        player_id: u16,
    },
    /// Host → clients: the session this lobby is configured to run.
    /// Broadcast to everyone when the host sets it and sent to each
    /// newcomer between `Welcome` and the roster — like `Roster` it is a
    /// complete snapshot, not a delta, so receivers replace wholesale.
    Session(SessionAdvertisement),
    /// Host → every client: the complete roster after any change. The
    /// roster is authoritative state, not a delta — receivers replace
    /// theirs wholesale.
    Roster {
        /// Every connected player, in slot order.
        players: Vec<RosterEntry>,
    },
    /// Client → host: toggle this player's readiness flag.
    SetReady {
        /// The new readiness state.
        ready: bool,
    },
    /// Client → host: set this player's vehicle pick. The host applies
    /// it through the consumer's pick validator and rebroadcasts the
    /// roster, or answers this peer alone with `VehicleRefused` — a bad
    /// pick is a refused request, not a protocol violation.
    SetVehicle(VehiclePick),
    /// Host → the refused client only: its `SetVehicle` pick failed the
    /// host's validator, with a display-ready reason. The roster is
    /// unchanged — receivers must not treat this as an error that ends
    /// the connection.
    VehicleRefused {
        /// Why the pick was refused.
        reason: String,
    },
    /// Client → host: a clean quit. Distinguishes a deliberate leave from
    /// a dropped connection on the wire.
    Leave,
    /// Host → every client: the session starts now. Carries the
    /// authoritative session the clients must build — the *started*
    /// session, self-contained, so a lobby-side `Session` re-advertised
    /// mid-session cannot confuse a late joiner about which config is
    /// running — plus the session generation the host minted for it.
    /// A newcomer joining an open in-progress session receives this
    /// unicast after its first `Roster`.
    Start {
        /// Host-minted session generation, monotonically increasing per
        /// lobby lifetime from 1 — the value `mm2_game`'s `Session`
        /// generation (and every `ObjectId` minted under it)
        /// namespaces to, so peers agree on which run an id belongs to.
        generation: u64,
        /// The session being started. Same shape and bounds as the
        /// lobby's `Session` broadcast.
        session: SessionAdvertisement,
        /// The host's own vehicle pick when the host process is also a
        /// player (`mm2 --host`); `None` on a dedicated seat-less host.
        /// The roster never carries the host seat — this is how peers
        /// learn what its snapshots' player-0 entity drives (F25-A).
        host_pick: Option<VehiclePick>,
    },
    /// Host → every client: the in-progress session is over — abort or
    /// normal end, the wire does not distinguish; every peer returns to
    /// the lobby, which re-opens for joins. `generation` names the
    /// session being cancelled so a client that never entered one can
    /// ignore a stray message.
    Cancel {
        /// The generation whose session ended.
        generation: u64,
    },
    /// Client → host: one driver input sample (F25-A). The host absorbs
    /// these into a per-player mailbox — they are session data-plane
    /// traffic, not lobby verbs, so they are legal whenever the link is
    /// up (the generation field, not the send timing, decides whether a
    /// sample applies).
    Input(DriveInput),
    /// Client → host: the driver asked to be reset (F25-B). The sender's
    /// roster slot names the seat — a request carries no target, so a
    /// client can only ever ask for its own car. `generation` namespaces
    /// the request like an `Input` sample: a request minted against a
    /// session the host is no longer running is dropped, never applied
    /// to the next one. Session data-plane traffic like `Input` — the
    /// host absorbs it into the mailbox rather than waking the lobby
    /// loop; the answer is the epoch-declared `Snap`, not a reply.
    ResetRequest {
        /// The session generation this request belongs to.
        generation: u64,
    },
    /// Host → every client: an authoritative pose snapshot of the
    /// running session's participants (F25-A). `tick` is the host's
    /// session tick when the snapshot was taken; entries are a complete
    /// set for that tick, bounded by [`MAX_PLAYERS`].
    Snap {
        /// The session generation this snapshot belongs to — a snap for
        /// any other generation is dropped, never replayed.
        generation: u64,
        /// Host session tick at capture — receivers order/discard by it.
        tick: u64,
        /// Every simulated player's pose, wire-id sorted.
        entries: Vec<SnapEntry>,
        /// The trailers the seated picks tow (v9, F25-B) — present only
        /// for trailered seats, bounded by [`MAX_PLAYERS`].
        trailers: Vec<SnapTrailer>,
        /// The participant impacts the authority emitted since the last
        /// snapshot (v10, F25-B) — presentation events, bounded by
        /// [`MAX_SNAP_IMPACTS`].
        impacts: Vec<SnapImpact>,
        /// The authority's race state while the session runs an event
        /// (v13, F25-B) — `None` on a raceless session.
        race: Option<SnapRace>,
    },
    /// Host → every client: the authority's world-prop state (F26-A) —
    /// every pooled/active body plus the freshly changed and a rolling
    /// window of the settled and broken ones, bounded by
    /// [`MAX_SNAP_PROPS`]. `tick` orders frames per prop on the
    /// receiver; `generation` gates it like [`Message::Snap`].
    Props {
        /// The session generation this frame belongs to.
        generation: u64,
        /// Host session tick at capture.
        tick: u64,
        /// The host's stamped-placement summary (v18) — what the row
        /// ordinals are relative to.
        table: SiteTable,
        /// The prop rows.
        rows: Vec<SnapProp>,
    },
}

/// A wire-decode failure on a well-framed payload.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ProtoError {
    /// Unknown message tag.
    #[error("unknown message tag {0:#04x}")]
    BadTag(u8),
    /// Unknown [`RejectCode`].
    #[error("unknown reject code {0}")]
    BadRejectCode(u8),
    /// Fewer bytes than the field needs.
    #[error("truncated field")]
    Truncated,
    /// A string field declared more than [`MAX_STRING`] bytes.
    #[error("string field over {MAX_STRING} bytes")]
    OversizeString,
    /// A string field was not valid UTF-8.
    #[error("string field is not UTF-8")]
    InvalidUtf8,
    /// Bytes left over after the message's last field.
    #[error("{0} trailing bytes")]
    Trailing(usize),
    /// A bool field carried a byte other than 0 or 1.
    #[error("invalid bool byte {0}")]
    InvalidBool(u8),
    /// A roster declared more than [`MAX_PLAYERS`] entries.
    #[error("roster declares {0} players, bound is {MAX_PLAYERS}")]
    OversizeRoster(u8),
    /// A session advertisement's `params` field declared more than
    /// [`MAX_SESSION_PARAMS`] bytes.
    #[error("session params declare {0} bytes, bound is {MAX_SESSION_PARAMS}")]
    OversizeSessionParams(usize),
    /// A snapshot declared more than [`MAX_PLAYERS`] entries.
    #[error("snapshot declares {0} entries, bound is {MAX_PLAYERS}")]
    OversizeSnapshot(u8),
    /// A snapshot declared more than [`MAX_PLAYERS`] trailer entries.
    #[error("snapshot declares {0} trailers, bound is {MAX_PLAYERS}")]
    OversizeTrailers(u8),
    /// A snapshot declared more than [`MAX_SNAP_IMPACTS`] impact rows.
    #[error("snapshot declares {0} impacts, bound is {MAX_SNAP_IMPACTS}")]
    OversizeImpacts(u8),
    /// A props frame declared more than [`MAX_SNAP_PROPS`] rows.
    #[error("props frame declares {0} rows, bound is {MAX_SNAP_PROPS}")]
    OversizeProps(u8),
}

impl RejectCode {
    fn to_u8(self) -> u8 {
        self as u8
    }

    fn from_u8(v: u8) -> Result<Self, ProtoError> {
        match v {
            1 => Ok(Self::VersionMismatch),
            2 => Ok(Self::ContentMismatch),
            3 => Ok(Self::Malformed),
            4 => Ok(Self::LobbyFull),
            5 => Ok(Self::SessionStarted),
            other => Err(ProtoError::BadRejectCode(other)),
        }
    }
}

/// Bounds-checked little-endian decode cursor.
struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], ProtoError> {
        let end = self.pos.checked_add(n).ok_or(ProtoError::Truncated)?;
        let out = self.buf.get(self.pos..end).ok_or(ProtoError::Truncated)?;
        self.pos = end;
        Ok(out)
    }

    fn u8(&mut self) -> Result<u8, ProtoError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, ProtoError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn i16(&mut self) -> Result<i16, ProtoError> {
        Ok(i16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn u32(&mut self) -> Result<u32, ProtoError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> Result<u64, ProtoError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn i64(&mut self) -> Result<i64, ProtoError> {
        Ok(i64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn f32(&mut self) -> Result<f32, ProtoError> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn vec3(&mut self) -> Result<[f32; 3], ProtoError> {
        Ok([self.f32()?, self.f32()?, self.f32()?])
    }

    fn vec4(&mut self) -> Result<[f32; 4], ProtoError> {
        Ok([self.f32()?, self.f32()?, self.f32()?, self.f32()?])
    }

    fn bool(&mut self) -> Result<bool, ProtoError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            other => Err(ProtoError::InvalidBool(other)),
        }
    }

    fn string(&mut self) -> Result<String, ProtoError> {
        let len = self.u16()? as usize;
        if len > MAX_STRING {
            return Err(ProtoError::OversizeString);
        }
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| ProtoError::InvalidUtf8)
    }

    fn finish(self) -> Result<(), ProtoError> {
        let left = self.buf.len() - self.pos;
        if left > 0 {
            Err(ProtoError::Trailing(left))
        } else {
            Ok(())
        }
    }
}

fn put_string(out: &mut Vec<u8>, s: &str) -> Result<(), ProtoError> {
    if s.len() > MAX_STRING {
        return Err(ProtoError::OversizeString);
    }
    out.extend_from_slice(&(s.len() as u16).to_le_bytes());
    out.extend_from_slice(s.as_bytes());
    Ok(())
}

fn put_session(out: &mut Vec<u8>, ad: &SessionAdvertisement) -> Result<(), ProtoError> {
    put_string(out, &ad.summary)?;
    if ad.params.len() > MAX_SESSION_PARAMS {
        return Err(ProtoError::OversizeSessionParams(ad.params.len()));
    }
    out.extend_from_slice(&(ad.params.len() as u16).to_le_bytes());
    out.extend_from_slice(&ad.params);
    Ok(())
}

fn get_session(cur: &mut Cursor<'_>) -> Result<SessionAdvertisement, ProtoError> {
    let summary = cur.string()?;
    let len = cur.u16()? as usize;
    if len > MAX_SESSION_PARAMS {
        return Err(ProtoError::OversizeSessionParams(len));
    }
    Ok(SessionAdvertisement {
        summary,
        params: cur.take(len)?.to_vec(),
    })
}

fn put_opt_pick(out: &mut Vec<u8>, pick: &Option<VehiclePick>) -> Result<(), ProtoError> {
    match pick {
        Some(pick) => {
            out.push(1);
            put_string(out, &pick.vehicle)?;
            out.push(pick.paint);
        }
        None => out.push(0),
    }
    Ok(())
}

fn get_opt_pick(cur: &mut Cursor<'_>) -> Result<Option<VehiclePick>, ProtoError> {
    Ok(if cur.bool()? {
        Some(VehiclePick {
            vehicle: cur.string()?,
            paint: cur.u8()?,
        })
    } else {
        None
    })
}

impl Message {
    /// Serialize into one frame payload.
    pub fn encode(&self) -> Result<Vec<u8>, ProtoError> {
        let mut out = Vec::new();
        match self {
            Self::Hello(h) => {
                out.push(TAG_HELLO);
                out.extend_from_slice(&h.protocol.to_le_bytes());
                out.extend_from_slice(&h.gameplay_fingerprint.to_le_bytes());
                put_string(&mut out, &h.build)?;
                put_string(&mut out, &h.driver)?;
            }
            Self::Accept => out.push(TAG_ACCEPT),
            Self::Reject { code, message } => {
                out.push(TAG_REJECT);
                out.push(code.to_u8());
                put_string(&mut out, message)?;
            }
            Self::Welcome { player_id } => {
                out.push(TAG_WELCOME);
                out.extend_from_slice(&player_id.to_le_bytes());
            }
            Self::Session(ad) => {
                out.push(TAG_SESSION);
                put_session(&mut out, ad)?;
            }
            Self::Roster { players } => {
                out.push(TAG_ROSTER);
                if players.len() > MAX_PLAYERS as usize {
                    return Err(ProtoError::OversizeRoster(players.len() as u8));
                }
                out.push(players.len() as u8);
                for p in players {
                    out.extend_from_slice(&p.player_id.to_le_bytes());
                    out.push(p.ready as u8);
                    put_string(&mut out, &p.driver)?;
                    put_string(&mut out, &p.build)?;
                    match &p.pick {
                        Some(pick) => {
                            out.push(1);
                            put_string(&mut out, &pick.vehicle)?;
                            out.push(pick.paint);
                        }
                        None => out.push(0),
                    }
                }
            }
            Self::SetReady { ready } => {
                out.push(TAG_SET_READY);
                out.push(*ready as u8);
            }
            Self::SetVehicle(pick) => {
                out.push(TAG_SET_VEHICLE);
                put_string(&mut out, &pick.vehicle)?;
                out.push(pick.paint);
            }
            Self::VehicleRefused { reason } => {
                out.push(TAG_VEHICLE_REFUSED);
                put_string(&mut out, reason)?;
            }
            Self::Leave => out.push(TAG_LEAVE),
            Self::Start {
                generation,
                session,
                host_pick,
            } => {
                out.push(TAG_START);
                out.extend_from_slice(&generation.to_le_bytes());
                put_session(&mut out, session)?;
                put_opt_pick(&mut out, host_pick)?;
            }
            Self::Cancel { generation } => {
                out.push(TAG_CANCEL);
                out.extend_from_slice(&generation.to_le_bytes());
            }
            Self::Input(input) => {
                out.push(TAG_INPUT);
                out.extend_from_slice(&input.generation.to_le_bytes());
                out.extend_from_slice(&input.seq.to_le_bytes());
                out.push(input.throttle);
                out.push(input.brake);
                out.push(input.steer as u8);
                out.push(input.handbrake);
            }
            Self::ResetRequest { generation } => {
                out.push(TAG_RESET_REQUEST);
                out.extend_from_slice(&generation.to_le_bytes());
            }
            Self::Snap {
                generation,
                tick,
                entries,
                trailers,
                impacts,
                race,
            } => {
                out.push(TAG_SNAP);
                out.extend_from_slice(&generation.to_le_bytes());
                out.extend_from_slice(&tick.to_le_bytes());
                if entries.len() > MAX_PLAYERS as usize {
                    return Err(ProtoError::OversizeSnapshot(entries.len() as u8));
                }
                out.push(entries.len() as u8);
                for e in entries {
                    out.extend_from_slice(&e.player.to_le_bytes());
                    for v in e
                        .pos
                        .iter()
                        .chain(e.rot.iter())
                        .chain(e.vel.iter())
                        .chain(e.angvel.iter())
                    {
                        out.extend_from_slice(&v.to_le_bytes());
                    }
                    out.push(e.epoch);
                    out.extend_from_slice(&e.steer.to_le_bytes());
                    out.extend_from_slice(&e.spin.to_le_bytes());
                    out.push(e.compression);
                    out.push(e.flags);
                    out.push(e.damage);
                    out.extend_from_slice(&e.breaks.to_le_bytes());
                    out.push(e.prog_state);
                    out.extend_from_slice(&e.prog_ticks.to_le_bytes());
                    out.extend_from_slice(&e.prog_lap.to_le_bytes());
                    out.extend_from_slice(&e.prog_next.to_le_bytes());
                    out.extend_from_slice(&e.prog_cleared.to_le_bytes());
                    out.extend_from_slice(&e.prog_crossings.to_le_bytes());
                    out.extend_from_slice(&e.prog_route_clears.to_le_bytes());
                    out.extend_from_slice(&e.rpm.to_le_bytes());
                    out.extend_from_slice(&e.surf_skid.to_le_bytes());
                    out.push(e.skid_slip);
                    out.extend_from_slice(&e.skid_speed.to_le_bytes());
                    out.extend_from_slice(&e.surf_roll.to_le_bytes());
                }
                if trailers.len() > MAX_PLAYERS as usize {
                    return Err(ProtoError::OversizeTrailers(trailers.len() as u8));
                }
                out.push(trailers.len() as u8);
                for t in trailers {
                    out.extend_from_slice(&t.owner.to_le_bytes());
                    for v in t
                        .pos
                        .iter()
                        .chain(t.rot.iter())
                        .chain(t.vel.iter())
                        .chain(t.angvel.iter())
                    {
                        out.extend_from_slice(&v.to_le_bytes());
                    }
                    out.extend_from_slice(&t.spin.to_le_bytes());
                    out.push(t.flags);
                }
                if impacts.len() > MAX_SNAP_IMPACTS as usize {
                    return Err(ProtoError::OversizeImpacts(impacts.len() as u8));
                }
                out.push(impacts.len() as u8);
                for m in impacts {
                    out.extend_from_slice(&m.seat.to_le_bytes());
                    out.extend_from_slice(&m.id.to_le_bytes());
                    out.extend_from_slice(&m.tick.to_le_bytes());
                    for v in m.point.iter().chain(m.normal.iter()) {
                        out.extend_from_slice(&v.to_le_bytes());
                    }
                    out.extend_from_slice(&m.severity.to_le_bytes());
                    out.extend_from_slice(&m.audio_id.to_le_bytes());
                }
                match race {
                    Some(race) => {
                        out.push(1);
                        out.push(race.phase);
                        out.extend_from_slice(&race.countdown.to_le_bytes());
                        out.extend_from_slice(&race.clock.to_le_bytes());
                    }
                    None => out.push(0),
                }
            }
            Self::Props {
                generation,
                tick,
                table,
                rows,
            } => {
                out.push(TAG_PROPS);
                out.extend_from_slice(&generation.to_le_bytes());
                out.extend_from_slice(&tick.to_le_bytes());
                out.extend_from_slice(&table.count.to_le_bytes());
                out.extend_from_slice(&table.digest.to_le_bytes());
                if rows.len() > MAX_SNAP_PROPS as usize {
                    // Saturate the reported count so an absurd length
                    // never wraps into a small, plausible-looking one.
                    return Err(ProtoError::OversizeProps(
                        rows.len().min(u8::MAX as usize) as u8
                    ));
                }
                out.push(rows.len() as u8);
                for row in rows {
                    out.extend_from_slice(&row.site.to_le_bytes());
                    out.push(row.fragment);
                    out.push(row.phase);
                    for v in row.pos.iter().chain(row.rot.iter()) {
                        out.extend_from_slice(&v.to_le_bytes());
                    }
                }
            }
        }
        Ok(out)
    }

    /// Parse one frame payload. Strict: every byte must be consumed.
    pub fn decode(payload: &[u8]) -> Result<Self, ProtoError> {
        let mut cur = Cursor::new(payload);
        let msg = match cur.u8()? {
            TAG_HELLO => Self::Hello(Hello {
                protocol: cur.u16()?,
                gameplay_fingerprint: cur.u64()?,
                build: cur.string()?,
                driver: cur.string()?,
            }),
            TAG_ACCEPT => Self::Accept,
            TAG_REJECT => Self::Reject {
                code: RejectCode::from_u8(cur.u8()?)?,
                message: cur.string()?,
            },
            TAG_WELCOME => Self::Welcome {
                player_id: cur.u16()?,
            },
            TAG_SESSION => Self::Session(get_session(&mut cur)?),
            TAG_ROSTER => {
                let count = cur.u8()?;
                if count > MAX_PLAYERS {
                    return Err(ProtoError::OversizeRoster(count));
                }
                let mut players = Vec::with_capacity(count as usize);
                for _ in 0..count {
                    players.push(RosterEntry {
                        player_id: cur.u16()?,
                        ready: cur.bool()?,
                        driver: cur.string()?,
                        build: cur.string()?,
                        pick: if cur.bool()? {
                            Some(VehiclePick {
                                vehicle: cur.string()?,
                                paint: cur.u8()?,
                            })
                        } else {
                            None
                        },
                    });
                }
                Self::Roster { players }
            }
            TAG_SET_READY => Self::SetReady { ready: cur.bool()? },
            TAG_SET_VEHICLE => Self::SetVehicle(VehiclePick {
                vehicle: cur.string()?,
                paint: cur.u8()?,
            }),
            TAG_VEHICLE_REFUSED => Self::VehicleRefused {
                reason: cur.string()?,
            },
            TAG_LEAVE => Self::Leave,
            TAG_START => Self::Start {
                generation: cur.u64()?,
                session: get_session(&mut cur)?,
                host_pick: get_opt_pick(&mut cur)?,
            },
            TAG_CANCEL => Self::Cancel {
                generation: cur.u64()?,
            },
            TAG_INPUT => Self::Input(DriveInput {
                generation: cur.u64()?,
                seq: cur.u64()?,
                throttle: cur.u8()?,
                brake: cur.u8()?,
                steer: cur.u8()? as i8,
                handbrake: cur.u8()?,
            }),
            TAG_RESET_REQUEST => Self::ResetRequest {
                generation: cur.u64()?,
            },
            TAG_SNAP => {
                let generation = cur.u64()?;
                let tick = cur.u64()?;
                let count = cur.u8()?;
                if count > MAX_PLAYERS {
                    return Err(ProtoError::OversizeSnapshot(count));
                }
                let mut entries = Vec::with_capacity(count as usize);
                for _ in 0..count {
                    entries.push(SnapEntry {
                        player: cur.u16()?,
                        pos: cur.vec3()?,
                        rot: [cur.f32()?, cur.f32()?, cur.f32()?, cur.f32()?],
                        vel: cur.vec3()?,
                        angvel: cur.vec3()?,
                        epoch: cur.u8()?,
                        steer: cur.i16()?,
                        spin: cur.i16()?,
                        compression: cur.u8()?,
                        flags: cur.u8()?,
                        damage: cur.u8()?,
                        breaks: cur.u32()?,
                        prog_state: cur.u8()?,
                        prog_ticks: cur.u64()?,
                        prog_lap: cur.u32()?,
                        prog_next: cur.u32()?,
                        prog_cleared: cur.u64()?,
                        prog_crossings: cur.u32()?,
                        prog_route_clears: cur.u32()?,
                        rpm: cur.u16()?,
                        surf_skid: cur.u16()?,
                        skid_slip: cur.u8()?,
                        skid_speed: cur.i16()?,
                        surf_roll: cur.u16()?,
                    });
                }
                let trailer_count = cur.u8()?;
                if trailer_count > MAX_PLAYERS {
                    return Err(ProtoError::OversizeTrailers(trailer_count));
                }
                let mut trailers = Vec::with_capacity(trailer_count as usize);
                for _ in 0..trailer_count {
                    trailers.push(SnapTrailer {
                        owner: cur.u16()?,
                        pos: cur.vec3()?,
                        rot: [cur.f32()?, cur.f32()?, cur.f32()?, cur.f32()?],
                        vel: cur.vec3()?,
                        angvel: cur.vec3()?,
                        spin: cur.i16()?,
                        flags: cur.u8()?,
                    });
                }
                let impact_count = cur.u8()?;
                if impact_count > MAX_SNAP_IMPACTS {
                    return Err(ProtoError::OversizeImpacts(impact_count));
                }
                let mut impacts = Vec::with_capacity(impact_count as usize);
                for _ in 0..impact_count {
                    impacts.push(SnapImpact {
                        seat: cur.u16()?,
                        id: cur.u64()?,
                        tick: cur.u64()?,
                        point: cur.vec3()?,
                        normal: cur.vec3()?,
                        severity: cur.f32()?,
                        audio_id: cur.i64()?,
                    });
                }
                // `phase` decodes verbatim — the discriminant's naming
                // lives in `mm2_app`, which drops what it cannot name.
                let race = if cur.bool()? {
                    Some(SnapRace {
                        phase: cur.u8()?,
                        countdown: cur.u32()?,
                        clock: cur.u64()?,
                    })
                } else {
                    None
                };
                Self::Snap {
                    generation,
                    tick,
                    entries,
                    trailers,
                    impacts,
                    race,
                }
            }
            TAG_PROPS => {
                let generation = cur.u64()?;
                let tick = cur.u64()?;
                let table = SiteTable {
                    count: cur.u32()?,
                    digest: cur.u64()?,
                };
                let count = cur.u8()?;
                if count > MAX_SNAP_PROPS {
                    return Err(ProtoError::OversizeProps(count));
                }
                let mut rows = Vec::with_capacity(count as usize);
                for _ in 0..count {
                    rows.push(SnapProp {
                        site: cur.u32()?,
                        fragment: cur.u8()?,
                        phase: cur.u8()?,
                        pos: cur.vec3()?,
                        rot: cur.vec4()?,
                    });
                }
                Self::Props {
                    generation,
                    tick,
                    table,
                    rows,
                }
            }
            tag => return Err(ProtoError::BadTag(tag)),
        };
        cur.finish()?;
        Ok(msg)
    }
}

/// The host-side compatibility gate: whether a `Hello` is allowed in.
/// `Ok(())` accepts; `Err((code, message))` is the refusal to send back.
/// Pure so the decision is testable without a socket.
pub fn admit(hello: &Hello, gameplay_fingerprint: u64) -> Result<(), (RejectCode, String)> {
    if hello.protocol != PROTOCOL_VERSION {
        return Err((
            RejectCode::VersionMismatch,
            format!(
                "protocol {PROTOCOL_VERSION} required, client offered {}",
                hello.protocol
            ),
        ));
    }
    if hello.gameplay_fingerprint != gameplay_fingerprint {
        return Err((
            RejectCode::ContentMismatch,
            "gameplay content differs (tuning/bounds/geometry/events)".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hello() -> Hello {
        Hello {
            protocol: PROTOCOL_VERSION,
            gameplay_fingerprint: 0xdead_beef,
            build: "test".to_string(),
            driver: "driver one".to_string(),
        }
    }

    #[test]
    fn roundtrip_all_variants() {
        for msg in [
            Message::Hello(hello()),
            Message::Accept,
            Message::Reject {
                code: RejectCode::ContentMismatch,
                message: "content differs".to_string(),
            },
            Message::Reject {
                code: RejectCode::SessionStarted,
                message: "session already started".to_string(),
            },
            Message::Welcome { player_id: 3 },
            Message::Session(SessionAdvertisement {
                summary: "sf, cruise, amateur".to_string(),
                params: vec![1, 2, 3, 4],
            }),
            Message::Session(SessionAdvertisement {
                summary: String::new(),
                params: Vec::new(),
            }),
            Message::Roster {
                players: vec![
                    RosterEntry {
                        player_id: 1,
                        driver: "driver one".to_string(),
                        build: "test".to_string(),
                        ready: false,
                        pick: Some(VehiclePick {
                            vehicle: "vpbug".to_string(),
                            paint: 2,
                        }),
                    },
                    RosterEntry {
                        player_id: 2,
                        driver: "driver two".to_string(),
                        build: "test".to_string(),
                        ready: true,
                        pick: None,
                    },
                ],
            },
            Message::SetReady { ready: true },
            Message::SetVehicle(VehiclePick {
                vehicle: "vpbug".to_string(),
                paint: 0,
            }),
            Message::SetVehicle(VehiclePick {
                vehicle: String::new(),
                paint: 0,
            }),
            Message::VehicleRefused {
                reason: "unknown vehicle id".to_string(),
            },
            Message::Leave,
            Message::Start {
                generation: 7,
                session: SessionAdvertisement {
                    summary: "sf, cruise, amateur".to_string(),
                    params: vec![9, 8, 7],
                },
                host_pick: Some(VehiclePick {
                    vehicle: "vpbug".to_string(),
                    paint: 0,
                }),
            },
            // A seat-less host's start — `host_pick: None`.
            Message::Start {
                generation: 8,
                session: SessionAdvertisement {
                    summary: "sf, cruise, amateur".to_string(),
                    params: vec![1],
                },
                host_pick: None,
            },
            Message::Cancel { generation: 7 },
            Message::Input(DriveInput {
                generation: 7,
                seq: 240,
                throttle: 255,
                brake: 0,
                steer: -64,
                handbrake: 12,
            }),
            Message::ResetRequest { generation: 7 },
            Message::ResetRequest {
                generation: u64::MAX,
            },
            Message::Snap {
                generation: 7,
                tick: 480,
                entries: vec![
                    SnapEntry {
                        player: 0,
                        pos: [1.0, 2.5, -3.25],
                        rot: [0.0, 0.707, 0.0, 0.707],
                        vel: [12.5, 0.0, -1.0],
                        angvel: [0.0, 0.4, 0.0],
                        epoch: 2,
                        steer: -310,
                        spin: 1420,
                        compression: 96,
                        flags: SNAP_FLAG_BRAKE | SNAP_FLAG_GROUNDED,
                        damage: 128,
                        breaks: 0b0101,
                        prog_state: 2,
                        prog_ticks: 4200,
                        prog_lap: 1,
                        prog_next: 3,
                        prog_cleared: 0b101,
                        prog_crossings: 9,
                        prog_route_clears: 2,
                        rpm: 4321,
                        surf_skid: 1,
                        skid_slip: 179,
                        skid_speed: -124,
                        surf_roll: 1,
                    },
                    SnapEntry {
                        player: 3,
                        pos: [-9.0, 1.0, 0.5],
                        rot: [0.0, 0.0, 0.0, 1.0],
                        vel: [0.0, 0.0, 0.0],
                        angvel: [0.0, 0.0, 0.0],
                        epoch: 0,
                        steer: 0,
                        spin: -80,
                        compression: 0,
                        flags: SNAP_FLAG_REVERSE,
                        damage: 0,
                        breaks: 0,
                        prog_state: 1,
                        prog_ticks: 0,
                        prog_lap: 0,
                        prog_next: 1,
                        prog_cleared: 0,
                        prog_crossings: 0,
                        prog_route_clears: 0,
                        rpm: 900,
                        surf_skid: SNAP_NO_SURFACE,
                        skid_slip: 0,
                        skid_speed: 0,
                        surf_roll: SNAP_NO_SURFACE,
                    },
                ],
                trailers: vec![
                    SnapTrailer {
                        owner: 0,
                        pos: [1.0, 2.0, 5.9],
                        rot: [0.0, 0.707, 0.0, 0.707],
                        vel: [12.5, 0.0, -1.0],
                        angvel: [0.0, 0.4, 0.0],
                        spin: 1420,
                        flags: SNAP_FLAG_GROUNDED,
                    },
                    SnapTrailer {
                        owner: 3,
                        pos: [-9.0, 0.5, 6.0],
                        rot: [0.0, 0.0, 0.0, 1.0],
                        vel: [0.0, 0.0, 0.0],
                        angvel: [0.0, 0.0, 0.0],
                        spin: -80,
                        flags: 0,
                    },
                ],
                impacts: vec![
                    SnapImpact {
                        seat: 1,
                        id: 7,
                        tick: 2,
                        point: [3.0, 0.4, -1.0],
                        normal: [0.0, 0.0, 1.0],
                        severity: 12.5,
                        audio_id: 7,
                    },
                    SnapImpact {
                        seat: 3,
                        id: 7,
                        tick: 2,
                        point: [3.0, 0.4, -1.0],
                        normal: [0.0, 0.0, -1.0],
                        severity: 12.5,
                        audio_id: 0,
                    },
                ],
                race: Some(SnapRace {
                    phase: 0,
                    countdown: 180,
                    clock: 0,
                }),
            },
            // A raceless session's snap — `race: None`.
            Message::Snap {
                generation: 7,
                tick: 481,
                entries: Vec::new(),
                trailers: Vec::new(),
                impacts: Vec::new(),
                race: None,
            },
        ] {
            let bytes = msg.encode().unwrap();
            assert_eq!(Message::decode(&bytes).unwrap(), msg);
        }
    }

    #[test]
    fn decode_rejects_garbage() {
        assert!(matches!(
            Message::decode(&[0xff]),
            Err(ProtoError::BadTag(0xff))
        ));
        // Truncated hello: tag + partial protocol field.
        assert!(matches!(
            Message::decode(&[TAG_HELLO, 0x01]),
            Err(ProtoError::Truncated)
        ));
        // String length beyond the cap.
        let mut bad = vec![TAG_HELLO];
        bad.extend_from_slice(&1u16.to_le_bytes());
        bad.extend_from_slice(&0u64.to_le_bytes());
        bad.extend_from_slice(&1000u16.to_le_bytes());
        assert!(matches!(
            Message::decode(&bad),
            Err(ProtoError::OversizeString)
        ));
        // Trailing bytes are a violation.
        let mut extra = Message::Accept.encode().unwrap();
        extra.push(0);
        assert!(matches!(
            Message::decode(&extra),
            Err(ProtoError::Trailing(1))
        ));
        // Unknown reject code.
        assert!(matches!(
            Message::decode(&[TAG_REJECT, 99]),
            Err(ProtoError::BadRejectCode(99))
        ));
        // A roster declaring more than the player ceiling.
        assert!(matches!(
            Message::decode(&[TAG_ROSTER, MAX_PLAYERS + 1]),
            Err(ProtoError::OversizeRoster(9))
        ));
        // Session params declaring more than the bound.
        let mut bad_session = vec![TAG_SESSION];
        bad_session.extend_from_slice(&1u16.to_le_bytes()); // summary len
        bad_session.push(b's');
        bad_session.extend_from_slice(&(MAX_SESSION_PARAMS as u16 + 1).to_le_bytes());
        assert!(matches!(
            Message::decode(&bad_session),
            Err(ProtoError::OversizeSessionParams(4097))
        ));
        // A bool field byte other than 0/1.
        assert!(matches!(
            Message::decode(&[TAG_SET_READY, 2]),
            Err(ProtoError::InvalidBool(2))
        ));
        // A snapshot declaring more than the player ceiling.
        let mut bad_snap = vec![TAG_SNAP];
        bad_snap.extend_from_slice(&1u64.to_le_bytes());
        bad_snap.extend_from_slice(&2u64.to_le_bytes());
        bad_snap.push(MAX_PLAYERS + 1);
        assert!(matches!(
            Message::decode(&bad_snap),
            Err(ProtoError::OversizeSnapshot(9))
        ));
        // A truncated snapshot entry ends in `Truncated`, not a partial pose.
        let mut short_snap = vec![TAG_SNAP];
        short_snap.extend_from_slice(&1u64.to_le_bytes());
        short_snap.extend_from_slice(&2u64.to_le_bytes());
        short_snap.push(1);
        short_snap.extend_from_slice(&[0; 10]);
        assert!(matches!(
            Message::decode(&short_snap),
            Err(ProtoError::Truncated)
        ));
        // A snapshot declaring more trailers than the player ceiling.
        let mut wide_snap = vec![TAG_SNAP];
        wide_snap.extend_from_slice(&1u64.to_le_bytes());
        wide_snap.extend_from_slice(&2u64.to_le_bytes());
        wide_snap.push(0);
        wide_snap.push(MAX_PLAYERS + 1);
        assert!(matches!(
            Message::decode(&wide_snap),
            Err(ProtoError::OversizeTrailers(9))
        ));
        // A snapshot declaring more impacts than the row bound.
        let mut hot_snap = vec![TAG_SNAP];
        hot_snap.extend_from_slice(&1u64.to_le_bytes());
        hot_snap.extend_from_slice(&2u64.to_le_bytes());
        hot_snap.push(0);
        hot_snap.push(0);
        hot_snap.push(MAX_SNAP_IMPACTS + 1);
        assert!(matches!(
            Message::decode(&hot_snap),
            Err(ProtoError::OversizeImpacts(65))
        ));
    }

    fn prop_row(site: u32, fragment: u8) -> SnapProp {
        SnapProp {
            site,
            fragment,
            phase: 1,
            pos: [1.5, -2.0, 300.25],
            rot: [0.0, 0.5, 0.0, 0.5],
        }
    }

    #[test]
    fn a_props_frame_round_trips() {
        let msg = Message::Props {
            generation: 7,
            tick: 4096,
            table: SiteTable {
                count: 41_001,
                digest: 0xfeed_beef_0123_4567,
            },
            rows: vec![prop_row(0, SNAP_NO_FRAGMENT), prop_row(41_000, 2)],
        };
        assert_eq!(Message::decode(&msg.encode().unwrap()).unwrap(), msg);
        let empty = Message::Props {
            generation: 1,
            tick: 0,
            table: SiteTable::default(),
            rows: Vec::new(),
        };
        assert_eq!(Message::decode(&empty.encode().unwrap()).unwrap(), empty);
    }

    #[test]
    fn a_props_frame_is_bounded_both_ways() {
        let full = Message::Props {
            generation: 1,
            tick: 2,
            table: SiteTable::default(),
            rows: (0..MAX_SNAP_PROPS as u32)
                .map(|i| prop_row(i, SNAP_NO_FRAGMENT))
                .collect(),
        };
        assert_eq!(Message::decode(&full.encode().unwrap()).unwrap(), full);
        let over = Message::Props {
            generation: 1,
            tick: 2,
            table: SiteTable::default(),
            rows: (0..=MAX_SNAP_PROPS as u32)
                .map(|i| prop_row(i, SNAP_NO_FRAGMENT))
                .collect(),
        };
        assert_eq!(over.encode(), Err(ProtoError::OversizeProps(97)));
        // A length that would wrap a u8 still reports a large count.
        let huge = Message::Props {
            generation: 1,
            tick: 2,
            table: SiteTable::default(),
            rows: (0..300).map(|i| prop_row(i, 0)).collect(),
        };
        assert_eq!(huge.encode(), Err(ProtoError::OversizeProps(255)));
        let mut wide = vec![TAG_PROPS];
        wide.extend_from_slice(&1u64.to_le_bytes());
        wide.extend_from_slice(&2u64.to_le_bytes());
        wide.extend_from_slice(&0u32.to_le_bytes());
        wide.extend_from_slice(&0u64.to_le_bytes());
        wide.push(MAX_SNAP_PROPS + 1);
        assert_eq!(Message::decode(&wide), Err(ProtoError::OversizeProps(97)));
    }

    #[test]
    fn a_truncated_or_padded_props_frame_is_refused() {
        let msg = Message::Props {
            generation: 3,
            tick: 9,
            table: SiteTable::default(),
            rows: vec![prop_row(5, SNAP_NO_FRAGMENT)],
        };
        let bytes = msg.encode().unwrap();
        for cut in 1..bytes.len() {
            assert!(
                Message::decode(&bytes[..cut]).is_err(),
                "a {cut}-byte prefix decoded"
            );
        }
        let mut padded = bytes;
        padded.push(0);
        assert!(Message::decode(&padded).is_err());
    }

    #[test]
    fn an_oversize_roster_does_not_encode() {
        let players = vec![
            RosterEntry {
                player_id: 0,
                driver: "d".to_string(),
                build: "b".to_string(),
                ready: false,
                pick: None,
            };
            MAX_PLAYERS as usize + 1
        ];
        assert!(matches!(
            Message::Roster { players }.encode(),
            Err(ProtoError::OversizeRoster(9))
        ));
    }

    /// An over-crowded snapshot is refused at encode like the roster.
    #[test]
    fn an_oversize_snapshot_does_not_encode() {
        let entries = vec![
            SnapEntry {
                player: 0,
                pos: [0.0; 3],
                rot: [0.0, 0.0, 0.0, 1.0],
                vel: [0.0; 3],
                angvel: [0.0; 3],
                epoch: 0,
                steer: 0,
                spin: 0,
                compression: 0,
                flags: 0,
                damage: 0,
                breaks: 0,
                prog_state: 0,
                prog_ticks: 0,
                prog_lap: 0,
                prog_next: 0,
                prog_cleared: 0,
                prog_crossings: 0,
                prog_route_clears: 0,
                rpm: 0,
                surf_skid: SNAP_NO_SURFACE,
                skid_slip: 0,
                skid_speed: 0,
                surf_roll: SNAP_NO_SURFACE,
            };
            MAX_PLAYERS as usize + 1
        ];
        assert!(matches!(
            Message::Snap {
                generation: 1,
                tick: 1,
                entries,
                trailers: Vec::new(),
                impacts: Vec::new(),
                race: None,
            }
            .encode(),
            Err(ProtoError::OversizeSnapshot(9))
        ));
        // …and the trailer list is bounded by the same ceiling.
        let trailers = vec![
            SnapTrailer {
                owner: 0,
                pos: [0.0; 3],
                rot: [0.0, 0.0, 0.0, 1.0],
                vel: [0.0; 3],
                angvel: [0.0; 3],
                spin: 0,
                flags: 0,
            };
            MAX_PLAYERS as usize + 1
        ];
        assert!(matches!(
            Message::Snap {
                generation: 1,
                tick: 1,
                entries: Vec::new(),
                trailers,
                impacts: Vec::new(),
                race: None,
            }
            .encode(),
            Err(ProtoError::OversizeTrailers(9))
        ));
        // …and the impact tail is bounded by its own row ceiling.
        let impacts = vec![
            SnapImpact {
                seat: 0,
                id: 1,
                tick: 1,
                point: [0.0; 3],
                normal: [0.0, 1.0, 0.0],
                severity: 1.0,
                audio_id: 0,
            };
            MAX_SNAP_IMPACTS as usize + 1
        ];
        assert!(matches!(
            Message::Snap {
                generation: 1,
                tick: 1,
                entries: Vec::new(),
                trailers: Vec::new(),
                impacts,
                race: None,
            }
            .encode(),
            Err(ProtoError::OversizeImpacts(65))
        ));
    }

    #[test]
    fn an_oversize_session_does_not_encode() {
        let msg = Message::Session(SessionAdvertisement {
            summary: "s".to_string(),
            params: vec![0; MAX_SESSION_PARAMS + 1],
        });
        assert!(matches!(
            msg.encode(),
            Err(ProtoError::OversizeSessionParams(4097))
        ));
        // `Start` carries a session payload under the same bound.
        let msg = Message::Start {
            generation: 1,
            session: SessionAdvertisement {
                summary: "s".to_string(),
                params: vec![0; MAX_SESSION_PARAMS + 1],
            },
            host_pick: None,
        };
        assert!(matches!(
            msg.encode(),
            Err(ProtoError::OversizeSessionParams(4097))
        ));
    }

    #[test]
    fn admit_gates_version_and_content() {
        assert_eq!(admit(&hello(), 0xdead_beef), Ok(()));
        let mut wrong_version = hello();
        wrong_version.protocol = PROTOCOL_VERSION + 1;
        assert!(matches!(
            admit(&wrong_version, 0xdead_beef),
            Err((RejectCode::VersionMismatch, _))
        ));
        let mut wrong_content = hello();
        wrong_content.gameplay_fingerprint += 1;
        assert!(matches!(
            admit(&wrong_content, 0xdead_beef),
            Err((RejectCode::ContentMismatch, _))
        ));
    }
}
