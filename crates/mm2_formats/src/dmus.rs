//! Structural reader for the DirectMusic RIFF containers in `aud/dmusic`
//! (F08-A.4): segments (`.sgt`, form `DMSG`), styles (`.sty`, `DMST`),
//! bands (`.bnd`, `DMBD`) and DLS sound banks (`.dls`, `DLS `).
//!
//! This is an *inventory* decoder, not a synthesizer: it walks the RIFF
//! chunk tree with hard depth/node bounds and returns what each container
//! declares — header words, the track kinds a segment carries, the
//! `DMRF` file references that bind a segment to its style, band and
//! sound bank, a band's instrument patches, a style's part/pattern
//! counts and a DLS bank's instruments, regions and wave pool (whose
//! samples are ordinary `fmt `/`data` waves, so PCM ones decode here).
//! What a track's payload *means* (command/tempo/chord/style-reference
//! events) and how segments transition are not decoded; the chunk
//! layouts follow Microsoft's published DirectMusic file-format
//! reference (`dmusicf.h`) and the DLS Level 1 specification, checked
//! against the retail containers rather than assumed.

use crate::wav::{WaveFormat, parse_fmt};
use crate::{FormatError, Reader};
use std::ops::Range;

/// RIFF nesting bound. Retail's deepest container (a segment holding a
/// band track holding an embedded band holding its instrument
/// references) nests nine levels; anything past this is not sane.
const MAX_DEPTH: usize = 16;
/// Upper bound on chunks visited in one container.
const MAX_NODES: usize = 200_000;

/// Which DirectMusic artifact a container is, from its RIFF form word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmKind {
    /// `DMSG` — a segment: the playable unit the cue tables name.
    Segment,
    /// `DMST` — a style: parts, patterns and an embedded band.
    Style,
    /// `DMBD` — a band: instrument-to-patch assignments.
    Band,
    /// `DLS ` — a downloadable sound bank with a wave pool.
    Dls,
}

impl DmKind {
    /// The kind a RIFF form word names, `None` for any other form.
    pub fn from_form(form: &[u8; 4]) -> Option<Self> {
        match form {
            b"DMSG" => Some(Self::Segment),
            b"DMST" => Some(Self::Style),
            b"DMBD" => Some(Self::Band),
            b"DLS " => Some(Self::Dls),
            _ => None,
        }
    }
}

/// Where a [`DmReference`] sits in its container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefSite {
    /// Inside a band instrument (`lbin`): the DLS collection it plays from.
    BandInstrument,
    /// Inside a style track's reference list (`strf`): the style a
    /// segment plays.
    StyleTrack,
    /// Directly under a track (`DMTK`).
    Track,
    /// Anywhere else.
    Other,
}

/// A `DMRF` file reference: how one container names another. The target
/// is a file *name* (`DLS Collection1.dls`), not a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DmReference {
    /// Where in the container the reference was authored.
    pub site: RefSite,
    /// The `name` string (the object's embedded name), when authored.
    pub name: Option<String>,
    /// The `file` string, when authored.
    pub file: Option<String>,
    /// The referenced object's `guid`, when authored.
    pub guid: Option<[u8; 16]>,
}

/// `segh`: the segment header (`DMUS_IO_SEGMENT_HEADER`, DX7 layout).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentHeader {
    /// Repeat count (the loop count; `0` plays once).
    pub repeats: u32,
    /// Length in music time units (`mtLength`).
    pub length: u32,
    /// Music time where playback starts.
    pub play_start: u32,
    /// Music time of the loop start.
    pub loop_start: u32,
    /// Music time of the loop end (`0` with `repeats > 0` loops the whole
    /// segment).
    pub loop_end: u32,
    /// Boundary a transition into this segment aligns to
    /// (`DMUS_SEGF_*` resolution bits).
    pub resolution: u32,
}

/// `trkh`: one track of a segment (`DMUS_IO_TRACK_HEADER`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Track {
    /// The track's data chunk id (`cmnd` command, `tetr` tempo, `sttr`
    /// style reference, `DMBT` band …) — taken from the header's `ckid`
    /// word, or its `fccType` form word when `ckid` is zero.
    pub kind: [u8; 4],
    /// The track-class GUID.
    pub class_guid: [u8; 16],
    /// Track-group bits (`dwGroup`).
    pub group: u32,
    /// Ordering position within the group.
    pub position: u32,
}

impl Track {
    /// A readable name for the track kind, the raw fourcc when unknown.
    pub fn kind_name(&self) -> String {
        match &self.kind {
            b"cmnd" => "command".into(),
            b"tetr" => "tempo".into(),
            b"sttr" => "style".into(),
            b"DMBT" => "band".into(),
            b"cord" | b"crdt" | b"DMCH" => "chord".into(),
            b"DMTK" => "track".into(),
            other => fourcc_text(other),
        }
    }
}

/// `styh`: a style's time signature and tempo.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StyleHeader {
    /// Beats per measure.
    pub beats_per_measure: u8,
    /// Beat note value (4 = quarter note).
    pub beat: u8,
    /// Grid subdivisions per beat.
    pub grids_per_beat: u16,
    /// Authored tempo, beats per minute.
    pub tempo: f64,
}

/// A band instrument entry (`bins`, `DMUS_IO_INSTRUMENT`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BandInstrument {
    /// MIDI bank select MSB.
    pub bank_msb: u8,
    /// MIDI bank select LSB.
    pub bank_lsb: u8,
    /// MIDI program number.
    pub program: u8,
    /// The performance channel it plays on.
    pub pchannel: u32,
    /// `DMUS_IO_INST_*` flags (which optional fields are valid).
    pub flags: u32,
}

/// What a container declares beyond its kind. Counts cover the whole
/// container including embedded bands.
#[derive(Debug, Clone, Default)]
pub struct DmContent {
    /// Segment header (`DMSG` only).
    pub segment: Option<SegmentHeader>,
    /// Segment tracks in file order.
    pub tracks: Vec<Track>,
    /// Style header (`DMST` only).
    pub style: Option<StyleHeader>,
    /// `part` lists in a style.
    pub parts: usize,
    /// `pttn` pattern lists in a style.
    pub patterns: usize,
    /// `DMBD` bands, the container itself and any embedded ones.
    pub bands: usize,
    /// Every band instrument, in file order.
    pub instruments: Vec<BandInstrument>,
}

/// One instrument of a DLS bank.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DlsInstrument {
    /// `INAM` name, when authored.
    pub name: Option<String>,
    /// `ulBank` (bit 31 flags a drum kit).
    pub bank: u32,
    /// `ulInstrument` — the MIDI program.
    pub program: u32,
    /// Region count the `insh` header declares.
    pub declared_regions: u32,
    /// Wave-pool table index each region links to (`wlnk`), `None` for a
    /// region with no link chunk.
    pub region_links: Vec<Option<u32>>,
}

/// One wave of a DLS bank's pool.
#[derive(Debug, Clone)]
pub struct DlsWave {
    /// The wave's `fmt ` record.
    pub format: WaveFormat,
    /// Payload range in the container bytes (empty when no `data`).
    pub data: Range<usize>,
    /// `INAM` name, when authored.
    pub name: Option<String>,
}

impl DlsWave {
    /// The payload as 16-bit samples, `None` unless uncompressed 16-bit
    /// PCM (anything else is reported, never guessed at).
    pub fn samples_i16(&self, file: &[u8]) -> Option<Vec<i16>> {
        if self.format.tag != crate::wav::FORMAT_PCM || self.format.bits_per_sample != 16 {
            return None;
        }
        let bytes = file.get(self.data.clone())?;
        Some(
            bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| i16::from_le_bytes(*b))
                .collect(),
        )
    }

    /// Whole frames in the payload (`None` for a degenerate block size).
    pub fn frames(&self) -> Option<usize> {
        (self.format.block_align != 0).then(|| self.data.len() / self.format.block_align as usize)
    }
}

/// DLS bank contents.
#[derive(Debug, Clone, Default)]
pub struct DlsContent {
    /// Instrument count the `colh` header declares.
    pub declared_instruments: Option<u32>,
    /// Instruments in file order.
    pub instruments: Vec<DlsInstrument>,
    /// `ptbl` cue offsets into the wave pool, in table order.
    pub pool_cues: Vec<u32>,
    /// Waves in pool order.
    pub waves: Vec<DlsWave>,
}

/// Findings on a parsed container — authored anomalies and unsupported
/// encodings. Structural failures are [`FormatError`]s instead.
#[derive(Debug, Clone, PartialEq)]
pub enum DmIssue {
    /// Bytes follow the container the RIFF size word declares.
    TrailingBytes {
        /// Unaccounted byte count.
        len: usize,
    },
    /// A fixed-layout chunk is shorter than its record.
    ShortChunk {
        /// The chunk fourcc.
        id: String,
        /// Payload length present.
        len: usize,
        /// Bytes the layout needs.
        need: usize,
    },
    /// A UTF-16 string chunk with an odd byte count.
    OddString {
        /// The chunk fourcc.
        id: String,
    },
    /// A segment carries no `segh` header.
    MissingSegmentHeader,
    /// A segment declares no tracks, so nothing would play.
    NoTracks,
    /// A style carries no `styh` header.
    MissingStyleHeader,
    /// A `DMRF` reference names no file.
    ReferenceWithoutFile,
    /// `colh` declares a different instrument count than `lins` holds.
    InstrumentCountMismatch {
        /// Authored count.
        declared: u32,
        /// Instruments present.
        found: usize,
    },
    /// An instrument's `insh` declares a different region count.
    RegionCountMismatch {
        /// Instrument index.
        instrument: usize,
        /// Authored count.
        declared: u32,
        /// Regions present.
        found: usize,
    },
    /// The `ptbl` cue count differs from the number of waves in the pool.
    PoolCueCountMismatch {
        /// Cues in the table.
        cues: usize,
        /// Waves in the pool.
        waves: usize,
    },
    /// A pool cue does not land on a `wave` list inside the pool.
    PoolCueOutOfRange {
        /// Table index.
        cue: usize,
        /// Authored offset.
        offset: u32,
    },
    /// A region links to a wave-pool table index past the table.
    DanglingWaveLink {
        /// Instrument index.
        instrument: usize,
        /// Region index.
        region: usize,
        /// Authored table index.
        index: u32,
    },
    /// A wave is missing its `fmt ` or `data` chunk.
    IncompleteWave {
        /// Pool index.
        wave: usize,
    },
    /// A wave is not uncompressed 16-bit PCM — present but not decodable
    /// here.
    UnsupportedWave {
        /// Pool index.
        wave: usize,
        /// Authored format tag.
        tag: u16,
        /// Authored bits per sample.
        bits: u16,
    },
}

/// A parsed DirectMusic container.
#[derive(Debug, Clone)]
pub struct DmContainer {
    /// Which artifact the form word names.
    pub kind: DmKind,
    /// The container's own `UNAM`/`INAM` name, when authored.
    pub name: Option<String>,
    /// The container `guid`, when authored.
    pub guid: Option<[u8; 16]>,
    /// `vers` as `(major word, minor word)`, when authored.
    pub version: Option<(u32, u32)>,
    /// Every `DMRF` reference, in file order.
    pub references: Vec<DmReference>,
    /// DirectMusic payload (segment/style/band; empty for a DLS bank).
    pub content: DmContent,
    /// DLS payload (`Some` only for a DLS bank).
    pub dls: Option<DlsContent>,
    /// Chunks visited.
    pub chunks: usize,
    /// Semantic findings.
    pub issues: Vec<DmIssue>,
}

/// Render a fourcc for display, replacing non-printable bytes.
pub fn fourcc_text(id: &[u8; 4]) -> String {
    id.iter()
        .map(|&b| {
            if b.is_ascii_graphic() || b == b' ' {
                b as char
            } else {
                '?'
            }
        })
        .collect()
}

/// One decoded chunk header: its id, payload range and the offset of the
/// next sibling (past even-byte padding).
struct Chunk {
    id: [u8; 4],
    /// Payload range (for `RIFF`/`LIST` it starts at the form word).
    body: Range<usize>,
    next: usize,
}

/// Read the chunk header at `off`, bounded by `end`.
fn chunk_at(data: &[u8], off: usize, end: usize) -> Result<Chunk, FormatError> {
    let mut r = Reader::at(data, off)?;
    let id: [u8; 4] = r.bytes(4)?.try_into().unwrap();
    let len = r.u32()? as usize;
    let start = off + 8;
    let stop = start
        .checked_add(len)
        .filter(|&s| s <= end)
        .ok_or(FormatError::InvalidValue {
            offset: off,
            field: "chunk_size",
            value: len as u64,
            reason: "chunk payload extends past its container",
        })?;
    Ok(Chunk {
        id,
        body: start..stop,
        next: stop + (len & 1),
    })
}

/// Iterate the sibling chunks in `range`. A trailing run too short for a
/// header is ignored (callers see it as padding).
fn children(
    data: &[u8],
    range: Range<usize>,
    nodes: &mut usize,
) -> Result<Vec<Chunk>, FormatError> {
    let mut out = Vec::new();
    let mut off = range.start;
    while off + 8 <= range.end {
        *nodes += 1;
        if *nodes > MAX_NODES {
            return Err(FormatError::InvalidValue {
                offset: off,
                field: "chunk_count",
                value: *nodes as u64,
                reason: "chunk tree exceeds sanity bound",
            });
        }
        let c = chunk_at(data, off, range.end)?;
        off = c.next;
        out.push(c);
    }
    Ok(out)
}

/// The form word of a `RIFF`/`LIST` chunk, `None` when its payload is
/// shorter than the word.
fn form_of(data: &[u8], c: &Chunk) -> Option<[u8; 4]> {
    (c.body.len() >= 4).then(|| data[c.body.start..c.body.start + 4].try_into().unwrap())
}

fn is_container(id: &[u8; 4]) -> bool {
    id == b"RIFF" || id == b"LIST"
}

/// UTF-16LE text up to the first NUL.
fn utf16_text(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u16::from_le_bytes(*b))
        .take_while(|&u| u != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

/// A NUL-terminated ASCII string chunk (DLS `INFO` entries).
fn ascii_text(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

impl DmContainer {
    /// Parse a DirectMusic RIFF container. Errors are structural (bad
    /// magic, an unknown form word, a chunk running past its parent,
    /// nesting or node bounds); authored anomalies land in
    /// [`DmContainer::issues`].
    pub fn parse(data: &[u8]) -> Result<Self, FormatError> {
        let mut r = Reader::new(data);
        let magic = r.bytes(4)?;
        if magic != b"RIFF" {
            return Err(FormatError::BadMagic {
                offset: 0,
                expected: "RIFF",
                found: magic.to_vec(),
            });
        }
        let declared = r.u32()? as usize;
        let form: [u8; 4] = r.bytes(4)?.try_into().unwrap();
        let kind = DmKind::from_form(&form).ok_or(FormatError::BadMagic {
            offset: 8,
            expected: "DMSG, DMST, DMBD or DLS ",
            found: form.to_vec(),
        })?;
        let end = declared.checked_add(8).filter(|&e| e <= data.len()).ok_or(
            FormatError::InvalidValue {
                offset: 4,
                field: "riff_size",
                value: declared as u64,
                reason: "RIFF size extends past end of file",
            },
        )?;
        if end < 12 {
            return Err(FormatError::InvalidValue {
                offset: 4,
                field: "riff_size",
                value: declared as u64,
                reason: "RIFF size shorter than its form word",
            });
        }
        let mut out = DmContainer {
            kind,
            name: None,
            guid: None,
            version: None,
            references: Vec::new(),
            content: DmContent::default(),
            dls: None,
            chunks: 0,
            issues: Vec::new(),
        };
        if data.len() > end {
            out.issues.push(DmIssue::TrailingBytes {
                len: data.len() - end,
            });
        }
        let mut nodes = 0usize;
        if kind == DmKind::Dls {
            out.parse_dls(data, 12..end, &mut nodes)?;
        } else {
            out.content.bands += usize::from(kind == DmKind::Band);
            out.walk(data, 12..end, 1, form, &mut nodes)?;
            out.finish_dm();
        }
        out.chunks = nodes;
        Ok(out)
    }

    fn short(&mut self, id: &[u8; 4], len: usize, need: usize) -> bool {
        if len >= need {
            return false;
        }
        self.issues.push(DmIssue::ShortChunk {
            id: fourcc_text(id),
            len,
            need,
        });
        true
    }

    /// Walk the chunks under a `RIFF`/`LIST` whose form word is `parent`;
    /// `depth` is 1 for the children of the top-level `RIFF`.
    fn walk(
        &mut self,
        data: &[u8],
        range: Range<usize>,
        depth: usize,
        parent: [u8; 4],
        nodes: &mut usize,
    ) -> Result<(), FormatError> {
        if depth > MAX_DEPTH {
            return Err(FormatError::InvalidValue {
                offset: range.start,
                field: "depth",
                value: depth as u64,
                reason: "RIFF nesting exceeds sanity bound",
            });
        }
        for c in children(data, range, nodes)? {
            if is_container(&c.id) {
                let Some(form) = form_of(data, &c) else {
                    continue;
                };
                match &form {
                    b"DMRF" => {
                        let site = match &parent {
                            b"lbin" => RefSite::BandInstrument,
                            b"strf" => RefSite::StyleTrack,
                            b"DMTK" => RefSite::Track,
                            _ => RefSite::Other,
                        };
                        self.parse_reference(data, c.body.start + 4..c.body.end, site, nodes)?;
                        continue;
                    }
                    b"DMBD" => self.content.bands += 1,
                    b"part" if &parent == b"DMST" => self.content.parts += 1,
                    b"pttn" if &parent == b"DMST" => self.content.patterns += 1,
                    _ => {}
                }
                self.walk(data, c.body.start + 4..c.body.end, depth + 1, form, nodes)?;
                continue;
            }
            let body = &data[c.body.clone()];
            // Fixed-layout records: flag a short one and skip it.
            let need = match (&c.id, &parent) {
                (b"guid", _) if depth == 1 => 16,
                (b"vers", _) if depth == 1 => 8,
                (b"segh", b"DMSG") => 24,
                (b"trkh", b"DMTK") => 32,
                (b"styh", b"DMST") => 12,
                (b"bins", b"lbin") => 32,
                _ => 0,
            };
            if self.short(&c.id, body.len(), need) {
                continue;
            }
            match (&c.id, &parent) {
                (b"guid", _) if depth == 1 => self.guid = Some(body[..16].try_into().unwrap()),
                (b"vers", _) if depth == 1 => self.version = Some((le32(body, 0), le32(body, 4))),
                (b"UNAM", b"UNFO") if depth == 2 => {
                    if body.len() % 2 == 1 {
                        self.issues.push(DmIssue::OddString { id: "UNAM".into() });
                    }
                    self.name = Some(utf16_text(body));
                }
                (b"segh", b"DMSG") => {
                    self.content.segment = Some(SegmentHeader {
                        repeats: le32(body, 0),
                        length: le32(body, 4),
                        play_start: le32(body, 8),
                        loop_start: le32(body, 12),
                        loop_end: le32(body, 16),
                        resolution: le32(body, 20),
                    });
                }
                (b"trkh", b"DMTK") => {
                    let ckid: [u8; 4] = body[24..28].try_into().unwrap();
                    let fcc: [u8; 4] = body[28..32].try_into().unwrap();
                    self.content.tracks.push(Track {
                        kind: if ckid == [0; 4] { fcc } else { ckid },
                        class_guid: body[..16].try_into().unwrap(),
                        group: le32(body, 20),
                        position: le32(body, 16),
                    });
                }
                (b"styh", b"DMST") => {
                    self.content.style = Some(StyleHeader {
                        beats_per_measure: body[0],
                        beat: body[1],
                        grids_per_beat: u16::from_le_bytes([body[2], body[3]]),
                        tempo: f64::from_le_bytes(body[4..12].try_into().unwrap()),
                    });
                }
                (b"bins", b"lbin") => {
                    let patch = le32(body, 0);
                    self.content.instruments.push(BandInstrument {
                        bank_msb: (patch >> 16) as u8,
                        bank_lsb: (patch >> 8) as u8,
                        program: patch as u8,
                        pchannel: le32(body, 24),
                        flags: le32(body, 28),
                    });
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// A `DMRF` list: `refh`, `guid`, `name`, `file` (UTF-16), `catg`, `vers`.
    fn parse_reference(
        &mut self,
        data: &[u8],
        range: Range<usize>,
        site: RefSite,
        nodes: &mut usize,
    ) -> Result<(), FormatError> {
        let mut reference = DmReference {
            site,
            name: None,
            file: None,
            guid: None,
        };
        for c in children(data, range, nodes)? {
            let body = &data[c.body.clone()];
            match &c.id {
                b"guid" if body.len() >= 16 => {
                    reference.guid = Some(body[..16].try_into().unwrap())
                }
                b"name" => reference.name = Some(utf16_text(body)),
                b"file" => {
                    if body.len() % 2 == 1 {
                        self.issues.push(DmIssue::OddString { id: "file".into() });
                    }
                    reference.file = Some(utf16_text(body));
                }
                _ => {}
            }
        }
        if reference.file.as_deref().is_none_or(str::is_empty) {
            self.issues.push(DmIssue::ReferenceWithoutFile);
        }
        self.references.push(reference);
        Ok(())
    }

    fn finish_dm(&mut self) {
        match self.kind {
            DmKind::Segment => {
                if self.content.segment.is_none() {
                    self.issues.push(DmIssue::MissingSegmentHeader);
                }
                if self.content.tracks.is_empty() {
                    self.issues.push(DmIssue::NoTracks);
                }
            }
            DmKind::Style if self.content.style.is_none() => {
                self.issues.push(DmIssue::MissingStyleHeader);
            }
            _ => {}
        }
    }

    /// DLS Level 1: `colh`, `lins`, `ptbl`, `wvpl`, top-level `INFO`.
    fn parse_dls(
        &mut self,
        data: &[u8],
        range: Range<usize>,
        nodes: &mut usize,
    ) -> Result<(), FormatError> {
        let mut dls = DlsContent::default();
        let mut pool_start = None;
        let mut offsets_of_waves: Vec<usize> = Vec::new();
        for c in children(data, range, nodes)? {
            let body = &data[c.body.clone()];
            if is_container(&c.id) {
                let Some(form) = form_of(data, &c) else {
                    continue;
                };
                let inner = c.body.start + 4..c.body.end;
                match &form {
                    b"lins" => {
                        for ins in children(data, inner, nodes)? {
                            if is_container(&ins.id)
                                && form_of(data, &ins).as_ref() == Some(b"ins ")
                            {
                                let i = dls.instruments.len();
                                let parsed = self.parse_dls_instrument(
                                    data,
                                    ins.body.start + 4..ins.body.end,
                                    i,
                                    nodes,
                                )?;
                                dls.instruments.push(parsed);
                            }
                        }
                    }
                    b"wvpl" => {
                        pool_start = Some(inner.start);
                        for w in children(data, inner, nodes)? {
                            if is_container(&w.id) && form_of(data, &w).as_ref() == Some(b"wave") {
                                offsets_of_waves.push(w.body.start - 8);
                                let wave = self.parse_dls_wave(
                                    data,
                                    w.body.start + 4..w.body.end,
                                    dls.waves.len(),
                                    nodes,
                                )?;
                                dls.waves.push(wave);
                            }
                        }
                    }
                    b"INFO" => {
                        for e in children(data, inner, nodes)? {
                            if &e.id == b"INAM" {
                                self.name = Some(ascii_text(&data[e.body.clone()]));
                            }
                        }
                    }
                    _ => {}
                }
                continue;
            }
            match &c.id {
                b"colh" if !self.short(&c.id, body.len(), 4) => {
                    dls.declared_instruments = Some(le32(body, 0));
                }
                b"dlid" if !self.short(&c.id, body.len(), 16) => {
                    self.guid = Some(body[..16].try_into().unwrap());
                }
                b"vers" if !self.short(&c.id, body.len(), 8) => {
                    self.version = Some((le32(body, 0), le32(body, 4)));
                }
                b"ptbl" if !self.short(&c.id, body.len(), 8) => {
                    let declared = le32(body, 4) as usize;
                    let table = body[8..].as_chunks::<4>().0;
                    dls.pool_cues = table
                        .iter()
                        .take(declared)
                        .map(|b| u32::from_le_bytes(*b))
                        .collect();
                    if dls.pool_cues.len() != declared {
                        self.issues.push(DmIssue::ShortChunk {
                            id: "ptbl".into(),
                            len: body.len(),
                            need: declared.saturating_mul(4).saturating_add(8),
                        });
                    }
                }
                _ => {}
            }
        }
        if let Some(declared) = dls.declared_instruments
            && declared as usize != dls.instruments.len()
        {
            self.issues.push(DmIssue::InstrumentCountMismatch {
                declared,
                found: dls.instruments.len(),
            });
        }
        if dls.pool_cues.len() != dls.waves.len()
            && !(dls.pool_cues.is_empty() && pool_start.is_none())
        {
            self.issues.push(DmIssue::PoolCueCountMismatch {
                cues: dls.pool_cues.len(),
                waves: dls.waves.len(),
            });
        }
        // Each cue is an offset from the first byte after the pool's form
        // word to a `LIST wave` header.
        if let Some(start) = pool_start {
            for (cue, &offset) in dls.pool_cues.iter().enumerate() {
                let at = start.saturating_add(offset as usize);
                if !offsets_of_waves.contains(&at) {
                    self.issues.push(DmIssue::PoolCueOutOfRange { cue, offset });
                }
            }
        }
        let cue_count = dls.pool_cues.len();
        for (i, ins) in dls.instruments.iter().enumerate() {
            for (region, link) in ins.region_links.iter().enumerate() {
                if let Some(index) = *link
                    && index as usize >= cue_count
                {
                    self.issues.push(DmIssue::DanglingWaveLink {
                        instrument: i,
                        region,
                        index,
                    });
                }
            }
        }
        self.dls = Some(dls);
        Ok(())
    }

    fn parse_dls_instrument(
        &mut self,
        data: &[u8],
        range: Range<usize>,
        index: usize,
        nodes: &mut usize,
    ) -> Result<DlsInstrument, FormatError> {
        let mut ins = DlsInstrument {
            name: None,
            bank: 0,
            program: 0,
            declared_regions: 0,
            region_links: Vec::new(),
        };
        for c in children(data, range, nodes)? {
            let body = &data[c.body.clone()];
            if is_container(&c.id) {
                let Some(form) = form_of(data, &c) else {
                    continue;
                };
                let inner = c.body.start + 4..c.body.end;
                match &form {
                    b"lrgn" => {
                        for rgn in children(data, inner, nodes)? {
                            if !is_container(&rgn.id) {
                                continue;
                            }
                            let mut link = None;
                            for e in children(data, rgn.body.start + 4..rgn.body.end, nodes)? {
                                if &e.id == b"wlnk" && e.body.len() >= 12 {
                                    link = Some(le32(&data[e.body.clone()], 8));
                                }
                            }
                            ins.region_links.push(link);
                        }
                    }
                    b"INFO" => {
                        for e in children(data, inner, nodes)? {
                            if &e.id == b"INAM" {
                                ins.name = Some(ascii_text(&data[e.body.clone()]));
                            }
                        }
                    }
                    _ => {}
                }
            } else if &c.id == b"insh" && !self.short(&c.id, body.len(), 12) {
                ins.declared_regions = le32(body, 0);
                ins.bank = le32(body, 4);
                ins.program = le32(body, 8);
            }
        }
        if ins.declared_regions as usize != ins.region_links.len() {
            self.issues.push(DmIssue::RegionCountMismatch {
                instrument: index,
                declared: ins.declared_regions,
                found: ins.region_links.len(),
            });
        }
        Ok(ins)
    }

    fn parse_dls_wave(
        &mut self,
        data: &[u8],
        range: Range<usize>,
        index: usize,
        nodes: &mut usize,
    ) -> Result<DlsWave, FormatError> {
        let mut format = None;
        let mut payload = None;
        let mut name = None;
        for c in children(data, range, nodes)? {
            match &c.id {
                b"fmt " if format.is_none() => {
                    format = Some(parse_fmt(&data[c.body.clone()], c.body.start)?);
                }
                b"data" if payload.is_none() => payload = Some(c.body.clone()),
                b"LIST" if form_of(data, &c).as_ref() == Some(b"INFO") => {
                    for e in children(data, c.body.start + 4..c.body.end, nodes)? {
                        if &e.id == b"INAM" {
                            name = Some(ascii_text(&data[e.body.clone()]));
                        }
                    }
                }
                _ => {}
            }
        }
        let (Some(format), Some(data)) = (format, payload) else {
            self.issues.push(DmIssue::IncompleteWave { wave: index });
            // Keep the pool indexable: an empty PCM placeholder would
            // read as audio, so use a format no decoder accepts.
            return Ok(DlsWave {
                format: WaveFormat {
                    tag: 0,
                    channels: 0,
                    sample_rate: 0,
                    byte_rate: 0,
                    block_align: 0,
                    bits_per_sample: 0,
                    extra: Vec::new(),
                },
                data: 0..0,
                name,
            });
        };
        if format.tag != crate::wav::FORMAT_PCM || format.bits_per_sample != 16 {
            self.issues.push(DmIssue::UnsupportedWave {
                wave: index,
                tag: format.tag,
                bits: format.bits_per_sample,
            });
        }
        Ok(DlsWave { format, data, name })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(id: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut v = id.to_vec();
        v.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        v.extend_from_slice(payload);
        if payload.len() % 2 == 1 {
            v.push(0);
        }
        v
    }

    fn form(kind: &[u8; 4], form: &[u8; 4], parts: &[Vec<u8>]) -> Vec<u8> {
        let mut body = form.to_vec();
        for p in parts {
            body.extend_from_slice(p);
        }
        chunk(kind, &body)
    }

    fn list(f: &[u8; 4], parts: &[Vec<u8>]) -> Vec<u8> {
        form(b"LIST", f, parts)
    }

    fn riff(f: &[u8; 4], parts: &[Vec<u8>]) -> Vec<u8> {
        form(b"RIFF", f, parts)
    }

    fn utf16(s: &str) -> Vec<u8> {
        s.encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect()
    }

    fn u32s(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    fn reference(name: &str, file: &str) -> Vec<u8> {
        list(
            b"DMRF",
            &[
                chunk(b"refh", &[0; 20]),
                chunk(b"guid", &[7; 16]),
                chunk(b"name", &utf16(name)),
                chunk(b"file", &utf16(file)),
            ],
        )
    }

    fn track(ckid: &[u8; 4], body: Vec<Vec<u8>>) -> Vec<u8> {
        let mut hdr = vec![9u8; 16];
        hdr.extend_from_slice(&u32s(&[100, 3]));
        hdr.extend_from_slice(ckid);
        hdr.extend_from_slice(&[0; 4]);
        let mut parts = vec![chunk(b"trkh", &hdr)];
        parts.extend(body);
        riff(b"DMTK", &parts)
    }

    fn segh(repeats: u32, length: u32) -> Vec<u8> {
        chunk(b"segh", &u32s(&[repeats, length, 0, 0, 0, 4]))
    }

    fn sample_segment() -> Vec<u8> {
        riff(
            b"DMSG",
            &[
                segh(2, 7680),
                chunk(b"guid", &[1; 16]),
                chunk(b"vers", &u32s(&[1, 2])),
                list(b"UNFO", &[chunk(b"UNAM", &utf16("Enemy Start"))]),
                list(
                    b"trkl",
                    &[
                        track(b"cmnd", vec![chunk(b"cmnd", &[0; 8])]),
                        track(b"tetr", vec![chunk(b"tetr", &[0; 8])]),
                        track(
                            b"sttr",
                            vec![list(
                                b"sttr",
                                &[list(
                                    b"strf",
                                    &[
                                        chunk(b"stmp", &[0; 4]),
                                        reference("Enemy Style", "enemystyle.sty"),
                                    ],
                                )],
                            )],
                        ),
                    ],
                ),
            ],
        )
    }

    #[test]
    fn a_segment_reports_its_header_name_tracks_and_style_reference() {
        let c = DmContainer::parse(&sample_segment()).unwrap();
        assert_eq!(c.kind, DmKind::Segment);
        assert_eq!(c.name.as_deref(), Some("Enemy Start"));
        assert_eq!(c.guid, Some([1; 16]));
        assert_eq!(c.version, Some((1, 2)));
        let h = c.content.segment.unwrap();
        assert_eq!((h.repeats, h.length, h.resolution), (2, 7680, 4));
        let kinds: Vec<_> = c.content.tracks.iter().map(Track::kind_name).collect();
        assert_eq!(kinds, ["command", "tempo", "style"]);
        assert_eq!(c.content.tracks[0].position, 100);
        assert_eq!(c.content.tracks[0].group, 3);
        assert_eq!(c.references.len(), 1);
        assert_eq!(c.references[0].site, RefSite::StyleTrack);
        assert_eq!(c.references[0].file.as_deref(), Some("enemystyle.sty"));
        assert_eq!(c.references[0].name.as_deref(), Some("Enemy Style"));
        assert_eq!(c.references[0].guid, Some([7; 16]));
        assert!(c.issues.is_empty(), "{:?}", c.issues);
    }

    #[test]
    fn a_segment_without_header_or_tracks_is_flagged_not_rejected() {
        let c = DmContainer::parse(&riff(b"DMSG", &[])).unwrap();
        assert!(c.issues.contains(&DmIssue::MissingSegmentHeader));
        assert!(c.issues.contains(&DmIssue::NoTracks));
    }

    #[test]
    fn a_style_counts_parts_patterns_and_its_embedded_band() {
        let mut styh = vec![4, 4, 4, 0];
        styh.extend_from_slice(&120.0f64.to_le_bytes());
        let mut ins = u32s(&[0x0001_0203, 0, 0, 0, 0, 0, 5, 0x21]);
        ins.extend_from_slice(&[0; 8]);
        let band = riff(
            b"DMBD",
            &[list(
                b"lbil",
                &[list(
                    b"lbin",
                    &[
                        chunk(b"bins", &ins),
                        reference("DLS Collection1", "dls collection1.dls"),
                    ],
                )],
            )],
        );
        let sty = riff(
            b"DMST",
            &[
                chunk(b"styh", &styh),
                band,
                list(b"part", &[chunk(b"prth", &[0; 4])]),
                list(b"part", &[]),
                list(b"pttn", &[]),
            ],
        );
        let c = DmContainer::parse(&sty).unwrap();
        assert_eq!(c.kind, DmKind::Style);
        let h = c.content.style.unwrap();
        assert_eq!((h.beats_per_measure, h.beat, h.grids_per_beat), (4, 4, 4));
        assert_eq!(h.tempo, 120.0);
        assert_eq!(
            (c.content.parts, c.content.patterns, c.content.bands),
            (2, 1, 1)
        );
        assert_eq!(c.content.instruments.len(), 1);
        let i = c.content.instruments[0];
        assert_eq!((i.bank_msb, i.bank_lsb, i.program), (1, 2, 3));
        assert_eq!((i.pchannel, i.flags), (5, 0x21));
        assert_eq!(c.references[0].site, RefSite::BandInstrument);
        assert_eq!(c.references[0].file.as_deref(), Some("dls collection1.dls"));
        assert!(c.issues.is_empty(), "{:?}", c.issues);
    }

    #[test]
    fn a_style_without_a_header_is_flagged() {
        let c = DmContainer::parse(&riff(b"DMST", &[])).unwrap();
        assert!(c.issues.contains(&DmIssue::MissingStyleHeader));
    }

    #[test]
    fn a_reference_without_a_file_is_flagged() {
        let bnd = riff(
            b"DMBD",
            &[list(
                b"lbil",
                &[list(
                    b"lbin",
                    &[list(b"DMRF", &[chunk(b"name", &utf16("Orphan"))])],
                )],
            )],
        );
        let c = DmContainer::parse(&bnd).unwrap();
        assert_eq!(c.content.bands, 1);
        assert!(c.issues.contains(&DmIssue::ReferenceWithoutFile));
    }

    fn pcm_wave(rate: u32, frames: usize, tag: u16) -> Vec<u8> {
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&tag.to_le_bytes());
        fmt.extend_from_slice(&1u16.to_le_bytes());
        fmt.extend_from_slice(&rate.to_le_bytes());
        fmt.extend_from_slice(&(rate * 2).to_le_bytes());
        fmt.extend_from_slice(&2u16.to_le_bytes());
        fmt.extend_from_slice(&16u16.to_le_bytes());
        let data: Vec<u8> = (0..frames as i16)
            .flat_map(|s| (s * 3).to_le_bytes())
            .collect();
        list(b"wave", &[chunk(b"fmt ", &fmt), chunk(b"data", &data)])
    }

    fn region(link: Option<u32>) -> Vec<u8> {
        let mut parts = vec![chunk(b"rgnh", &[0; 12])];
        if let Some(i) = link {
            parts.push(chunk(b"wlnk", &u32s(&[0, 1, i])));
        }
        list(b"rgn ", &parts)
    }

    fn instrument(program: u32, regions: &[Option<u32>], declared: u32) -> Vec<u8> {
        let rgns: Vec<_> = regions.iter().map(|l| region(*l)).collect();
        list(
            b"ins ",
            &[
                chunk(b"insh", &u32s(&[declared, 0, program])),
                list(b"lrgn", &rgns),
                list(b"INFO", &[chunk(b"INAM", b"Piano\0")]),
            ],
        )
    }

    /// A DLS bank whose `ptbl` cues are computed from the real wave sizes.
    fn dls_with(
        waves: &[Vec<u8>],
        cues: Option<Vec<u32>>,
        instruments: Vec<Vec<u8>>,
        colh: u32,
    ) -> Vec<u8> {
        let mut off = 0u32;
        let mut computed = Vec::new();
        for w in waves {
            computed.push(off);
            off += w.len() as u32;
        }
        let cues = cues.unwrap_or(computed);
        let mut ptbl = u32s(&[8, cues.len() as u32]);
        ptbl.extend_from_slice(&u32s(&cues));
        riff(
            b"DLS ",
            &[
                chunk(b"dlid", &[3; 16]),
                chunk(b"colh", &u32s(&[colh])),
                list(b"lins", &instruments),
                chunk(b"ptbl", &ptbl),
                list(b"wvpl", waves),
                list(b"INFO", &[chunk(b"INAM", b"Bank\0")]),
            ],
        )
    }

    #[test]
    fn a_dls_bank_exposes_instruments_regions_and_pcm_waves() {
        let bytes = dls_with(
            &[pcm_wave(22050, 4, 1), pcm_wave(11025, 6, 1)],
            None,
            vec![instrument(5, &[Some(0), Some(1)], 2)],
            1,
        );
        let c = DmContainer::parse(&bytes).unwrap();
        assert_eq!(c.kind, DmKind::Dls);
        assert_eq!(c.name.as_deref(), Some("Bank"));
        assert_eq!(c.guid, Some([3; 16]));
        let d = c.dls.as_ref().unwrap();
        assert_eq!(d.instruments.len(), 1);
        assert_eq!(d.instruments[0].program, 5);
        assert_eq!(d.instruments[0].name.as_deref(), Some("Piano"));
        assert_eq!(d.instruments[0].region_links, [Some(0), Some(1)]);
        assert_eq!(d.waves.len(), 2);
        assert_eq!(d.waves[0].format.sample_rate, 22050);
        assert_eq!(d.waves[1].frames(), Some(6));
        assert_eq!(d.waves[0].samples_i16(&bytes).unwrap(), [0, 3, 6, 9]);
        assert!(c.issues.is_empty(), "{:?}", c.issues);
    }

    #[test]
    fn dls_count_cue_and_link_mismatches_are_flagged() {
        let waves = [pcm_wave(22050, 4, 1), pcm_wave(22050, 4, 1)];
        // colh says 2 instruments, insh says 3 regions, region links wave 5,
        // and cue 1 points into the middle of nowhere.
        let bytes = dls_with(
            &waves,
            Some(vec![0, 9]),
            vec![instrument(0, &[Some(5)], 3)],
            2,
        );
        let c = DmContainer::parse(&bytes).unwrap();
        assert!(c.issues.contains(&DmIssue::InstrumentCountMismatch {
            declared: 2,
            found: 1
        }));
        assert!(c.issues.contains(&DmIssue::RegionCountMismatch {
            instrument: 0,
            declared: 3,
            found: 1
        }));
        assert!(c.issues.contains(&DmIssue::DanglingWaveLink {
            instrument: 0,
            region: 0,
            index: 5
        }));
        assert!(
            c.issues
                .contains(&DmIssue::PoolCueOutOfRange { cue: 1, offset: 9 })
        );
    }

    #[test]
    fn a_pool_with_fewer_cues_than_waves_is_flagged() {
        let waves = [pcm_wave(22050, 2, 1), pcm_wave(22050, 2, 1)];
        let bytes = dls_with(&waves, Some(vec![0]), vec![], 0);
        let c = DmContainer::parse(&bytes).unwrap();
        assert!(
            c.issues
                .contains(&DmIssue::PoolCueCountMismatch { cues: 1, waves: 2 })
        );
    }

    #[test]
    fn a_non_pcm_dls_wave_is_reported_unsupported_and_not_decoded() {
        let bytes = dls_with(&[pcm_wave(22050, 2, 0x11)], None, vec![], 0);
        let c = DmContainer::parse(&bytes).unwrap();
        assert!(c.issues.contains(&DmIssue::UnsupportedWave {
            wave: 0,
            tag: 0x11,
            bits: 16
        }));
        assert!(
            c.dls.as_ref().unwrap().waves[0]
                .samples_i16(&bytes)
                .is_none()
        );
    }

    #[test]
    fn a_dls_wave_without_data_is_flagged_incomplete() {
        let wave = list(b"wave", &[chunk(b"INFO", b"x\0")]);
        let bytes = dls_with(&[wave], None, vec![], 0);
        let c = DmContainer::parse(&bytes).unwrap();
        assert!(c.issues.contains(&DmIssue::IncompleteWave { wave: 0 }));
        let w = &c.dls.as_ref().unwrap().waves[0];
        assert!(w.samples_i16(&bytes).is_none());
    }

    #[test]
    fn structural_damage_is_an_error() {
        assert!(matches!(
            DmContainer::parse(b"WAVE0000DMSG"),
            Err(FormatError::BadMagic { offset: 0, .. })
        ));
        assert!(matches!(
            DmContainer::parse(&riff(b"WAVE", &[])),
            Err(FormatError::BadMagic { offset: 8, .. })
        ));
        assert!(DmContainer::parse(b"RIFF").is_err());
        // RIFF size past EOF.
        let mut seg = sample_segment();
        seg.truncate(seg.len() - 10);
        assert!(DmContainer::parse(&seg).is_err());
        // A child chunk claiming more than its parent holds.
        let mut bad = riff(b"DMSG", &[chunk(b"segh", &[0; 24])]);
        let at = 12 + 4;
        bad[at..at + 4].copy_from_slice(&1000u32.to_le_bytes());
        assert!(DmContainer::parse(&bad).is_err());
    }

    #[test]
    fn runaway_nesting_is_bounded() {
        let mut inner = chunk(b"zzzz", &[]);
        for _ in 0..40 {
            inner = list(b"deep", &[inner]);
        }
        let err = DmContainer::parse(&riff(b"DMSG", &[inner])).unwrap_err();
        assert!(
            matches!(err, FormatError::InvalidValue { field: "depth", .. }),
            "{err}"
        );
    }

    #[test]
    fn bytes_after_the_riff_are_reported_and_short_records_flagged() {
        let mut bytes = riff(b"DMSG", &[chunk(b"segh", &[0; 8])]);
        bytes.extend_from_slice(&[0; 6]);
        let c = DmContainer::parse(&bytes).unwrap();
        assert!(c.issues.contains(&DmIssue::TrailingBytes { len: 6 }));
        assert!(c.issues.contains(&DmIssue::ShortChunk {
            id: "segh".into(),
            len: 8,
            need: 24
        }));
        assert!(c.content.segment.is_none());
    }

    #[test]
    fn an_odd_length_name_string_is_flagged() {
        let bnd = riff(
            b"DMBD",
            &[list(b"UNFO", &[chunk(b"UNAM", &[b'A', 0, b'B'])])],
        );
        let c = DmContainer::parse(&bnd).unwrap();
        assert!(c.issues.contains(&DmIssue::OddString { id: "UNAM".into() }));
        assert_eq!(c.name.as_deref(), Some("A"));
    }

    #[test]
    fn a_nested_guid_or_name_does_not_replace_the_containers_own() {
        // The embedded band's guid/name must not be mistaken for the style's.
        let band = riff(
            b"DMBD",
            &[
                chunk(b"guid", &[8; 16]),
                list(b"UNFO", &[chunk(b"UNAM", &utf16("Inner"))]),
            ],
        );
        let sty = riff(
            b"DMST",
            &[
                chunk(b"guid", &[1; 16]),
                list(b"UNFO", &[chunk(b"UNAM", &utf16("Outer"))]),
                band,
            ],
        );
        let c = DmContainer::parse(&sty).unwrap();
        assert_eq!(c.guid, Some([1; 16]));
        assert_eq!(c.name.as_deref(), Some("Outer"));
    }

    #[test]
    fn fourcc_text_hides_control_bytes() {
        assert_eq!(fourcc_text(b"ab\x01 "), "ab? ");
    }
}
