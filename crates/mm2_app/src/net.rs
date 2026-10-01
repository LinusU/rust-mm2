//! The `mm2_net` bridge (F24-B): `SessionConfig` ↔ the lobby's session
//! advertisement, and the future home of the Bevy systems that drive a
//! lobby `Host`/`Client` from app state.
//!
//! `mm2_net` stays project-free — the wire's [`SessionAdvertisement`]
//! carries a bounded opaque `params` blob whose layout this module owns.
//! The blob is JSON so a captured advertisement is readable; its version
//! is the protocol's — peers that handshake built the same
//! `PROTOCOL_VERSION` code, so a layout change ships with a version
//! bump, never silently.
//!
//! Deliberately absent from the advertisement — the joining side fills
//! these in itself:
//!
//! - `authority` — a peer that accepted an advertisement is always
//!   `SessionAuthority::Remote`; the host alone is authoritative;
//! - `vehicle` — the driver's car/paint pick is per-player roster
//!   state, not session config; it travels separately via
//!   [`Message::SetVehicle`](mm2_net::Message::SetVehicle) and this
//!   module's [`encode_pick`]/[`decode_pick`]/[`vehicle_validator`]
//!   helpers;
//! - `mods_active` — whether *this* process mounted mods is a local
//!   fact the session builder stamps on its own;
//! - `dev` — developer overrides are never network-legal, so
//!   [`advertise`] refuses a config carrying any rather than dropping
//!   them silently.

use std::collections::BTreeMap;
use std::sync::Arc;

use mm2_content::{EntryStatus, VehicleCatalog};
use mm2_game::{
    ConfigError, Densities, DevOverrides, Difficulty, EventRef, EventTableKind, RaceCustomization,
    SelectorError, SessionAuthority, SessionConditions, SessionConfig, SessionCustomization,
    SessionMode, TimeOfDay, VehicleSelection, Weather, WorldMode,
};
use mm2_net::{PickValidator, SessionAdvertisement, VehiclePick};
use serde::{Deserialize, Serialize};

/// A `SessionConfig` the advertisement could not carry, or a `params`
/// blob that could not be read back into one.
#[derive(Debug, thiserror::Error)]
pub enum SessionWireError {
    /// The advertised config carries `DevOverrides` — never
    /// network-legal, so they cannot ride a lobby advertisement.
    #[error("developer overrides cannot be advertised")]
    DevOverrides,
    /// The config fails `SessionConfig::validate` — on encode (the
    /// host's own config) or on decode (the received blob).
    #[error("invalid session config: {0}")]
    Invalid(#[from] ConfigError),
    /// A condition selector outside the authored 0-3 range.
    #[error("{0}")]
    Selector(#[from] SelectorError),
    /// The params blob is not the encoding this build produces.
    #[error("session params: {0}")]
    Params(#[from] serde_json::Error),
    /// A paint index the wire's `u8` field cannot carry.
    #[error("paint index {0} exceeds the wire's u8 bound")]
    Paint(usize),
}

/// Encode a session's configuration for the lobby to carry. The result
/// is opaque to `mm2_net` but complete: [`accept`] rebuilds the same
/// world/mode/difficulty/conditions/densities/customization/seed.
pub fn advertise(config: &SessionConfig) -> Result<SessionAdvertisement, SessionWireError> {
    if config.dev != DevOverrides::default() {
        return Err(SessionWireError::DevOverrides);
    }
    config.validate()?;
    Ok(SessionAdvertisement {
        summary: summarize(config),
        params: serde_json::to_vec(&SessionParams::from(config))?,
    })
}

/// Decode a received advertisement back into a `SessionConfig`, stamped
/// `authority: Remote` with `vehicle`/`mods_active`/`dev` at defaults —
/// the local session builder fills those in from its own state.
pub fn accept(ad: &SessionAdvertisement) -> Result<SessionConfig, SessionWireError> {
    let params: SessionParams = serde_json::from_slice(&ad.params)?;
    let config = params.into_config()?;
    config.validate()?;
    Ok(config)
}

/// `VehicleSelection` → the wire pick the lobby carries: `id: None`
/// (the synthetic dev car) travels as the empty string — a real id is
/// never empty (`ConfigError::EmptyVehicleId` guards that), so `""` is
/// unambiguous — and `paint` must fit the wire's `u8` field.
pub fn encode_pick(selection: &VehicleSelection) -> Result<VehiclePick, SessionWireError> {
    Ok(VehiclePick {
        vehicle: selection.id.clone().unwrap_or_default(),
        paint: u8::try_from(selection.paint)
            .map_err(|_| SessionWireError::Paint(selection.paint))?,
    })
}

/// The reverse of [`encode_pick`]: a roster pick back into the app's
/// `VehicleSelection` — the empty wire id is the dev car.
pub fn decode_pick(pick: &VehiclePick) -> VehicleSelection {
    VehicleSelection {
        id: if pick.vehicle.is_empty() {
            None
        } else {
            Some(pick.vehicle.clone())
        },
        paint: pick.paint as usize,
    }
}

/// The lobby's pick validator, built from the mounted content catalog —
/// the authoritative side of `SetVehicle` (F24-B.3). A host installs it
/// as `HostConfig::pick_validator` so a peer cannot roster a car it
/// could never spawn. Designed policy:
///
/// - `""` (the synthetic dev car) is always a legal pick — it is the
///   engine's no-content fallback — but it has exactly one paint job,
///   so `paint` must be 0;
/// - any other pick must name a catalog entry *exactly* — ids are the
///   canonical lowercase basenames; display-name aliases are a menu
///   convenience, not a wire identity — and must be `EntryStatus::Ready`
///   (an entry with missing deps cannot spawn for anyone);
/// - `paint` is bounded by the entry's metadata `Colors` list
///   (`paints.len()`, minimum one job) — the same bound the garage menu
///   presents. The model's `paint_jobs` check in
///   `mm2_content::load_vehicle` stays authoritative at spawn time; the
///   fingerprint gate guarantees every peer's catalog is identical, so
///   a pick legal here is legal everywhere.
pub fn vehicle_validator(catalog: &VehicleCatalog) -> PickValidator {
    // id → paint bound, or the refusal reason for a known-but-unloadable
    // entry — kept whole so a refusal can say *why* a listed car fails.
    let mut legal: BTreeMap<String, Result<usize, String>> = BTreeMap::new();
    for e in &catalog.entries {
        let bound = match &e.status {
            EntryStatus::Ready => Ok(e.paints.len().max(1)),
            EntryStatus::Incomplete { missing } => Err(format!(
                "vehicle {} is incomplete: missing {}",
                e.id,
                missing.join(", ")
            )),
        };
        legal.insert(e.id.clone(), bound);
    }
    Arc::new(move |vehicle, paint| {
        if vehicle.is_empty() {
            return if paint == 0 {
                Ok(())
            } else {
                Err("the dev car has a single paint job".to_string())
            };
        }
        match legal.get(vehicle) {
            None => Err(format!("unknown vehicle id {vehicle:?}")),
            Some(Err(reason)) => Err(reason.clone()),
            Some(Ok(bound)) if paint as usize >= *bound => Err(format!(
                "paint {paint} out of range: {vehicle} has {bound} paint job(s)"
            )),
            Some(Ok(_)) => Ok(()),
        }
    })
}

/// The display line for lobby UIs/CLIs (`"sf, circuit:3, professional"`).
fn summarize(config: &SessionConfig) -> String {
    let world = match &config.world {
        WorldMode::DevWorld => "dev world".to_string(),
        WorldMode::City { psdl } => psdl
            .strip_prefix("city/")
            .and_then(|s| s.strip_suffix(".psdl"))
            .unwrap_or(psdl)
            .to_string(),
    };
    let mode = match &config.mode {
        SessionMode::Cruise => "cruise".to_string(),
        SessionMode::Event(r) => format!("{}:{}", r.table.stem_prefix(), r.index),
    };
    format!("{}, {}, {}", world, mode, config.difficulty.as_str())
}

/// The `params` payload layout: a session's full configuration minus
/// per-player and local-only fields.
#[derive(Debug, Serialize, Deserialize)]
struct SessionParams {
    world: WorldParams,
    mode: ModeParams,
    difficulty: Difficulty,
    conditions: ConditionsParams,
    densities: DensitiesParams,
    customization: Option<CustomizationParams>,
    seed: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WorldParams {
    DevWorld,
    City { psdl: String },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ModeParams {
    Cruise,
    Event {
        city: String,
        table: EventTableKind,
        index: usize,
    },
}

/// Selectors travel as raw `u8`s; [`accept`] re-bounds them through
/// `TimeOfDay::new`/`Weather::new` so a hostile or stale blob cannot
/// inject an out-of-range index.
#[derive(Debug, Serialize, Deserialize)]
struct ConditionsParams {
    time_of_day: u8,
    weather: u8,
}

#[derive(Debug, Serialize, Deserialize)]
struct DensitiesParams {
    traffic: f32,
    pedestrians: f32,
}

#[derive(Debug, Serialize, Deserialize)]
struct CustomizationParams {
    conditions: ConditionsParams,
    densities: DensitiesParams,
    race: Option<RaceParams>,
}

#[derive(Debug, Serialize, Deserialize)]
struct RaceParams {
    laps: u32,
    opponents: u32,
}

impl From<&SessionConfig> for SessionParams {
    fn from(config: &SessionConfig) -> Self {
        Self {
            world: match &config.world {
                WorldMode::DevWorld => WorldParams::DevWorld,
                WorldMode::City { psdl } => WorldParams::City { psdl: psdl.clone() },
            },
            mode: match &config.mode {
                SessionMode::Cruise => ModeParams::Cruise,
                SessionMode::Event(r) => ModeParams::Event {
                    city: r.city.clone(),
                    table: r.table,
                    index: r.index,
                },
            },
            difficulty: config.difficulty,
            conditions: ConditionsParams::from(config.conditions),
            densities: DensitiesParams::from(config.densities),
            customization: config.customization.as_ref().map(|c| CustomizationParams {
                conditions: ConditionsParams::from(c.conditions),
                densities: DensitiesParams::from(c.densities),
                race: c.race.map(|r| RaceParams {
                    laps: r.laps,
                    opponents: r.opponents,
                }),
            }),
            seed: config.seed,
        }
    }
}

impl From<SessionConditions> for ConditionsParams {
    fn from(c: SessionConditions) -> Self {
        Self {
            time_of_day: c.time_of_day.get(),
            weather: c.weather.get(),
        }
    }
}

impl From<Densities> for DensitiesParams {
    fn from(d: Densities) -> Self {
        Self {
            traffic: d.traffic,
            pedestrians: d.pedestrians,
        }
    }
}

impl SessionParams {
    fn into_config(self) -> Result<SessionConfig, SessionWireError> {
        Ok(SessionConfig {
            world: match self.world {
                WorldParams::DevWorld => WorldMode::DevWorld,
                WorldParams::City { psdl } => WorldMode::City { psdl },
            },
            mode: match self.mode {
                ModeParams::Cruise => SessionMode::Cruise,
                ModeParams::Event { city, table, index } => {
                    SessionMode::Event(EventRef { city, table, index })
                }
            },
            difficulty: self.difficulty,
            conditions: self.conditions.into_conditions()?,
            densities: self.densities.into_densities(),
            customization: self
                .customization
                .map(|c| {
                    Ok::<_, SessionWireError>(SessionCustomization {
                        conditions: c.conditions.into_conditions()?,
                        densities: c.densities.into_densities(),
                        race: c.race.map(|r| RaceCustomization {
                            laps: r.laps,
                            opponents: r.opponents,
                        }),
                    })
                })
                .transpose()?,
            seed: self.seed,
            // Per-player and local-only fields never ride the wire.
            vehicle: VehicleSelection::default(),
            authority: SessionAuthority::Remote,
            mods_active: false,
            dev: DevOverrides::default(),
        })
    }
}

impl ConditionsParams {
    fn into_conditions(self) -> Result<SessionConditions, SelectorError> {
        Ok(SessionConditions {
            time_of_day: TimeOfDay::new(self.time_of_day)?,
            weather: Weather::new(self.weather)?,
        })
    }
}

impl DensitiesParams {
    fn into_densities(self) -> Densities {
        Densities {
            traffic: self.traffic,
            pedestrians: self.pedestrians,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mm2_game::SpawnPose;
    use serde_json::json;

    fn city_event_config() -> SessionConfig {
        SessionConfig {
            world: WorldMode::City {
                psdl: "city/sf.psdl".to_string(),
            },
            mode: SessionMode::Event(EventRef {
                city: "sf".to_string(),
                table: EventTableKind::Circuit,
                index: 3,
            }),
            difficulty: Difficulty::Professional,
            conditions: SessionConditions {
                time_of_day: TimeOfDay::new(2).unwrap(),
                weather: Weather::new(3).unwrap(),
            },
            densities: Densities {
                traffic: 0.25,
                pedestrians: 0.75,
            },
            customization: Some(SessionCustomization {
                conditions: SessionConditions {
                    time_of_day: TimeOfDay::new(1).unwrap(),
                    weather: Weather::new(0).unwrap(),
                },
                densities: Densities {
                    traffic: 0.0,
                    pedestrians: 1.0,
                },
                race: Some(RaceCustomization {
                    laps: 4,
                    opponents: 5,
                }),
            }),
            seed: 0xfeed_beef,
            authority: SessionAuthority::Host,
            ..SessionConfig::default()
        }
    }

    #[test]
    fn a_dev_world_cruise_roundtrips() {
        let config = SessionConfig {
            seed: 42,
            ..SessionConfig::default()
        };
        let ad = advertise(&config).unwrap();
        assert_eq!(ad.summary, "dev world, cruise, amateur");
        assert!(!ad.params.is_empty());

        let back = accept(&ad).unwrap();
        assert_eq!(back.world, WorldMode::DevWorld);
        assert_eq!(back.mode, SessionMode::Cruise);
        assert_eq!(back.seed, 42);
        // Stamped by `accept`, not carried by the wire.
        assert_eq!(back.authority, SessionAuthority::Remote);
        assert_eq!(back.vehicle, VehicleSelection::default());
        assert!(!back.mods_active);
        assert_eq!(back.dev, DevOverrides::default());
    }

    #[test]
    fn a_full_config_roundtrips() {
        let config = city_event_config();
        let ad = advertise(&config).unwrap();
        assert_eq!(ad.summary, "sf, circuit:3, professional");
        let back = accept(&ad).unwrap();
        assert_eq!(back.world, config.world);
        assert_eq!(back.mode, config.mode);
        assert_eq!(back.difficulty, config.difficulty);
        assert_eq!(back.conditions, config.conditions);
        assert_eq!(back.densities, config.densities);
        assert_eq!(back.customization, config.customization);
        assert_eq!(back.seed, config.seed);
        assert_eq!(back.authority, SessionAuthority::Remote);
    }

    /// `DevOverrides` are never network-legal: a config carrying any
    /// refuses to advertise rather than silently dropping them.
    #[test]
    fn developer_overrides_are_never_advertised() {
        let mut config = SessionConfig::default();
        config.dev.traction = Some(0.9);
        assert!(matches!(
            advertise(&config),
            Err(SessionWireError::DevOverrides)
        ));
        let mut config = SessionConfig::default();
        config.dev.spawn = Some(SpawnPose {
            position: bevy::prelude::Vec3::ZERO,
            yaw: 0.0,
        });
        assert!(matches!(
            advertise(&config),
            Err(SessionWireError::DevOverrides)
        ));
    }

    #[test]
    fn an_invalid_host_config_is_refused() {
        let mut config = SessionConfig::default();
        config.densities.traffic = 2.0;
        assert!(matches!(
            advertise(&config),
            Err(SessionWireError::Invalid(ConfigError::Density { .. }))
        ));
    }

    #[test]
    fn garbage_params_do_not_decode() {
        for params in [
            b"not json".to_vec(),
            b"{}".to_vec(),
            b"{\"world\":1}".to_vec(),
            vec![0xff, 0x00],
        ] {
            let ad = SessionAdvertisement {
                summary: "x".to_string(),
                params,
            };
            assert!(
                matches!(accept(&ad), Err(SessionWireError::Params(_))),
                "params {ad:?} must not decode"
            );
        }
    }

    /// A blob is untrusted input: out-of-range selectors and density
    /// fractions are rejected on decode, not clamped into validity.
    #[test]
    fn out_of_range_wire_values_are_rejected() {
        let base = serde_json::to_value(SessionParams::from(&SessionConfig::default())).unwrap();
        for (pointer, value) in [
            ("/conditions/weather", json!(9)),
            ("/conditions/time_of_day", json!(4)),
            ("/densities/traffic", json!(1.5)),
            ("/densities/pedestrians", json!(-0.1)),
        ] {
            let mut params = base.clone();
            *params.pointer_mut(pointer).unwrap() = value;
            let ad = SessionAdvertisement {
                summary: "x".to_string(),
                params: serde_json::to_vec(&params).unwrap(),
            };
            assert!(
                accept(&ad).is_err(),
                "params with {pointer}={:?} must be rejected",
                params.pointer(pointer)
            );
        }
    }

    /// A `laps: 0` customization on the wire surfaces as the shared
    /// `ZeroLaps` config error, not a bespoke net-layer complaint.
    #[test]
    fn a_zero_laps_pick_fails_validation() {
        let mut params =
            serde_json::to_value(SessionParams::from(&SessionConfig::default())).unwrap();
        params["customization"] = json!({
            "conditions": {"time_of_day": 0, "weather": 0},
            "densities": {"traffic": 0.5, "pedestrians": 0.5},
            "race": {"laps": 0, "opponents": 3},
        });
        let ad = SessionAdvertisement {
            summary: "x".to_string(),
            params: serde_json::to_vec(&params).unwrap(),
        };
        assert!(matches!(
            accept(&ad),
            Err(SessionWireError::Invalid(ConfigError::ZeroLaps))
        ));
    }

    fn catalog_entry(id: &str, paints: &[&str], ready: bool) -> mm2_content::CatalogEntry {
        mm2_content::CatalogEntry {
            id: id.to_string(),
            display_name: id.to_string(),
            paints: paints.iter().map(|p| p.to_string()).collect(),
            canonical_info: true,
            unlock_score: 0,
            unlock_flags: 0,
            class: mm2_content::VehicleClass::Stock,
            deps: mm2_content::DepSet::default(),
            status: if ready {
                EntryStatus::Ready
            } else {
                EntryStatus::Incomplete {
                    missing: vec!["model (geometry/<id>.pkg)".to_string()],
                }
            },
            notes: Vec::new(),
        }
    }

    fn test_catalog() -> VehicleCatalog {
        VehicleCatalog {
            entries: vec![
                catalog_entry("vpbug", &["red", "blue", "green", "yellow"], true),
                catalog_entry("vpcab", &["taxi"], false),
                // A ready entry with no Colors metadata gets the
                // one-paint-job floor like `paint_jobs.max(1)`.
                catalog_entry("vpbare", &[], true),
            ],
        }
    }

    /// The dev car (empty wire id ↔ `VehicleSelection::id = None`) and
    /// a catalog pick both survive the pick codec; a paint index that
    /// does not fit the wire's `u8` is refused, not clamped.
    #[test]
    fn picks_roundtrip_between_selection_and_wire() {
        let sel = VehicleSelection {
            id: Some("vpbug".to_string()),
            paint: 2,
        };
        let pick = encode_pick(&sel).unwrap();
        assert_eq!(pick.vehicle, "vpbug");
        assert_eq!(pick.paint, 2);
        assert_eq!(decode_pick(&pick), sel);

        let dev = VehicleSelection::default();
        let pick = encode_pick(&dev).unwrap();
        assert_eq!(pick.vehicle, "");
        assert_eq!(decode_pick(&pick), dev);

        let wild = VehicleSelection {
            id: Some("vpbug".to_string()),
            paint: 300,
        };
        assert!(matches!(
            encode_pick(&wild),
            Err(SessionWireError::Paint(300))
        ));
    }

    /// The validator applies the designed policy: the dev car is always
    /// legal (paint 0 only), catalog ids must match exactly, incomplete
    /// entries refuse with their missing deps, and paint is bounded by
    /// the entry's `Colors` list.
    #[test]
    fn the_vehicle_validator_gates_picks_by_catalog() {
        let validate = vehicle_validator(&test_catalog());

        validate("", 0).unwrap();
        assert_eq!(
            validate("", 1).unwrap_err(),
            "the dev car has a single paint job"
        );

        validate("vpbug", 0).unwrap();
        validate("vpbug", 3).unwrap();
        assert_eq!(
            validate("vpbug", 4).unwrap_err(),
            "paint 4 out of range: vpbug has 4 paint job(s)"
        );

        // No Colors metadata: paint 0 alone is legal.
        validate("vpbare", 0).unwrap();
        assert!(validate("vpbare", 1).unwrap_err().contains("out of range"));

        // Unknown ids and non-canonical spellings refuse alike — the
        // wire carries catalog ids, not menu aliases.
        assert_eq!(
            validate("nosuch", 0).unwrap_err(),
            "unknown vehicle id \"nosuch\""
        );
        assert_eq!(
            validate("VPBUG", 0).unwrap_err(),
            "unknown vehicle id \"VPBUG\""
        );

        // A cataloged-but-incomplete entry refuses with the reason.
        let err = validate("vpcab", 0).unwrap_err();
        assert!(err.contains("incomplete"), "got {err}");
        assert!(err.contains("geometry"), "got {err}");
    }

    /// An empty catalog still admits the dev car — a content-free host
    /// (`mm2-host --dev-world` on an empty install) lobbies fine.
    #[test]
    fn the_validator_on_empty_content_admits_only_the_dev_car() {
        let validate = vehicle_validator(&VehicleCatalog::default());
        validate("", 0).unwrap();
        assert!(validate("vpbug", 0).is_err());
    }
}
