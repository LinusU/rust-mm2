//! Parser for MM2 `BAI` ambient-navigation files (`CAI1`).
//!
//! Each city ships one primary `.bai` (`city/<name>.bai`) holding the
//! directed road network ambient traffic and pedestrians travel on: per-road
//! lane curves, intersections connecting road ends, and per-room "AI bubble"
//! culling lists naming the roads that are active while the player occupies
//! a room.
//!
//! Provenance: `angel-file-formats/Midtown Madness 2/BAI.md` (source R3),
//! measured against the retail `london.bai`/`sf.bai`/`sfai.bai` — see
//! `docs/research/bai.md`. One layout correction vs. the document: inside a
//! road side the `[lanes + sidewalks][sections]` distance matrix is stored
//! *before* the per-lane outer-edge distances, not after.

use crate::{FormatError, Reader};
use std::collections::BTreeSet;
use std::fmt;

/// File magic.
pub const BAI_MAGIC: &[u8; 4] = b"CAI1";

/// Fill value marking a [`RoadEnd`] that is not wired into its
/// intersection's road list (dead ends, unconnected stubs).
pub const END_FILL: u32 = 0xCDCD_CDCD;

/// Road flag: divided carriageway.
pub const FLAG_DIVIDED: u16 = 0x1;
/// Road flag: alleyway/walkway.
pub const FLAG_ALLEYWAY: u16 = 0x2;
/// Road flag: freeway.
pub const FLAG_FREEWAY: u16 = 0x4;
/// Road flag: flat (single constant slope — cheaper orientation checks).
pub const FLAG_FLAT: u16 = 0x8;
/// Mask of every documented road-flag bit.
pub const KNOWN_FLAGS: u16 = FLAG_DIVIDED | FLAG_ALLEYWAY | FLAG_FREEWAY | FLAG_FLAT;

// Sanity caps. Retail maxima measured 2026-09-20: 86 sections/road,
// 5 lane+sidewalk curves/side, 1 rail curve/side, 42 rooms/road,
// 1342 culling rooms.
const MAX_SECTIONS: usize = 1024;
const MAX_SIDE_CURVES: u16 = 32;
const MAX_RAIL_CURVES: u16 = 16;
const MAX_ROAD_ROOMS: usize = 4096;
const MAX_CULL_ROOMS: u32 = 1 << 20;

/// Which side of the road centre line a [`RoadSide`] describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// Right-hand side data.
    Right,
    /// Left-hand side data.
    Left,
}

impl fmt::Display for Side {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Side::Right => f.write_str("right"),
            Side::Left => f.write_str("left"),
        }
    }
}

/// Which extremity of a road a [`RoadEnd`] describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    /// Road start (stored second in the file).
    Start,
    /// Road end (stored first in the file).
    End,
}

impl fmt::Display for End {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            End::Start => f.write_str("start"),
            End::End => f.write_str("end"),
        }
    }
}

/// Ambient classes a road side permits (documented `ambientTypes` values).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmbientType {
    /// Vehicles and pedestrians.
    VehiclesAndPedestrians,
    /// Pedestrians only.
    PedestriansOnly,
    /// Vehicles only.
    VehiclesOnly,
    /// Nobody — all ambient traffic disabled on this side.
    Disabled,
}

/// How vehicles behave at a road end (documented `vehicleRule` values).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VehicleRule {
    /// Stop sign: vehicles stop, longest waiting vehicle drives first.
    StopSign,
    /// Traffic light: one road at a time. Only valid if every road at the
    /// intersection uses it.
    TrafficLight,
    /// Always stop: vehicles stop and never proceed.
    AlwaysStop,
    /// Never stop: vehicles drive straight through.
    NeverStop,
}

/// One cross-section of a road's centre frame.
#[derive(Debug, Clone)]
pub struct RoadSection {
    /// Cumulative distance along the road centre to this section.
    pub distance: f32,
    /// Centre point.
    pub origin: [f32; 3],
    /// Cross-section X axis.
    pub x_axis: [f32; 3],
    /// Cross-section Y axis.
    pub y_axis: [f32; 3],
    /// Cross-section Z axis.
    pub z_axis: [f32; 3],
    /// Travel direction at this cross-section.
    pub tangent: [f32; 3],
}

/// Per-side lane/rail/sidewalk curves of a road.
#[derive(Debug, Clone)]
pub struct RoadSide {
    /// Number of driving-lane curves.
    pub lane_count: u16,
    /// Number of tram-rail curves.
    pub tram_count: u16,
    /// Number of train-rail curves.
    pub train_count: u16,
    /// Number of sidewalk curves (always 1 on observed retail data).
    pub sidewalk_count: u16,
    /// Raw `ambientTypes` code; see [`RoadSide::ambient_type`].
    pub ambient_types: u16,
    /// Per-curve cumulative distances: `lane_distances[i][s]` is the
    /// distance along curve `i` at section `s`. Outer index is
    /// `lane_count + sidewalk_count` curves.
    pub lane_distances: Vec<Vec<f32>>,
    /// Outer-edge distance of each lane/sidewalk curve from the road
    /// centre. Stored after the distance matrix in the file.
    pub edge_distances: Vec<f32>,
    /// 40 bytes of preserved undocumented data (mostly `0xCD` fill with a
    /// `half_width`-shaped float on retail).
    pub misc: [u8; 40],
    /// Lane/sidewalk curve vertices: `[lane_count + sidewalk_count]
    /// [sections]` XYZ triples.
    pub lane_vertices: Vec<Vec<[f32; 3]>>,
    /// Tram-rail curve vertices: `[tram_count][sections]`.
    pub tram_vertices: Vec<Vec<[f32; 3]>>,
    /// Train-rail curve vertices: `[train_count][sections]`.
    pub train_vertices: Vec<Vec<[f32; 3]>>,
    /// Sidewalk inner-edge curve (one vertex per section, present even
    /// where `sidewalk_count` would be 0).
    pub sidewalk_inner: Vec<[f32; 3]>,
    /// Sidewalk outer-edge curve.
    pub sidewalk_outer: Vec<[f32; 3]>,
}

impl RoadSide {
    /// Interpret the raw `ambient_types` code. `None` for undocumented
    /// values.
    pub fn ambient_type(&self) -> Option<AmbientType> {
        match self.ambient_types {
            0 => Some(AmbientType::VehiclesAndPedestrians),
            1 => Some(AmbientType::PedestriansOnly),
            2 => Some(AmbientType::VehiclesOnly),
            3 => Some(AmbientType::Disabled),
            _ => None,
        }
    }
}

/// How one extremity of a road joins an intersection.
#[derive(Debug, Clone)]
pub struct RoadEnd {
    /// Intersection reference. On retail data this is the index of the
    /// intersection in the file's intersection list (ids are sequential);
    /// only meaningful together with [`RoadEnd::intersection_road_index`].
    pub intersection: u32,
    /// `0xCDCD` fill on retail.
    pub fill0: u16,
    /// Raw `vehicleRule` code; see [`RoadEnd::vehicle_rule`].
    pub vehicle_rule_code: u16,
    /// Undocumented.
    pub unknown1: u16,
    /// Index of this road inside `intersection`'s road list, or
    /// [`END_FILL`] when the end is not connected.
    pub intersection_road_index: u32,
    /// Traffic-light origin (lights render only when nonzero, per R3).
    pub traffic_light_origin: [f32; 3],
    /// Traffic-light orientation axis.
    pub traffic_light_axis: [f32; 3],
}

impl RoadEnd {
    /// Whether this end is wired into an intersection's road list.
    pub fn is_connected(&self) -> bool {
        self.intersection_road_index != END_FILL
    }

    /// Interpret the raw `vehicle_rule_code`. `None` for undocumented
    /// values.
    pub fn vehicle_rule(&self) -> Option<VehicleRule> {
        match self.vehicle_rule_code {
            0 => Some(VehicleRule::StopSign),
            1 => Some(VehicleRule::TrafficLight),
            2 => Some(VehicleRule::AlwaysStop),
            3 => Some(VehicleRule::NeverStop),
            _ => None,
        }
    }
}

/// One ambient-traffic route: a directed strip of road with lane curves on
/// both sides.
#[derive(Debug, Clone)]
pub struct Road {
    /// Road identifier (sequential with the file order on retail).
    pub id: u16,
    /// Raw flag bits; see `FLAG_*`/`KNOWN_FLAGS`.
    pub flags: u16,
    /// PSDL rooms the road passes through (room index + 1).
    pub rooms: Vec<u16>,
    /// Half of the road width.
    pub half_width: f32,
    /// Base vehicle speed on this road. R3 notes race `.aimap` files
    /// override it entirely.
    pub base_speed: f32,
    /// Right-side lane data.
    pub right: RoadSide,
    /// Left-side lane data.
    pub left: RoadSide,
    /// Centre-line cross-sections.
    pub sections: Vec<RoadSection>,
    /// End-of-road junction (stored first in the file).
    pub end: RoadEnd,
    /// Start-of-road junction.
    pub start: RoadEnd,
}

/// An intersection: a PSDL room plus the roads meeting there.
#[derive(Debug, Clone)]
pub struct Intersection {
    /// Intersection identifier (sequential on retail).
    pub id: u16,
    /// PSDL room hosting the intersection (room index + 1).
    pub room: u16,
    /// Centre point.
    pub center: [f32; 3],
    /// Connected roads in counterclockwise order. Values index into
    /// [`Bai::roads`] (ids are sequential on retail, so the two readings
    /// coincide — see `docs/research/bai.md`).
    pub roads: Vec<u32>,
}

/// Per-room "AI bubble" culling lists: which roads ambient simulation
/// computes for while the player occupies each room.
#[derive(Debug, Clone)]
pub struct Culling {
    /// Large AI bubbles, one road list per PSDL room (+ 1 empty leading
    /// entry).
    pub large: Vec<Vec<u16>>,
    /// Small AI bubbles.
    pub small: Vec<Vec<u16>>,
}

/// A parsed BAI file.
#[derive(Debug, Clone)]
pub struct Bai {
    /// All ambient routes.
    pub roads: Vec<Road>,
    /// All intersections.
    pub intersections: Vec<Intersection>,
    /// Per-room culling lists.
    pub culling: Culling,
}

/// A consistency problem found by [`Bai::validate`]. Parsing only checks
/// structural bounds; cross-reference integrity is reported here so good
/// data with authored quirks still loads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BaiIssue {
    /// Two roads share an id.
    DuplicateRoadId(u16),
    /// Two intersections share an id.
    DuplicateIntersectionId(u16),
    /// A road carries flag bits outside `KNOWN_FLAGS`.
    UnknownRoadFlags {
        /// Road index.
        road: usize,
        /// Raw flag word.
        flags: u16,
    },
    /// A road side carries an undocumented `ambientTypes` code.
    UnknownAmbientTypes {
        /// Road index.
        road: usize,
        /// Which side.
        side: Side,
        /// Raw code.
        value: u16,
    },
    /// A road end carries an undocumented `vehicleRule` code.
    UnknownVehicleRule {
        /// Road index.
        road: usize,
        /// Which end.
        end: End,
        /// Raw code.
        value: u16,
    },
    /// A road has fewer than two cross-sections and cannot describe a
    /// direction of travel.
    TooFewSections {
        /// Road index.
        road: usize,
        /// Section count.
        sections: usize,
    },
    /// A road or intersection references PSDL room 0; rooms are stored
    /// index + 1 and 0 never appears on retail data.
    ZeroRoomReference {
        /// What carries the reference.
        subject: &'static str,
        /// Road or intersection index.
        index: usize,
    },
    /// A connected road end names an intersection outside the file.
    DanglingEndIntersection {
        /// Road index.
        road: usize,
        /// Which end.
        end: End,
        /// Raw reference.
        intersection: u32,
    },
    /// `intersection_road_index` is not [`END_FILL`] but exceeds the named
    /// intersection's road list.
    EndIndexOutOfRange {
        /// Road index.
        road: usize,
        /// Which end.
        end: End,
        /// Raw index.
        index: u32,
        /// Roads connected to that intersection.
        connected: usize,
    },
    /// `intersection.roads[index]` does not point back at this road.
    EndIndexMismatch {
        /// Road index.
        road: usize,
        /// Which end.
        end: End,
        /// Raw index.
        index: u32,
        /// Road found at that position.
        found: u32,
    },
    /// An intersection references a road outside the file.
    DanglingIntersectionRoad {
        /// Intersection index.
        intersection: usize,
        /// Raw reference.
        road: u32,
    },
    /// An intersection references a road that does not name this
    /// intersection at either end.
    IntersectionNotReferencedBack {
        /// Intersection index.
        intersection: usize,
        /// Referenced road index.
        road: u32,
    },
    /// A culling list references a road outside the file.
    DanglingCullingRoad {
        /// PSDL room the list belongs to.
        room: usize,
        /// Which bubble (`true` = large, `false` = small).
        large: bool,
        /// Raw reference.
        road: u16,
    },
}

impl fmt::Display for BaiIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BaiIssue::DuplicateRoadId(id) => write!(f, "duplicate road id {id}"),
            BaiIssue::DuplicateIntersectionId(id) => {
                write!(f, "duplicate intersection id {id}")
            }
            BaiIssue::UnknownRoadFlags { road, flags } => {
                write!(f, "road {road}: unknown flag bits {flags:#06x}")
            }
            BaiIssue::UnknownAmbientTypes { road, side, value } => {
                write!(f, "road {road} {side}: unknown ambientTypes {value}")
            }
            BaiIssue::UnknownVehicleRule { road, end, value } => {
                write!(f, "road {road} {end}: unknown vehicleRule {value}")
            }
            BaiIssue::TooFewSections { road, sections } => {
                write!(f, "road {road}: only {sections} section(s)")
            }
            BaiIssue::ZeroRoomReference { subject, index } => {
                write!(
                    f,
                    "{subject} {index}: room reference 0 (rooms are index + 1)"
                )
            }
            BaiIssue::DanglingEndIntersection {
                road,
                end,
                intersection,
            } => write!(
                f,
                "road {road} {end}: connected to missing intersection {intersection}"
            ),
            BaiIssue::EndIndexOutOfRange {
                road,
                end,
                index,
                connected,
            } => write!(
                f,
                "road {road} {end}: road-list index {index} but intersection has {connected} road(s)"
            ),
            BaiIssue::EndIndexMismatch {
                road,
                end,
                index,
                found,
            } => write!(
                f,
                "road {road} {end}: intersection road-list[{index}] is road {found}, not this road"
            ),
            BaiIssue::DanglingIntersectionRoad { intersection, road } => {
                write!(f, "intersection {intersection}: missing road {road}")
            }
            BaiIssue::IntersectionNotReferencedBack { intersection, road } => write!(
                f,
                "intersection {intersection}: road {road} does not name this intersection at either end"
            ),
            BaiIssue::DanglingCullingRoad { room, large, road } => {
                let bubble = if *large { "large" } else { "small" };
                write!(
                    f,
                    "culling room {room} ({bubble} bubble): missing road {road}"
                )
            }
        }
    }
}

impl Bai {
    /// Parse a complete `CAI1` file. Trailing bytes are an error — every
    /// byte of a BAI file is accounted for by the three sections.
    pub fn parse(data: &[u8]) -> Result<Self, FormatError> {
        let mut r = Reader::new(data);
        let magic = r.bytes(4)?;
        if magic != BAI_MAGIC {
            return Err(FormatError::BadMagic {
                offset: 0,
                expected: "CAI1",
                found: magic.to_vec(),
            });
        }
        let n_intersections = r.u16()? as usize;
        let n_roads = r.u16()? as usize;

        let mut roads = Vec::with_capacity(n_roads);
        for _ in 0..n_roads {
            roads.push(parse_road(&mut r)?);
        }

        let mut intersections = Vec::with_capacity(n_intersections);
        for _ in 0..n_intersections {
            intersections.push(parse_intersection(&mut r)?);
        }

        let culling = parse_culling(&mut r)?;

        if !r.is_empty() {
            return Err(FormatError::parse(
                r.pos(),
                format!("{} trailing byte(s) after culling section", r.remaining()),
            ));
        }
        Ok(Bai {
            roads,
            intersections,
            culling,
        })
    }

    /// Check internal cross-reference integrity: ids, references between
    /// road ends and intersection road lists, room-reference zero, and
    /// culling lists. See [`BaiIssue`].
    pub fn validate(&self) -> Vec<BaiIssue> {
        let mut issues = Vec::new();

        let mut seen = BTreeSet::new();
        for road in &self.roads {
            if !seen.insert(road.id) {
                issues.push(BaiIssue::DuplicateRoadId(road.id));
            }
        }
        let mut seen = BTreeSet::new();
        for i in &self.intersections {
            if !seen.insert(i.id) {
                issues.push(BaiIssue::DuplicateIntersectionId(i.id));
            }
        }

        for (ri, road) in self.roads.iter().enumerate() {
            if road.flags & !KNOWN_FLAGS != 0 {
                issues.push(BaiIssue::UnknownRoadFlags {
                    road: ri,
                    flags: road.flags,
                });
            }
            for (side, s) in [(Side::Right, &road.right), (Side::Left, &road.left)] {
                if s.ambient_type().is_none() {
                    issues.push(BaiIssue::UnknownAmbientTypes {
                        road: ri,
                        side,
                        value: s.ambient_types,
                    });
                }
            }
            if road.sections.len() < 2 {
                issues.push(BaiIssue::TooFewSections {
                    road: ri,
                    sections: road.sections.len(),
                });
            }
            if road.rooms.contains(&0) {
                issues.push(BaiIssue::ZeroRoomReference {
                    subject: "road",
                    index: ri,
                });
            }
            for (end, e) in [(End::End, &road.end), (End::Start, &road.start)] {
                if e.vehicle_rule().is_none() {
                    issues.push(BaiIssue::UnknownVehicleRule {
                        road: ri,
                        end,
                        value: e.vehicle_rule_code,
                    });
                }
                if !e.is_connected() {
                    continue;
                }
                let Some(int) = self.intersections.get(e.intersection as usize) else {
                    issues.push(BaiIssue::DanglingEndIntersection {
                        road: ri,
                        end,
                        intersection: e.intersection,
                    });
                    continue;
                };
                let idx = e.intersection_road_index as usize;
                match int.roads.get(idx) {
                    None => issues.push(BaiIssue::EndIndexOutOfRange {
                        road: ri,
                        end,
                        index: e.intersection_road_index,
                        connected: int.roads.len(),
                    }),
                    Some(&found) if found as usize != ri => {
                        issues.push(BaiIssue::EndIndexMismatch {
                            road: ri,
                            end,
                            index: e.intersection_road_index,
                            found,
                        });
                    }
                    Some(_) => {}
                }
            }
        }

        for (ii, int) in self.intersections.iter().enumerate() {
            if int.room == 0 {
                issues.push(BaiIssue::ZeroRoomReference {
                    subject: "intersection",
                    index: ii,
                });
            }
            for &rref in &int.roads {
                match self.roads.get(rref as usize) {
                    None => issues.push(BaiIssue::DanglingIntersectionRoad {
                        intersection: ii,
                        road: rref,
                    }),
                    Some(road)
                        if ![&road.start, &road.end]
                            .into_iter()
                            .any(|e| e.is_connected() && e.intersection as usize == ii) =>
                    {
                        issues.push(BaiIssue::IntersectionNotReferencedBack {
                            intersection: ii,
                            road: rref,
                        });
                    }
                    Some(_) => {}
                }
            }
        }

        for (room, list) in self.culling.large.iter().enumerate() {
            for &rref in list {
                if rref as usize >= self.roads.len() {
                    issues.push(BaiIssue::DanglingCullingRoad {
                        room,
                        large: true,
                        road: rref,
                    });
                }
            }
        }
        for (room, list) in self.culling.small.iter().enumerate() {
            for &rref in list {
                if rref as usize >= self.roads.len() {
                    issues.push(BaiIssue::DanglingCullingRoad {
                        room,
                        large: false,
                        road: rref,
                    });
                }
            }
        }

        issues
    }
}

fn checked_u16(
    value: u16,
    offset: usize,
    field: &'static str,
    max: u16,
) -> Result<usize, FormatError> {
    if value > max {
        return Err(FormatError::InvalidValue {
            offset,
            field,
            value: value as u64,
            reason: "implausible count",
        });
    }
    Ok(value as usize)
}

fn parse_road(r: &mut Reader<'_>) -> Result<Road, FormatError> {
    let id = r.u16()?;
    let n_sections = checked_u16(r.u16()?, r.pos() - 2, "nSections", MAX_SECTIONS as u16)?;
    let flags = r.u16()?;
    let n_rooms = checked_u16(r.u16()?, r.pos() - 2, "nRooms", MAX_ROAD_ROOMS as u16)?;
    let mut rooms = Vec::with_capacity(n_rooms);
    for _ in 0..n_rooms {
        rooms.push(r.u16()?);
    }
    let half_width = r.f32()?;
    let base_speed = r.f32()?;

    let right = parse_side(r, n_sections)?;
    let left = parse_side(r, n_sections)?;

    // Per-section data is stored as six parallel arrays, not interleaved.
    let mut distances = Vec::with_capacity(n_sections);
    for _ in 0..n_sections {
        distances.push(r.f32()?);
    }
    let mut origins = Vec::with_capacity(n_sections);
    for _ in 0..n_sections {
        origins.push(r.vec3()?);
    }
    let mut xs = Vec::with_capacity(n_sections);
    for _ in 0..n_sections {
        xs.push(r.vec3()?);
    }
    let mut ys = Vec::with_capacity(n_sections);
    for _ in 0..n_sections {
        ys.push(r.vec3()?);
    }
    let mut zs = Vec::with_capacity(n_sections);
    for _ in 0..n_sections {
        zs.push(r.vec3()?);
    }
    let mut tangents = Vec::with_capacity(n_sections);
    for _ in 0..n_sections {
        tangents.push(r.vec3()?);
    }
    let sections = (0..n_sections)
        .map(|i| RoadSection {
            distance: distances[i],
            origin: origins[i],
            x_axis: xs[i],
            y_axis: ys[i],
            z_axis: zs[i],
            tangent: tangents[i],
        })
        .collect();

    let end = parse_end(r)?;
    let start = parse_end(r)?;
    Ok(Road {
        id,
        flags,
        rooms,
        half_width,
        base_speed,
        right,
        left,
        sections,
        end,
        start,
    })
}

fn parse_side(r: &mut Reader<'_>, n_sections: usize) -> Result<RoadSide, FormatError> {
    let lane_count = r.u16()?;
    let tram_count = r.u16()?;
    let train_count = r.u16()?;
    let sidewalk_count = r.u16()?;
    let ambient_types = r.u16()?;
    checked_u16(lane_count, r.pos() - 10, "nLanes", MAX_SIDE_CURVES)?;
    checked_u16(tram_count, r.pos() - 8, "nTrams", MAX_RAIL_CURVES)?;
    checked_u16(train_count, r.pos() - 6, "nTrains", MAX_RAIL_CURVES)?;
    checked_u16(sidewalk_count, r.pos() - 4, "nSidewalks", MAX_SIDE_CURVES)?;
    let n = lane_count as usize + sidewalk_count as usize;

    // Measured layout: the [curve][section] distance matrix comes first,
    // then the per-curve outer-edge distances, then the misc block.
    let mut lane_distances = Vec::with_capacity(n);
    for _ in 0..n {
        let mut curve = Vec::with_capacity(n_sections);
        for _ in 0..n_sections {
            curve.push(r.f32()?);
        }
        lane_distances.push(curve);
    }
    let mut edge_distances = Vec::with_capacity(n);
    for _ in 0..n {
        edge_distances.push(r.f32()?);
    }
    let misc: [u8; 40] = r
        .bytes(40)?
        .try_into()
        .expect("slice length checked by bytes()");
    let lane_vertices = read_curves(r, n, n_sections)?;
    let tram_vertices = read_curves(r, tram_count as usize, n_sections)?;
    let train_vertices = read_curves(r, train_count as usize, n_sections)?;
    let sidewalk_inner = read_curves(r, 1, n_sections)?.remove(0);
    let sidewalk_outer = read_curves(r, 1, n_sections)?.remove(0);

    Ok(RoadSide {
        lane_count,
        tram_count,
        train_count,
        sidewalk_count,
        ambient_types,
        lane_distances,
        edge_distances,
        misc,
        lane_vertices,
        tram_vertices,
        train_vertices,
        sidewalk_inner,
        sidewalk_outer,
    })
}

fn read_curves(
    r: &mut Reader<'_>,
    curves: usize,
    n_sections: usize,
) -> Result<Vec<Vec<[f32; 3]>>, FormatError> {
    let mut out = Vec::with_capacity(curves);
    for _ in 0..curves {
        let mut curve = Vec::with_capacity(n_sections);
        for _ in 0..n_sections {
            curve.push(r.vec3()?);
        }
        out.push(curve);
    }
    Ok(out)
}

fn parse_end(r: &mut Reader<'_>) -> Result<RoadEnd, FormatError> {
    Ok(RoadEnd {
        intersection: r.u32()?,
        fill0: r.u16()?,
        vehicle_rule_code: r.u16()?,
        unknown1: r.u16()?,
        intersection_road_index: r.u32()?,
        traffic_light_origin: r.vec3()?,
        traffic_light_axis: r.vec3()?,
    })
}

fn parse_intersection(r: &mut Reader<'_>) -> Result<Intersection, FormatError> {
    let id = r.u16()?;
    let room = r.u16()?;
    let center = r.vec3()?;
    let n_roads = r.u16()? as usize;
    let mut roads = Vec::with_capacity(n_roads);
    for _ in 0..n_roads {
        roads.push(r.u32()?);
    }
    Ok(Intersection {
        id,
        room,
        center,
        roads,
    })
}

fn parse_culling(r: &mut Reader<'_>) -> Result<Culling, FormatError> {
    let n_rooms = r.u32()?;
    if n_rooms > MAX_CULL_ROOMS {
        return Err(FormatError::InvalidValue {
            offset: r.pos() - 4,
            field: "culling nRooms",
            value: n_rooms as u64,
            reason: "implausible count",
        });
    }
    let read_lists = |r: &mut Reader<'_>| -> Result<Vec<Vec<u16>>, FormatError> {
        let mut out = Vec::with_capacity(n_rooms as usize);
        for _ in 0..n_rooms {
            let n = r.u16()? as usize;
            let mut list = Vec::with_capacity(n);
            for _ in 0..n {
                list.push(r.u16()?);
            }
            out.push(list);
        }
        Ok(out)
    };
    let large = read_lists(r)?;
    let small = read_lists(r)?;
    Ok(Culling { large, small })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal valid file builder: caller describes roads as
    /// `(flags, n_sections, n_lanes_each_side)` and intersections as
    /// `(room, road refs)`; ends are auto-wired so every connected end
    /// points back at the owning road.
    struct Builder {
        roads: Vec<(u16, usize, u16)>,
        intersections: Vec<(u16, Vec<u32>)>,
        cull_rooms: usize,
    }

    fn push_v3(d: &mut Vec<u8>, v: [f32; 3]) {
        for f in v {
            d.extend_from_slice(&f.to_le_bytes());
        }
    }

    impl Builder {
        fn build(&self) -> Vec<u8> {
            let mut d = Vec::new();
            d.extend_from_slice(BAI_MAGIC);
            d.extend_from_slice(&(self.intersections.len() as u16).to_le_bytes());
            d.extend_from_slice(&(self.roads.len() as u16).to_le_bytes());
            for (i, &(flags, nsec, lanes)) in self.roads.iter().enumerate() {
                self.write_road(&mut d, i as u16, flags, nsec, lanes);
            }
            for (i, &(room, ref refs)) in self.intersections.iter().enumerate() {
                d.extend_from_slice(&(i as u16).to_le_bytes());
                d.extend_from_slice(&room.to_le_bytes());
                push_v3(&mut d, [0.0; 3]);
                d.extend_from_slice(&(refs.len() as u16).to_le_bytes());
                for r in refs {
                    d.extend_from_slice(&r.to_le_bytes());
                }
            }
            d.extend_from_slice(&(self.cull_rooms as u32).to_le_bytes());
            for _ in 0..self.cull_rooms {
                d.extend_from_slice(&1u16.to_le_bytes());
                d.extend_from_slice(&0u16.to_le_bytes());
            }
            for _ in 0..self.cull_rooms {
                d.extend_from_slice(&0u16.to_le_bytes());
            }
            d
        }

        fn write_road(&self, d: &mut Vec<u8>, id: u16, flags: u16, nsec: usize, lanes: u16) {
            d.extend_from_slice(&id.to_le_bytes());
            d.extend_from_slice(&(nsec as u16).to_le_bytes());
            d.extend_from_slice(&flags.to_le_bytes());
            d.extend_from_slice(&1u16.to_le_bytes()); // nRooms
            d.extend_from_slice(&7u16.to_le_bytes()); // room ref (index + 1)
            d.extend_from_slice(&7.5f32.to_le_bytes()); // half width
            d.extend_from_slice(&15.0f32.to_le_bytes()); // base speed
            for _ in 0..2 {
                // right, then left side
                d.extend_from_slice(&lanes.to_le_bytes());
                d.extend_from_slice(&0u16.to_le_bytes()); // trams
                d.extend_from_slice(&0u16.to_le_bytes()); // trains
                d.extend_from_slice(&1u16.to_le_bytes()); // sidewalks
                d.extend_from_slice(&0u16.to_le_bytes()); // ambientTypes
                let n = (lanes + 1) as usize;
                for c in 0..n {
                    for s in 0..nsec {
                        d.extend_from_slice(&((s * 10 + c) as f32).to_le_bytes());
                    }
                }
                for c in 0..n {
                    d.extend_from_slice(&(2.5f32 * (c + 1) as f32).to_le_bytes());
                }
                d.extend_from_slice(&[0xCDu8; 40]);
                for _ in 0..(n + 2) * nsec {
                    push_v3(d, [0.0; 3]);
                }
            }
            for s in 0..nsec {
                d.extend_from_slice(&(s as f32 * 10.0).to_le_bytes());
            }
            for _ in 0..5 * nsec {
                push_v3(d, [0.0; 3]);
            }
            for end in self.ends_for(id) {
                d.extend_from_slice(&end.intersection.to_le_bytes());
                d.extend_from_slice(&0xCDCDu16.to_le_bytes());
                d.extend_from_slice(&end.vehicle_rule_code.to_le_bytes());
                d.extend_from_slice(&0u16.to_le_bytes());
                d.extend_from_slice(&end.intersection_road_index.to_le_bytes());
                for _ in 0..2 {
                    push_v3(d, [0.0; 3]);
                }
            }
        }

        /// Wire this road's two ends to the first two intersections that
        /// list it, end first. Unwired ends get END_FILL.
        fn ends_for(&self, road: u16) -> [RoadEnd; 2] {
            let mut ends = [
                RoadEnd {
                    intersection: 0,
                    fill0: 0xCDCD,
                    vehicle_rule_code: 0,
                    unknown1: 0,
                    intersection_road_index: END_FILL,
                    traffic_light_origin: [0.0; 3],
                    traffic_light_axis: [0.0; 3],
                },
                RoadEnd {
                    intersection: 0,
                    fill0: 0xCDCD,
                    vehicle_rule_code: 0,
                    unknown1: 0,
                    intersection_road_index: END_FILL,
                    traffic_light_origin: [0.0; 3],
                    traffic_light_axis: [0.0; 3],
                },
            ];
            let mut slot = 0;
            for (ii, (_, refs)) in self.intersections.iter().enumerate() {
                if let Some(pos) = refs.iter().position(|&r| r == road as u32)
                    && slot < 2
                {
                    ends[slot].intersection = ii as u32;
                    ends[slot].intersection_road_index = pos as u32;
                    slot += 1;
                }
            }
            ends
        }
    }

    fn two_road_fixture() -> Vec<u8> {
        Builder {
            roads: vec![(FLAG_FLAT, 4, 2), (0, 6, 1)],
            intersections: vec![(5, vec![0, 1])],
            cull_rooms: 3,
        }
        .build()
    }

    #[test]
    fn parses_minimal_file() {
        let bai = Bai::parse(&two_road_fixture()).unwrap();
        assert_eq!(bai.roads.len(), 2);
        assert_eq!(bai.intersections.len(), 1);
        assert_eq!(bai.culling.large.len(), 3);
        let r0 = &bai.roads[0];
        assert_eq!(r0.flags, FLAG_FLAT);
        assert_eq!(r0.sections.len(), 4);
        assert_eq!(r0.sections[2].distance, 20.0);
        assert_eq!(r0.rooms, vec![7]);
        assert_eq!(r0.right.lane_count, 2);
        assert_eq!(r0.right.lane_distances.len(), 3); // 2 lanes + 1 sidewalk
        assert_eq!(r0.right.lane_distances[0][3], 30.0);
        assert_eq!(r0.right.edge_distances, vec![2.5, 5.0, 7.5]);
        assert_eq!(r0.right.sidewalk_inner.len(), 4);
        assert_eq!(
            r0.right.ambient_type(),
            Some(AmbientType::VehiclesAndPedestrians)
        );
        assert!(r0.end.is_connected());
        assert_eq!(r0.end.vehicle_rule(), Some(VehicleRule::StopSign));
        assert_eq!(bai.intersections[0].roads, vec![0, 1]);
        assert!(bai.validate().is_empty());
    }

    #[test]
    fn rejects_bad_magic() {
        let mut d = two_road_fixture();
        d[0] = b'X';
        assert!(matches!(Bai::parse(&d), Err(FormatError::BadMagic { .. })));
    }

    #[test]
    fn rejects_truncation() {
        let d = two_road_fixture();
        let cut = d.len() - 10;
        assert!(matches!(
            Bai::parse(&d[..cut]),
            Err(FormatError::UnexpectedEof { .. })
        ));
    }

    #[test]
    fn rejects_trailing_bytes() {
        let mut d = two_road_fixture();
        d.push(0);
        assert!(matches!(Bai::parse(&d), Err(FormatError::Parse { .. })));
    }

    #[test]
    fn rejects_implausible_counts() {
        // nSections = 0xFFFF must fail the sanity cap rather than allocate.
        let mut d = Vec::new();
        d.extend_from_slice(BAI_MAGIC);
        d.extend_from_slice(&0u16.to_le_bytes());
        d.extend_from_slice(&1u16.to_le_bytes());
        d.extend_from_slice(&0u16.to_le_bytes()); // road id
        d.extend_from_slice(&0xFFFFu16.to_le_bytes()); // nSections
        assert!(matches!(
            Bai::parse(&d),
            Err(FormatError::InvalidValue { .. })
        ));
    }

    #[test]
    fn validate_catches_dangling_references() {
        // Intersection references road 5 which does not exist.
        let bai = Bai::parse(
            &Builder {
                roads: vec![(0, 4, 1)],
                intersections: vec![(3, vec![0, 5])],
                cull_rooms: 2,
            }
            .build(),
        )
        .unwrap();
        let issues = bai.validate();
        assert!(
            issues
                .iter()
                .any(|i| matches!(i, BaiIssue::DanglingIntersectionRoad { road: 5, .. })),
            "{issues:?}"
        );
        // Road 0's connected end is fine, but the auto-wired second slot is
        // absent — only the dangling ref is reported.
        assert_eq!(issues.len(), 1, "{issues:?}");

        // Culling references road 9 which does not exist.
        let mut d = Vec::new();
        d.extend_from_slice(BAI_MAGIC);
        d.extend_from_slice(&0u16.to_le_bytes());
        d.extend_from_slice(&0u16.to_le_bytes());
        d.extend_from_slice(&2u32.to_le_bytes());
        d.extend_from_slice(&1u16.to_le_bytes());
        d.extend_from_slice(&9u16.to_le_bytes()); // large[0] -> road 9
        d.extend_from_slice(&0u16.to_le_bytes());
        d.extend_from_slice(&0u16.to_le_bytes());
        d.extend_from_slice(&0u16.to_le_bytes());
        let bai = Bai::parse(&d).unwrap();
        let issues = bai.validate();
        assert!(
            issues.iter().any(|i| matches!(
                i,
                BaiIssue::DanglingCullingRoad {
                    room: 0,
                    large: true,
                    road: 9
                }
            )),
            "{issues:?}"
        );
    }

    #[test]
    fn validate_catches_back_reference_mismatch() {
        // Road 0's end claims slot 0 of intersection 0 but the
        // intersection lists road 1 there.
        let mut d = Vec::new();
        d.extend_from_slice(BAI_MAGIC);
        d.extend_from_slice(&1u16.to_le_bytes()); // 1 intersection
        d.extend_from_slice(&1u16.to_le_bytes()); // 1 road
        let b = Builder {
            roads: vec![(0, 4, 0)],
            intersections: vec![],
            cull_rooms: 0,
        };
        // road with a hand-written end: intersection 0, index 0 — but the
        // intersection below references road 7.
        b.write_road(&mut d, 0, 0, 4, 0);
        // fixup: write_road wrote END_FILL ends; patch the first end's
        // fields to (intersection 0, index 0).
        let end_off = d.len() - 76;
        d[end_off..end_off + 4].copy_from_slice(&0u32.to_le_bytes());
        d[end_off + 10..end_off + 14].copy_from_slice(&0u32.to_le_bytes());
        d.extend_from_slice(&0u16.to_le_bytes()); // int id
        d.extend_from_slice(&4u16.to_le_bytes()); // room
        push_v3(&mut d, [0.0; 3]);
        d.extend_from_slice(&1u16.to_le_bytes()); // 1 road ref
        d.extend_from_slice(&7u32.to_le_bytes()); // -> road 7 (dangling)
        d.extend_from_slice(&0u32.to_le_bytes()); // 0 cull rooms
        let bai = Bai::parse(&d).unwrap();
        let issues = bai.validate();
        assert!(
            issues.iter().any(|i| matches!(
                i,
                BaiIssue::EndIndexMismatch {
                    index: 0,
                    found: 7,
                    ..
                }
            )),
            "{issues:?}"
        );
        assert!(
            issues
                .iter()
                .any(|i| matches!(i, BaiIssue::DanglingIntersectionRoad { road: 7, .. })),
            "{issues:?}"
        );
    }
}
