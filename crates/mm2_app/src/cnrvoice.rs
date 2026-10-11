//! Cops & Robbers commentary (F27-B.4c): the announcer calls the gold's
//! pickups, drops and stashes from the city's `aud/spchdata/cnr<city>.csv`.
//!
//! The cue vocabulary and the wave pool each family draws from are
//! original data (ledger CNR-13); *when* each family fires is not
//! recovered, so the mapping is designed. Every process voices its own
//! calls: the authority from the [`CnrEvent`] stream its match emits, a
//! joined client from the changes between the [`CnrReplica`] frames it
//! receives ([`GoldView::call_since`]) — nothing new on the wire, and a
//! frame lost to the link costs a line, never a wrong score.
//!
//! No cue is invented. A city without the table binds nothing; a family
//! the table lacks, an undrawable row or a wave the bank cannot resolve
//! counts one `failed` and says nothing.

use bevy::prelude::*;
use mm2_assets::Vfs;
use mm2_content::cnr::{CnrContent, commentary_cue};
use mm2_formats::spchdata::CueTable;
use mm2_game::gold::{Call, GoldView};
use mm2_game::{Mm2Vfs, NavRng, Session, SessionPhase, cue_wave_stem, draw_cue_suffix};

use crate::audio::{AudioReport, PcmAudio, WaveBank};
use crate::cnr::{CnrEvent, CnrHost};
use crate::cnrnet::CnrReplica;
use crate::speech::{SpeechArbiter, SpeechLine, SpeechPriority};

/// Most calls waiting for the line in progress. A burst (a steal, a
/// drop and a recovery inside a second) keeps its first lines and
/// counts the rest `dropped` instead of narrating the past.
pub const MAX_PENDING_CALLS: usize = 3;

/// A queued line older than this has been overtaken by play and is
/// dropped, counted — a drop announced ten seconds late is wrong.
pub const STALE_CALL_SECS: f32 = 6.0;

/// Separates the seeded draw stream from the other audio streams.
const CNR_VOICE_DOMAIN: u64 = 0x636e_7276_6f69_6365;

/// The session's C&R announcer: the parsed cue table and the seeded
/// suffix stream; its lines wait for the floor in the
/// [`SpeechArbiter`].
#[derive(Resource)]
pub struct CnrCommentary {
    table: CueTable,
    rng: NavRng,
    /// The replica frame the last call was derived from — a client's
    /// baseline. `None` until the first frame, which never announces.
    seen: Option<GoldView>,
    /// Lines queued so far, played or not — the process-level evidence.
    said: u32,
}

impl CnrCommentary {
    /// Bind the city's table, or `None` (warned) when it is absent or
    /// unreadable — the content audit counts that, play goes on silent.
    pub fn load(vfs: &Vfs, city: &str, seed: u64) -> Option<Self> {
        let path = CnrContent::commentary_path(city);
        let bytes = vfs.read_logical(&path).ok()?;
        match CueTable::parse(&String::from_utf8_lossy(&bytes)) {
            Ok(table) => Some(Self::new(table, seed)),
            Err(e) => {
                warn!("audio: {path}: {e}");
                None
            }
        }
    }

    /// Bind an already parsed table.
    pub fn new(table: CueTable, seed: u64) -> Self {
        Self {
            table,
            rng: NavRng::new(seed.wrapping_add(CNR_VOICE_DOMAIN)),
            seen: None,
            said: 0,
        }
    }

    /// The wave stem a call draws: the family's authored row, a seeded
    /// suffix inside its window, the speaker-qualified prefix flattened
    /// to the bank's stem. `Err` names what is missing.
    pub fn draw(&mut self, call: Call) -> Result<String, String> {
        let cue = commentary_cue(call).ok_or_else(|| format!("{call:?} has no cue family"))?;
        let row = self
            .table
            .section(cue)
            .and_then(|s| s.rows.first())
            .ok_or_else(|| format!("the table authors no {cue} cue"))?;
        let suffix = draw_cue_suffix(row.end, row.add, &mut self.rng).ok_or_else(|| {
            format!(
                "{cue} has an undrawable range (end {} add {})",
                row.end, row.add
            )
        })?;
        // The prefix carries its speaker (`AS1\AS1COPS`), so none is
        // passed.
        Ok(cue_wave_stem("", &row.prefix, suffix))
    }

    /// Lines this session's announcer has queued to speak.
    pub fn said(&self) -> u32 {
        self.said
    }
}

/// Voice the session's calls. Authority: its own match events.
/// Client: the change between replica frames. Lines queue (bounded,
/// stale ones dropped) and play one at a time as non-spatial
/// `PlaybackMode::Despawn` one-shots, `SessionEntity`-stamped so
/// teardown sweeps them. Idle while paused; events are read every frame
/// regardless, so a pause cannot leave an old one to speak later.
#[allow(clippy::too_many_arguments)] // Bevy system — the borrows are the contract.
pub fn cnr_commentary_voices(
    session: Res<Session>,
    mut events: MessageReader<CnrEvent>,
    host: Option<Res<CnrHost>>,
    replica: Option<Res<CnrReplica>>,
    commentary: Option<ResMut<CnrCommentary>>,
    vfs: Option<Res<Mm2Vfs>>,
    bank: Option<ResMut<WaveBank>>,
    mut waves: ResMut<Assets<PcmAudio>>,
    mut report: ResMut<AudioReport>,
    mut arbiter: ResMut<SpeechArbiter>,
) {
    let events: Vec<CnrEvent> = events.read().copied().collect();
    let (Some(mut commentary), Some(vfs), Some(mut bank)) = (commentary, vfs, bank) else {
        return;
    };
    let commentary = &mut *commentary;
    if !matches!(
        session.phase(),
        SessionPhase::Playing | SessionPhase::Results
    ) {
        return;
    }
    let mut calls: Vec<Call> = Vec::new();
    if let Some(host) = &host {
        calls.extend(
            events
                .iter()
                .filter_map(|e| e.0.call(|p| host.game.side_of(p))),
        );
    } else if let Some(replica) = &replica {
        let view = &replica.0;
        if commentary
            .seen
            .as_ref()
            .is_none_or(|seen| view.freshness() > seen.freshness())
        {
            if let Some(seen) = &commentary.seen {
                calls.extend(view.call_since(seen));
            }
            commentary.seen = Some(view.clone());
        }
    }
    for call in calls {
        // The bound is on the calls waiting for the floor: a burst
        // keeps its first lines and counts the rest.
        if arbiter.queued_at(SpeechPriority::Call) >= MAX_PENDING_CALLS {
            report.dropped += 1;
            continue;
        }
        let loaded =
            commentary
                .draw(call)
                .and_then(|stem| match bank.load(&vfs.0, &mut waves, &stem) {
                    Ok(handle) => Ok((stem, handle)),
                    Err(e) => Err(format!("{stem}: {e}")),
                });
        match loaded {
            Ok((stem, clip)) => {
                info!("cops and robbers commentary: {call:?} -> {stem}");
                let secs = waves.get(&clip).map(|w| w.duration()).unwrap_or(0.0);
                arbiter.submit(
                    session.generation(),
                    SpeechLine {
                        stem,
                        clip,
                        secs,
                        priority: SpeechPriority::Call,
                        max_age: Some(STALE_CALL_SECS),
                        suppress_repeats: false,
                    },
                    &mut report,
                );
                commentary.said += 1;
            }
            Err(e) => {
                report.failed += 1;
                warn!("audio: Cops & Robbers commentary: {e}");
            }
        }
    }
}
