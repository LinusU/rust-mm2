//! The authored race timer (F22-A.4; HUD-2's "stopwatch"/"countdown
//! timer" pair — the `mmTimer` instruments the recovered `mmHUD`
//! layout owns). The instrument renders the authored `digitac_*` /
//! `digi_colon` glyph set the installation ships rather than a text
//! fallback: every glyph is a real TGA resolved through the VFS like
//! every other authored resource.
//!
//! What is original here: the glyph artwork itself and the
//! instrument's *data* contract — one timer that counts down from the
//! authored limit while a timed definition (Blitz) runs and counts up
//! otherwise, driven by the authoritative [`RaceState::clock`], so
//! pause/finish freeze it deterministically (HUD-2, F22-AC01). The
//! exact original layout — which glyph sizes compose where, whether
//! minutes are zero-padded, where the timers sit — is unrecovered
//! (UNK-32), so the presentation is a designed reading (DSN-53):
//!
//! - one row, top-centre just under the nav arrow, laid out
//!   `m:ss:hh` — full-size digits for minutes/seconds, the authored
//!   half-size digits for centiseconds, `digi_colon` separators;
//! - leading-zero suppression on minutes, capped at `999:59:99`;
//! - a translucent backing plate tinted to the colon tile's own
//!   authored background (`digi_colon` is an opaque-panel image, so
//!   the seam reads as part of the plate).
//!
//! The countdown seconds cue stays with the dev-rig
//! [`crate::race::CountdownBanner`]; this instrument covers the
//! running/descending clock the HUD line previously compressed into
//! text. Layer membership is `mmHUD`'s: the `H` gate (F22-A.3) hides
//! the row while the composed display keeps updating on the report,
//! and `camera::retarget_hud` pins the root to the active world camera
//! with the rest of the HUD layer.
//!
//! - [`spawn_race_timer`] runs in `load_session_world`'s event arm:
//!   it loads all 22 glyph stems through the VFS, and a partial or
//!   missing set records `absent` on the [`RaceTimerReport`] — never
//!   substituted glyph art.
//! - [`update_race_timer`] drives visibility and slot contents every
//!   frame off authoritative race state: hidden with no live race
//!   (cruise, stale generation, `Complete`), live through `Countdown`
//!   and `Running` — including under the results overlay while
//!   unresolved participants still race.

use bevy::prelude::*;
use mm2_assets::Vfs;
use mm2_game::{RACE_TICK_HZ, RacePhase, RaceState, Session, SessionEntity};

/// Slots in the timer row: `[m3][m2][m1][:][s1][s2][:][c1][c2]` —
/// three minute digits (leading zeros off), a seconds pair and the
/// centiseconds pair with their separators.
pub const TIMER_SLOTS: usize = 9;

/// Top edge of the timer plate — just under the nav arrow (which ends
/// ~90 px). Designed placement (UNK-32); the rear-view strip still
/// draws over this band while it is up, the same overlap the nav
/// arrow already has.
const TIMER_TOP_PX: f32 = 96.0;

/// Backing-plate tint — the colon tile's own authored background
/// (~`#313031`), made translucent so the plate stays a HUD wash
/// rather than a solid block.
const TIMER_PANEL: Color = Color::srgba(0.19, 0.188, 0.192, 0.82);

/// The full glyph set the instrument binds, in VFS `texture/` stems:
/// `digitac_0..9` (41×56 green alpha-keyed digits), the two
/// `digi_colon` separators and the half-size `digitac_*_half` set.
const fn digit_stem(d: usize) -> [&'static str; 2] {
    // Full + half stems share the `digitac_` prefix; the half set
    // carries the `_half` suffix the installation ships.
    const FULL: [&str; 10] = [
        "digitac_0",
        "digitac_1",
        "digitac_2",
        "digitac_3",
        "digitac_4",
        "digitac_5",
        "digitac_6",
        "digitac_7",
        "digitac_8",
        "digitac_9",
    ];
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
    [FULL[d], HALF[d]]
}

/// Total glyph images the instrument needs — 10 full digits, the full
/// colon, 10 half digits, the half colon.
pub const TIMER_GLYPH_COUNT: usize = 22;

/// Marker on the session-owned timer root — an absolute row node the
/// drive pass fills with authored glyph images. SessionEntity-stamped
/// through `owner` so teardown removes it; `camera::retarget_hud`
/// writes its `UiTargetCamera` like every other HUD-layer root.
#[derive(Component)]
pub struct RaceTimer;

/// The bound glyph set, carried on the timer root.
#[derive(Component)]
pub struct TimerDigits {
    /// `digitac_0..9` — minutes and seconds.
    pub full: [Handle<Image>; 10],
    /// `digi_colon` — the minutes/seconds separator.
    pub colon: Handle<Image>,
    /// `digitac_*_half` — centiseconds.
    pub half: [Handle<Image>; 10],
    /// `digi_colon_half` — the seconds/centiseconds separator.
    pub colon_half: Handle<Image>,
}

/// Marker + slot index on one glyph cell of the timer row.
#[derive(Component)]
pub struct RaceTimerGlyph(pub usize);

/// What one slot shows on a given pass — public so tests can check
/// the composed layout directly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimerGlyph {
    /// Slot suppressed — a leading zero. `Display::None` so it takes
    /// no layout space and the row stays compact and centred.
    Off,
    /// A full-size `digitac_` digit (minutes/seconds).
    Digit(u8),
    /// The full-size `digi_colon`.
    Colon,
    /// A half-size `digitac_*_half` digit (centiseconds).
    HalfDigit(u8),
    /// The half-size `digi_colon_half`.
    HalfColon,
}

/// Session-scoped report for the timer instrument — the `tmr=` record
/// field's source. Inserted for every event session (bound or not —
/// `absent` says why not); cruise and dev-world sessions get no
/// report, so their records stay bit-identical.
#[derive(Resource, Default)]
pub struct RaceTimerReport {
    /// Glyph images bound — the full authored set is
    /// [`TIMER_GLYPH_COUNT`].
    pub glyphs: usize,
    /// Why the instrument did not bind: `missing-glyphs` (absent or
    /// undecodable artwork — never substituted).
    pub absent: Option<&'static str>,
    /// The `m:ss:hh` text the row composed on the last drive pass —
    /// kept live even while the `H` gate hides the row, so the field
    /// shows demand like `ind=`'s `bound`. `None` when no live race
    /// wants a timer.
    pub display: Option<String>,
}

impl RaceTimerReport {
    /// The `tmr=` record field body: `<glyphs>g/<m:ss:hh>` while a
    /// race is live, `<glyphs>g/off` when bound but idle, or
    /// `absent:<why>` when the glyph set never loaded.
    pub fn smoke_detail(&self) -> String {
        match self.absent {
            Some(why) => format!("absent:{why}"),
            None => format!(
                "{}g/{}",
                self.glyphs,
                self.display.as_deref().unwrap_or("off")
            ),
        }
    }
}

/// The authoritative ticks the instrument shows right now: the
/// countdown remainder while a timed definition runs, else the race
/// clock — `None` when no live race wants a timer (no race, a stale
/// generation, or a `Complete` phase).
fn shown_ticks(race: Option<&RaceState>, session: &Session) -> Option<u64> {
    let race = race?;
    if race.is_stale(session.generation()) {
        return None;
    }
    match race.phase {
        RacePhase::Countdown { .. } | RacePhase::Running => {
            Some(race.time_remaining().map(u64::from).unwrap_or(race.clock))
        }
        RacePhase::Complete => None,
    }
}

/// Race ticks → centiseconds (the `hh` pair's unit) at the race rate.
fn centiseconds(ticks: u64) -> u64 {
    ticks * 100 / u64::from(RACE_TICK_HZ)
}

/// Split a centiseconds value into the nine slot glyphs: unpadded
/// minutes (leading slots `Off`), `ss` seconds, `hh` centiseconds —
/// the designed `m:ss:hh` layout (DSN-53), capped at `999:59:99`.
pub fn timer_slots(cs: u64) -> [TimerGlyph; TIMER_SLOTS] {
    let cs = cs.min(999 * 6000 + 5999);
    let m = (cs / 6000) as u32;
    let s = ((cs % 6000) / 100) as u8;
    let h = (cs % 100) as u8;
    [
        if m >= 100 {
            TimerGlyph::Digit((m / 100) as u8)
        } else {
            TimerGlyph::Off
        },
        if m >= 10 {
            TimerGlyph::Digit((m / 10 % 10) as u8)
        } else {
            TimerGlyph::Off
        },
        TimerGlyph::Digit((m % 10) as u8),
        TimerGlyph::Colon,
        TimerGlyph::Digit(s / 10),
        TimerGlyph::Digit(s % 10),
        TimerGlyph::HalfColon,
        TimerGlyph::HalfDigit(h / 10),
        TimerGlyph::HalfDigit(h % 10),
    ]
}

/// The `m:ss:hh` text for a slot row — the report's `display` form,
/// matching the rendered glyphs exactly.
fn display_text(cs: u64) -> String {
    let cs = cs.min(999 * 6000 + 5999);
    format!("{}:{:02}:{:02}", cs / 6000, (cs % 6000) / 100, cs % 100)
}

/// Spawn the timer row and bind the authored glyph set. Every stem
/// resolves through `crate::city::load_image` — the same VFS lookup
/// (mods can substitute `png`/`ktx2`/`tex`) every other texture
/// takes. A missing or undecodable glyph aborts the spawn: the report
/// records `absent` and no half-bound row exists.
pub fn spawn_race_timer(
    commands: &mut Commands,
    vfs: &Vfs,
    images: &mut Assets<Image>,
    owner: SessionEntity,
) -> RaceTimerReport {
    let mut report = RaceTimerReport::default();
    let mut load = |stem: &str, report: &mut RaceTimerReport| {
        let image = crate::city::load_image(vfs, stem).map(|(img, _)| images.add(img));
        if image.is_some() {
            report.glyphs += 1;
        }
        image
    };
    let mut full: [Handle<Image>; 10] = Default::default();
    let mut half: [Handle<Image>; 10] = Default::default();
    let mut ok = true;
    for d in 0..10 {
        let [full_stem, half_stem] = digit_stem(d);
        match load(full_stem, &mut report) {
            Some(h) => full[d] = h,
            None => ok = false,
        }
        match load(half_stem, &mut report) {
            Some(h) => half[d] = h,
            None => ok = false,
        }
    }
    let colon = load("digi_colon", &mut report);
    let colon_half = load("digi_colon_half", &mut report);
    let (Some(colon), Some(colon_half)) = (colon, colon_half) else {
        report.absent = Some("missing-glyphs");
        return report;
    };
    if !ok {
        report.absent = Some("missing-glyphs");
        return report;
    }
    commands
        .spawn((
            owner,
            RaceTimer,
            TimerDigits {
                full,
                colon,
                half,
                colon_half,
            },
            Node {
                position_type: PositionType::Absolute,
                left: Val::Percent(50.0),
                top: Val::Px(TIMER_TOP_PX),
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::FlexEnd,
                padding: UiRect::axes(Val::Px(6.0), Val::Px(3.0)),
                ..default()
            },
            // Percent resolves against the row's own size, so the
            // variable-width row stays centred as digits come and go.
            UiTransform::from_xy(Val::Percent(-50.0), Val::ZERO),
            BackgroundColor(TIMER_PANEL),
            Visibility::Hidden,
        ))
        .with_children(|row| {
            for slot in 0..TIMER_SLOTS {
                row.spawn((
                    RaceTimerGlyph(slot),
                    ImageNode::default(),
                    Node {
                        display: Display::None,
                        ..default()
                    },
                ));
            }
        });
    report
}

/// Drive the timer row off authoritative race state every frame: the
/// row shows while a non-stale race is `Countdown` or `Running` —
/// counting down from `time_remaining` on a timed definition, up on
/// `clock` otherwise — and hides on `Complete`, on a stale race, or
/// under the `H` HUD gate (F22-A.3). Glyph slots recompose only on a
/// live race; the report's `display` records the same `m:ss:hh`
/// whether or not the row is visible. Runs ungated by `capturing`
/// like `drive_mirror` — a `--frames`/`--screenshot` run must render
/// the instrument with live input frozen.
pub fn update_race_timer(
    race: Option<Res<RaceState>>,
    session: Res<Session>,
    hud: Res<crate::hud::HudVisible>,
    mut report: Option<ResMut<RaceTimerReport>>,
    mut timers: Query<(&TimerDigits, &mut Visibility), With<RaceTimer>>,
    mut glyphs: Query<(&RaceTimerGlyph, &mut ImageNode, &mut Node), Without<RaceTimer>>,
) {
    let shown = shown_ticks(race.as_deref(), &session).map(centiseconds);
    for (digits, mut vis) in &mut timers {
        *vis = if shown.is_some() && hud.0 {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        let Some(cs) = shown else { continue };
        let slots = timer_slots(cs);
        for (glyph, mut image, mut node) in &mut glyphs {
            let (slot_image, display) = match slots[glyph.0] {
                TimerGlyph::Off => (None, Display::None),
                TimerGlyph::Digit(d) => (Some(&digits.full[d as usize]), Display::Flex),
                TimerGlyph::Colon => (Some(&digits.colon), Display::Flex),
                TimerGlyph::HalfDigit(d) => (Some(&digits.half[d as usize]), Display::Flex),
                TimerGlyph::HalfColon => (Some(&digits.colon_half), Display::Flex),
            };
            if node.display != display {
                node.display = display;
            }
            if let Some(handle) = slot_image
                && image.image != *handle
            {
                image.image = handle.clone();
            }
        }
    }
    if let Some(report) = report.as_deref_mut() {
        report.display = shown.map(display_text);
    }
}
