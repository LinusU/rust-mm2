//! The remaining HUD-2 race instruments (F22-A.6): the **place
//! indicator**, the **laps record** (`Ordered`/Circuit rules) and the
//! **checkpoint list** — the last members of the documented
//! instrument set still rendered only inside the dev telemetry line.
//!
//! What is original here: the instruments themselves (HUD-2 names
//! them verbatim) and their *data* contract — every read comes from
//! authoritative race state: [`RaceProgress`] cleared flags and lap
//! counters, [`live_order`]'s running order for the place, and the
//! navigation target (`TargetSelection::picked` respected, so the
//! list's armed entry matches the RACE-6 arrow) for the armed gate.
//! The numbers render the authored `digitac_*_half` glyph tiles the
//! installation ships — bound through the same `city::load_image`
//! VFS path every other instrument takes; a missing glyph aborts the
//! whole cluster (`absent:missing-glyphs`), never substitute art.
//!
//! What stays designed (DSN-54; the original layout is unrecovered —
//! UNK-34): the install ships no authored label art for these
//! instruments — the `race_*` TGA tiles turn out to be menu/results
//! screens' photographic button panels ("Laps", "Opponents", "Race
//! Records", "Select Vehicle"), not in-race instrument labels — so
//! the cluster composes the half-size authored digits with small
//! dev-font labels in a top-right column:
//!
//! - `PLACE n/total` — the local participant's live standing, only
//!   while a field of ≥2 participants races (the `pos=` contract,
//!   DSN-13);
//! - `LAP n/total` — `Ordered` definitions only (HUD-2 scopes the
//!   laps record to Circuit);
//! - `CHECKPOINTS` — one row per authored gate showing its 1-based
//!   index: dim once cleared, bright on the armed objective (the
//!   next required gate under `Ordered`, the arrow's target under
//!   `AnyOrder`), plain otherwise. An authored `finish` adds a `FIN`
//!   entry that stays dim until every gate clears and it arms
//!   (RACE-7).
//!
//! Layer membership is `mmHUD`'s like the rest of F22-A: the `H` gate
//! hides the cluster while the [`RaceStatReport`] keeps composing
//! demand, and `camera::retarget_hud` pins the root to the active
//! world camera.

use avian3d::prelude::Position;
use bevy::prelude::*;
use mm2_assets::Vfs;
use mm2_game::{
    CheckpointRule, NavTarget, ParticipantState, Player, PlayerControl, PlayerVehicle, RacePhase,
    RaceProgress, RaceState, Session, SessionEntity, TargetSelection, live_order,
    navigation_target,
};

/// The glyph set the cluster binds — the authored half-size
/// `digitac_*_half` digits (20×27, alpha-keyed) the timer already
/// uses for its centisecond field.
pub const STAT_GLYPH_COUNT: usize = 10;

/// Top/right inset of the cluster — the corner opposite the authored
/// map inset (`mmhudmap` Pos+Size anchors it bottom-right on both
/// cities). Designed placement (UNK-34).
const STAT_TOP_PX: f32 = 8.0;
const STAT_RIGHT_PX: f32 = 10.0;

/// Backing-plate tint — the same translucent wash the timer plate
/// carries (its `digi_colon` tile's authored background).
const STAT_PANEL: Color = Color::srgba(0.19, 0.188, 0.192, 0.82);

/// Small dev-font labels — muted so the authored digits read first.
const STAT_LABEL: Color = Color::srgb(0.72, 0.78, 0.72);

/// Gate-list states (designed palette, DSN-54): an un-cleared gate
/// shows its authored digits verbatim, a cleared one dims, and the
/// armed objective lights warm — the same attention cue the arrow's
/// authored yellow tile carries. Public so tests can assert the
/// states directly.
pub const GATE_PENDING: Color = Color::srgb(1.0, 1.0, 1.0);
/// See [`GATE_PENDING`].
pub const GATE_CLEARED: Color = Color::srgba(0.5, 0.5, 0.5, 0.45);
/// See [`GATE_PENDING`].
pub const GATE_ARMED: Color = Color::srgb(1.0, 0.85, 0.3);

/// Marker on the session-owned standings root — a right-aligned
/// column the drive pass fills with authored glyph images.
/// SessionEntity-stamped through `owner`; `camera::retarget_hud`
/// writes its `UiTargetCamera` like every other HUD-layer root.
#[derive(Component)]
pub struct RaceStats;

/// The bound digit bank, carried on the [`RaceStats`] root.
#[derive(Component)]
pub struct StatDigits {
    /// `digitac_0_half..9_half`.
    pub half: [Handle<Image>; 10],
}

/// Which `n/total` row a digit cell belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairKind {
    /// The place indicator's digits.
    Place,
    /// The laps record's digits.
    Lap,
}

/// One row of the cluster — hidden (`Display::None`) whenever its
/// instrument has nothing live to say.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatRow {
    /// `PLACE n/total`.
    Place,
    /// `LAP n/total` — spawned only under `Ordered` rules.
    Lap,
}

/// One authored digit cell inside an `n/total` pair row: slots
/// `[v_tens, v_units, t_tens, t_units]`, leading tens collapsed.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct PairDigit {
    /// Which row the cell belongs to.
    pub kind: PairKind,
    /// Position in [`pair_slots`]' four-cell layout.
    pub slot: usize,
}

/// One authored digit cell of a checkpoint-list entry: `gate` is the
/// authored checkpoint index, `slot` its `[tens, units]` position.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct GateDigit {
    /// Authored checkpoint index (the list shows `gate + 1`).
    pub gate: usize,
    /// `0` = tens, `1` = units.
    pub slot: usize,
}

/// The checkpoint list's `FIN` entry — a dev-font label (the digit
/// set has no letters) tinted like the gate rows: dim while gates
/// remain, lit once the finish arms.
#[derive(Component)]
pub struct FinishLabel;

/// What one digit cell shows — public so tests can check the
/// composed layout directly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatGlyph {
    /// Collapsed — a leading tens zero takes no space.
    Off,
    /// A `digitac_*_half` digit.
    Digit(u8),
}

/// Split one `value/total` pair into the row's four digit cells:
/// unpadded tens (leading slots `Off`), both values capped at 99 —
/// the half set has two cells per number.
pub fn pair_slots(value: u32, total: u32) -> [StatGlyph; 4] {
    let tens = |v: u32, slot: usize| {
        if slot == 0 {
            if v >= 10 {
                StatGlyph::Digit((v / 10) as u8)
            } else {
                StatGlyph::Off
            }
        } else {
            StatGlyph::Digit((v % 10) as u8)
        }
    };
    let (v, t) = (value.min(99), total.min(99));
    [tens(v, 0), tens(v, 1), tens(t, 0), tens(t, 1)]
}

/// Split a gate's 1-based index into its two list cells.
pub fn gate_slots(index: usize) -> [StatGlyph; 2] {
    let v = (index as u32 + 1).min(99);
    [
        if v >= 10 {
            StatGlyph::Digit((v / 10) as u8)
        } else {
            StatGlyph::Off
        },
        StatGlyph::Digit((v % 10) as u8),
    ]
}

/// The row, pair-cell, gate-cell and finish-label views the drive
/// pass switches between — disjoint by marker so they can run as one
/// `ParamSet`.
type StatRows<'w, 's> = Query<'w, 's, (&'static StatRow, &'static mut Node), Without<PairDigit>>;
type PairCells<'w, 's> = Query<
    'w,
    's,
    (
        &'static PairDigit,
        &'static mut ImageNode,
        &'static mut Node,
    ),
    Without<GateDigit>,
>;
type GateCells<'w, 's> =
    Query<'w, 's, (&'static GateDigit, &'static mut ImageNode), Without<PairDigit>>;
type FinishCells<'w, 's> = Query<'w, 's, &'static mut TextColor, With<FinishLabel>>;

/// Session-scoped report for the cluster — the `sta=` record field's
/// source. Inserted for every event session (bound or not — `absent`
/// says why not); cruise and dev-world sessions get no report, so
/// their records stay bit-identical. Each instrument records its own
/// demand while the `H` gate hides the cluster — the same contract
/// `tmr=`/`arr=` keep.
#[derive(Resource, Default)]
pub struct RaceStatReport {
    /// Digit images bound — the full authored set is
    /// [`STAT_GLYPH_COUNT`].
    pub glyphs: usize,
    /// Why the cluster did not bind: `missing-glyphs` (absent or
    /// undecodable artwork — never substituted).
    pub absent: Option<&'static str>,
    /// The place row's `(place, field)` on the last drive pass.
    pub place: Option<(u32, u32)>,
    /// The lap row's `(lap, total)` on the last drive pass.
    pub lap: Option<(u32, u32)>,
    /// The checkpoint list's `(cleared, total)` on the last pass.
    pub checkpoints: Option<(usize, usize)>,
}

impl RaceStatReport {
    /// The `sta=` record field body: `<glyphs>g/p<n>of<m>/l<n>of<m>/
    /// c<n>of<m>` — `-` for an instrument with nothing to show — or
    /// `absent:<why>` when the glyph set never loaded.
    pub fn smoke_detail(&self) -> String {
        if let Some(why) = self.absent {
            return format!("absent:{why}");
        }
        let p = self
            .place
            .map(|(a, b)| format!("p{a}of{b}"))
            .unwrap_or_else(|| "-".to_string());
        let l = self
            .lap
            .map(|(a, b)| format!("l{a}of{b}"))
            .unwrap_or_else(|| "-".to_string());
        let c = self
            .checkpoints
            .map(|(a, b)| format!("c{a}of{b}"))
            .unwrap_or_else(|| "-".to_string());
        format!("{}g/{p}/{l}/{c}", self.glyphs)
    }
}

/// The `digitac_*_half` stem for digit `d`.
const fn half_stem(d: usize) -> &'static str {
    const HALF: [&str; 10] = [
        "digitac_0_half",
        "digitac_1_half",
        "digitac_2_half",
        "digitac_3_half",
        "digitac_4_half",
        "digitac_5_half",
        "digitac_6_half",
        "digitac_7_half",
        "digitac_8_half",
        "digitac_9_half",
    ];
    HALF[d]
}

fn label(text: &'static str) -> impl Bundle {
    (
        Text::new(text),
        TextFont {
            font_size: bevy::text::FontSize::Px(13.0),
            ..default()
        },
        TextColor(STAT_LABEL),
        Node {
            align_self: AlignSelf::Center,
            margin: UiRect::horizontal(Val::Px(4.0)),
            ..default()
        },
    )
}

/// Spawn one `LABEL n/total` row — the value and total pair each get
/// two digit cells so a leading tens zero collapses (`Off` slots take
/// no layout space).
fn pair_row(
    col: &mut bevy::ecs::hierarchy::ChildSpawnerCommands,
    kind: PairKind,
    row: StatRow,
    text: &'static str,
) {
    col.spawn((
        row,
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::FlexEnd,
            display: Display::None,
            ..default()
        },
    ))
    .with_children(|r| {
        r.spawn(label(text));
        for slot in 0..2 {
            r.spawn((
                PairDigit { kind, slot },
                ImageNode::default(),
                Node {
                    display: Display::None,
                    ..default()
                },
            ));
        }
        r.spawn(label("/"));
        for slot in 2..4 {
            r.spawn((
                PairDigit { kind, slot },
                ImageNode::default(),
                Node {
                    display: Display::None,
                    ..default()
                },
            ));
        }
    });
}

/// Spawn the standings cluster and bind the authored half-digit set.
/// Every stem resolves through `crate::city::load_image` — the same
/// VFS lookup (mods can substitute `png`/`ktx2`/`tex`) every other
/// texture takes. A missing or undecodable glyph aborts the spawn:
/// the report records `absent` and no half-bound cluster exists.
///
/// Which instruments spawn rides on the definition, not the session:
/// the laps row exists only under `Ordered` rules (HUD-2 scopes the
/// laps record to Circuit) and the `FIN` entry only where the
/// authored definition carries a separate finish trigger.
pub fn spawn_race_stats(
    commands: &mut Commands,
    vfs: &Vfs,
    images: &mut Assets<Image>,
    owner: SessionEntity,
    def: &mm2_game::RaceDefinition,
) -> RaceStatReport {
    let mut report = RaceStatReport::default();
    let mut half: [Handle<Image>; 10] = Default::default();
    let mut ok = true;
    for (d, cell) in half.iter_mut().enumerate() {
        match crate::city::load_image(vfs, half_stem(d)).map(|(img, _)| images.add(img)) {
            Some(h) => {
                report.glyphs += 1;
                *cell = h;
            }
            None => ok = false,
        }
    }
    if !ok {
        report.absent = Some("missing-glyphs");
        return report;
    }

    let mut root = commands.spawn((
        owner,
        RaceStats,
        StatDigits { half },
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(STAT_TOP_PX),
            right: Val::Px(STAT_RIGHT_PX),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::FlexEnd,
            padding: UiRect::axes(Val::Px(6.0), Val::Px(4.0)),
            row_gap: Val::Px(2.0),
            ..default()
        },
        BackgroundColor(STAT_PANEL),
        Visibility::Hidden,
    ));
    root.with_children(|col| {
        pair_row(col, PairKind::Place, StatRow::Place, "PLACE");
        if def.rule == CheckpointRule::Ordered {
            pair_row(col, PairKind::Lap, StatRow::Lap, "LAP");
        }
        col.spawn(label("CHECKPOINTS"));
        for gate in 0..def.checkpoints.len() {
            col.spawn(Node {
                flex_direction: FlexDirection::Row,
                ..default()
            })
            .with_children(|r| {
                for slot in 0..2 {
                    r.spawn((
                        GateDigit { gate, slot },
                        ImageNode::default(),
                        Node {
                            display: if slot == 0 && gate + 1 < 10 {
                                Display::None
                            } else {
                                Display::Flex
                            },
                            ..default()
                        },
                    ));
                }
            });
        }
        if def.rule == CheckpointRule::AnyOrder && def.finish.is_some() {
            col.spawn((FinishLabel, label("FIN")));
        }
    });
    report
}

/// Drive the cluster off authoritative race state every frame: it
/// shows while a non-stale race is `Countdown` or `Running` and hides
/// on `Complete`, on a stale race, or under the `H` HUD gate
/// (F22-A.3) — the same contract [`crate::racetime::update_race_timer`]
/// keeps. The report's fields compose whenever the race is live,
/// gate or no gate, so `sta=` records demand like `tmr=`'s `display`.
/// Runs ungated by `capturing` — a `--frames`/`--screenshot` run must
/// render the instruments with live input frozen.
// The system legitimately reads several resources and two participant
// views — the same spread `update_nav_arrow` + `update_hud` carry.
#[allow(clippy::too_many_arguments)]
pub fn update_race_stats(
    race: Option<Res<RaceState>>,
    session: Res<Session>,
    hud: Res<crate::hud::HudVisible>,
    mut report: Option<ResMut<RaceStatReport>>,
    local: Query<
        (&Player, &RaceProgress, &Position, Option<&TargetSelection>),
        With<PlayerVehicle>,
    >,
    participants: Query<(&Player, &RaceProgress, &Position), Without<PlayerVehicle>>,
    mut roots: Query<(&StatDigits, &mut Visibility), With<RaceStats>>,
    mut cells: ParamSet<(StatRows, PairCells, GateCells, FinishCells)>,
) {
    let live = race
        .filter(|r| !r.is_stale(session.generation()))
        .filter(|r| r.phase != RacePhase::Complete);
    // The local participant's view — the player vehicle's components,
    // or any `Local`-controlled participant when the vehicle entity
    // itself is not a participant (the `update_hud` precedent).
    let local_view = local.iter().next().map(|(p, prog, pos, sel)| {
        (
            Some(p.id),
            Some(prog),
            Some(pos.0),
            sel.and_then(|s| s.picked),
        )
    });
    let (local_id, local_progress, local_pos, picked) = local_view.unwrap_or_else(|| {
        participants
            .iter()
            .find(|(p, _, _)| p.control == PlayerControl::Local)
            .map(|(p, prog, pos)| (Some(p.id), Some(prog), Some(pos.0), None))
            .unwrap_or((None, None, None, None))
    });

    // What each instrument says this frame — computed even while the
    // cluster is hidden, so the report records demand.
    let mut place = None;
    let mut lap = None;
    let mut checkpoints = None;
    let mut armed_gate = None;
    let mut finish_armed = false;
    if let Some(r) = live.as_deref() {
        let def = &r.definition;
        // The place indicator reads the live running order — and only
        // a competitive field has a placing to show (DSN-13).
        let order = live_order(
            def,
            local
                .iter()
                .map(|(p, prog, pos, _)| (p.id, prog, pos.0))
                .chain(
                    participants
                        .iter()
                        .map(|(p, prog, pos)| (p.id, prog, pos.0)),
                ),
        );
        if order.len() > 1
            && let Some(i) = local_id.and_then(|id| order.iter().position(|p| *p == id))
        {
            place = Some((i as u32 + 1, order.len() as u32));
        }
        if let Some(prog) = local_progress {
            if def.rule == CheckpointRule::Ordered {
                // Completed laps + 1 — a resolved participant parks on
                // `laps/laps` rather than rolling past the total.
                lap = Some((prog.lap.saturating_add(1).min(def.laps), def.laps));
                if matches!(
                    prog.state,
                    ParticipantState::AwaitingStart | ParticipantState::Racing
                ) && prog.next < def.checkpoints.len()
                {
                    armed_gate = Some(prog.next);
                }
            }
            checkpoints = Some((prog.cleared_count(), def.checkpoints.len()));
            if def.rule == CheckpointRule::AnyOrder
                && let Some(pos) = local_pos
                && let Some(target) = navigation_target(def, prog, picked, pos)
            {
                match target {
                    NavTarget::Gate(i) => armed_gate = Some(i),
                    NavTarget::Finish => finish_armed = true,
                }
            }
        }
    }

    for (_digits, mut vis) in &mut roots {
        *vis = if live.is_some() && hud.0 {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
    for (row, mut node) in &mut cells.p0() {
        let show = match row {
            StatRow::Place => place.is_some(),
            StatRow::Lap => lap.is_some(),
        };
        let display = if show { Display::Flex } else { Display::None };
        if node.display != display {
            node.display = display;
        }
    }
    // The cells recompose whether or not the race is live: `Off`
    // slots and pending tints are the idle state, so a race that
    // goes stale mid-session never leaves a frozen frame behind.
    let digits = roots
        .iter()
        .next()
        .map(|(d, _)| d.half.clone())
        .unwrap_or_default();
    for (cell, mut image, mut node) in &mut cells.p1() {
        let value = match cell.kind {
            PairKind::Place => place,
            PairKind::Lap => lap,
        };
        match value
            .map(|(v, t)| pair_slots(v, t)[cell.slot])
            .unwrap_or(StatGlyph::Off)
        {
            StatGlyph::Off => {
                if node.display != Display::None {
                    node.display = Display::None;
                }
            }
            StatGlyph::Digit(d) => {
                if node.display != Display::Flex {
                    node.display = Display::Flex;
                }
                if image.image != digits[d as usize] {
                    image.image = digits[d as usize].clone();
                }
            }
        }
    }
    for (cell, mut image) in &mut cells.p2() {
        let state = if let Some(prog) = local_progress {
            if prog.is_cleared(cell.gate) {
                GATE_CLEARED
            } else if armed_gate == Some(cell.gate) {
                GATE_ARMED
            } else {
                GATE_PENDING
            }
        } else {
            GATE_PENDING
        };
        if image.color != state {
            image.color = state;
        }
        if let StatGlyph::Digit(d) = gate_slots(cell.gate)[cell.slot]
            && image.image != digits[d as usize]
        {
            image.image = digits[d as usize].clone();
        }
    }
    for mut color in &mut cells.p3() {
        *color = TextColor(if finish_armed {
            GATE_ARMED
        } else {
            GATE_CLEARED
        });
    }
    if let Some(report) = report.as_deref_mut() {
        report.place = place;
        report.lap = lap;
        report.checkpoints = checkpoints;
    }
}
