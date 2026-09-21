//! Menu front-end (F17-A.1): profile/mode/content selection screens
//! backed by the real domain catalogs, navigable by keyboard and
//! gamepad, driving `Session::begin` for launches.
//!
//! The model is deliberately thin and testable:
//!
//! - [`MenuShell`] is the navigation state — current screen, a back
//!   stack, focus, the row list [`rebuild`] produces, a status line and
//!   the pending launch selections (vehicle/paint/difficulty).
//! - [`MenuData`] holds the profile-store handle plus lazily-scanned
//!   catalogs (cities, events, availability gates, garage). Content
//!   reads happen once per menu lifetime; the profile list refreshes on
//!   every Profiles rebuild so store changes are visible immediately.
//! - [`MenuShell::apply`] turns a [`MenuCommand`] into state changes
//!   plus [`MenuEffect`]s — the only way the menu touches the world.
//!   `menu_input` executes the effects against ECS resources, so the
//!   model itself is drivable headlessly.
//! - `menu_present` (re)draws the `bevy_ui` text tree when `dirty` and
//!   owns the menu's `Camera2d` — `bevy_ui` renders per camera view and
//!   session cameras are `SessionEntity`-stamped, so without it the
//!   shell would have nothing to draw into. `menu_watch` reopens the
//!   shell whenever the session returns to `Menu`, which is also how a
//!   quit from a menu-launched session returns here instead of
//!   exiting.
//!
//! Deferred to later slices (honest gaps, not placeholders): Quick Race
//! (DRV-8's `last_event` launch), per-event weather/time/density
//! controls (needs F18's session-legal writers; RACE-3 `customizable`),
//! mouse navigation, results/pause screens (F17-B), original menu art
//! and audio.

use std::collections::BTreeMap;

use bevy::prelude::*;
use mm2_assets::Vfs;
use mm2_content::{EventCatalog, VehicleCatalog, VehicleDef};
use mm2_game::{
    AvailabilityTable, Difficulty, EventRef, EventTableKind, GarageTable, Mm2Vfs, PlayerProfile,
    ProfileId, ProfileStore, ProfileSummary, Session, SessionConfig, SessionMode, SessionPhase,
    VehicleSelection, WorldMode,
};
use tracing::{info, warn};

use crate::profile::{ActiveProfile, ProfileRequest};
use crate::session::{SelectedCar, TunedVehicle};

/// One user intent. Keyboard, gamepad and tests all produce these —
/// the model never reads devices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuCommand {
    /// Move focus up/down one row (clamped, no wrap).
    Up,
    /// Down.
    Down,
    /// Adjust a value row left (difficulty).
    Left,
    /// Right.
    Right,
    /// Activate the focused row.
    Activate,
    /// Leave the current screen; at the root this quits the app.
    Back,
    /// Destructive-action key on the Profiles screen (X / Delete).
    Delete,
}

/// The menu's navigation state. Each screen rebuilds its rows from
/// [`MenuData`] on entry, so stale lists never linger.
#[derive(Debug, Clone, PartialEq)]
pub enum Screen {
    /// Top level.
    Root,
    /// Pick a city to cruise.
    CruiseCity,
    /// Pick a city whose event tables to browse.
    EventCity,
    /// Pick one of a city's four authored event tables.
    EventTable {
        /// City stem.
        city: String,
    },
    /// Pick a row of one table.
    EventList {
        /// City stem.
        city: String,
        /// Which `mm*data.csv` table.
        table: EventTableKind,
    },
    /// Pick a roster vehicle.
    Garage,
    /// Pick one of the selected vehicle's paints.
    Paints {
        /// Catalog id being painted.
        car: String,
    },
    /// Select/create/delete driver profiles.
    Profiles,
    /// F16-AC06's deliberate-confirmation step — deleting a profile is
    /// a separate screen requiring a second activation.
    ConfirmDelete {
        /// Profile being deleted.
        id: ProfileId,
        /// Display label from the profile row.
        label: String,
    },
}

/// What a row activation does. Actions are resolved at rebuild time —
/// `apply` never interprets screen data.
#[derive(Debug, Clone)]
pub enum Action {
    /// Descend into a sub-screen.
    Push(Screen),
    /// Ascend one level.
    Back,
    /// Cycle Amateur ↔ Professional (Root's difficulty row).
    ToggleDifficulty,
    /// Free-roam a city.
    LaunchCruise {
        /// City stem (`city/<city>.psdl` must resolve to be enabled).
        city: String,
    },
    /// Run an authored event.
    LaunchEvent(EventRef),
    /// Select a roster vehicle and open its paint list.
    PickVehicle {
        /// Catalog id.
        id: String,
    },
    /// Select the pending vehicle's paint.
    PickPaint {
        /// Catalog id the paint belongs to.
        car: String,
        /// Zero-based paint index.
        index: usize,
    },
    /// Bind an existing profile (marks it `active`).
    BindProfile(ProfileId),
    /// Create and bind a new auto-named standard profile.
    CreateProfile,
    /// Drop the bound profile and drive profile-less.
    DriveProfileless,
    /// Open the delete confirmation for a profile.
    AskDelete {
        /// Profile to delete.
        id: ProfileId,
        /// Display label for the confirm screen.
        label: String,
    },
    /// Actually delete (only offered behind [`Screen::ConfirmDelete`]).
    ConfirmDelete(ProfileId),
    /// Exit the application.
    Quit,
}

/// One menu row: its label, whether it may activate and what it does.
/// Disabled rows stay visible and focusable — their reason is shown
/// instead of hiding the capability (spec req 3, AC05's
/// no-dead-end rule: the row names why, it never navigates to a fake
/// screen).
#[derive(Debug, Clone)]
pub struct Row {
    /// Display text (without the focus marker).
    pub text: String,
    /// `Err(reason)` keeps the row visible but disabled.
    pub enabled: Result<(), String>,
    /// What `Activate` runs when enabled.
    pub action: Action,
}

/// What [`MenuShell::apply`] asks the app shell to do. The model emits,
/// `menu_input` executes — keeping the store/VFS operations in the
/// model and the ECS touches in one system.
pub enum MenuEffect {
    /// Begin a session; the vehicle is already resolved so a load
    /// failure surfaces as a status line and never becomes an effect.
    Launch {
        /// The validated session config — boxed to keep the effect
        /// enum small (most effects are a row action, not a launch).
        config: Box<SessionConfig>,
        /// Resolved vehicle (None = synthetic dev car).
        car: Option<Box<VehicleDef>>,
        /// Paint index driven.
        paint: usize,
    },
    /// Insert `ActiveProfile` for this slot.
    Bind(Box<ActiveProfile>),
    /// Remove the `ActiveProfile` resource.
    Unbind,
    /// Exit the process.
    Exit,
}

/// The menu's navigation/selection state — a resource so systems and
/// tests share the one instance the app drives.
#[derive(Resource)]
pub struct MenuShell {
    /// Whether the menu owns input and its UI is up. False while a
    /// launched session loads/plays; `menu_watch` reopens it when the
    /// session returns to `Menu`.
    pub active: bool,
    /// Current screen.
    pub screen: Screen,
    /// `(screen, focus)` stack Back pops.
    stack: Vec<(Screen, usize)>,
    /// Focused row index.
    pub focus: usize,
    /// Rows of the current screen — rebuilt on every change.
    pub rows: Vec<Row>,
    /// One-line status/error shown under the rows.
    pub status: Option<String>,
    /// The vehicle a launch drives — seeded from the launch resolution
    /// (`--car`/remembered/default) and changed on Garage/Paints.
    pub vehicle: VehicleSelection,
    /// Session difficulty — seeded from `--pro`/the bound profile's
    /// rank (DRV-2) and cycled on the Root row.
    pub difficulty: Difficulty,
    dirty: bool,
    /// Last gamepad nav-axis reading — edge detection for stick moves.
    pad_axis: f32,
}

/// Cached domain data the screens read. The VFS never changes at
/// runtime, so content scans run once; `profiles` refreshes per
/// Profiles rebuild because menu actions mutate the store.
#[derive(Resource)]
pub struct MenuData {
    /// Profile store the Driver screen works against — `None` on
    /// `--no-profile` or an unopenable root.
    store: Option<ProfileStore>,
    /// `--mods` was mounted — carried into launched `SessionConfig`s so
    /// modded sessions stay record-ineligible.
    has_mods: bool,
    /// The menu's view of the bound driver — synced from the
    /// `ActiveProfile` resource by `menu_watch` and updated by bind/
    /// unbind effects inside `apply`, so gating and labels reflect the
    /// current pick without waiting on deferred resource writes.
    pub bound: Option<PlayerProfile>,
    scanned: bool,
    /// `city/*.psdl` stems — the cruisable cities.
    cities: Vec<String>,
    /// Cities carrying authored race data (`race_cities` — includes
    /// expected-but-missing stock cities so they report honestly).
    race_cities: Vec<String>,
    catalog: Option<VehicleCatalog>,
    garage: Option<GarageTable>,
    events: BTreeMap<String, EventCatalog>,
    availability: BTreeMap<String, AvailabilityTable>,
    profiles: Vec<ProfileSummary>,
}

impl MenuData {
    /// A fresh cache; content scans run lazily on first use.
    pub fn new(store: Option<ProfileStore>, has_mods: bool, bound: Option<PlayerProfile>) -> Self {
        Self {
            store,
            has_mods,
            bound,
            scanned: false,
            cities: Vec::new(),
            race_cities: Vec::new(),
            catalog: None,
            garage: None,
            events: BTreeMap::new(),
            availability: BTreeMap::new(),
            profiles: Vec::new(),
        }
    }

    /// Scan content once — the mounted set cannot change mid-run.
    fn ensure_scan(&mut self, vfs: &Vfs) {
        if self.scanned {
            return;
        }
        self.scanned = true;
        let mut cities: Vec<String> = vfs
            .list()
            .iter()
            .filter_map(|p| {
                p.strip_prefix("city/")
                    .and_then(|rest| rest.strip_suffix(".psdl"))
                    .filter(|stem| !stem.contains('/'))
                    .map(|stem| stem.to_string())
            })
            .collect();
        cities.sort();
        cities.dedup();
        self.cities = cities;
        self.race_cities = mm2_content::race_cities(vfs);
        self.catalog = Some(VehicleCatalog::scan(vfs));
        self.garage = Some(mm2_content::scan_garage(vfs));
        self.refresh_profiles();
    }

    /// A city's event catalog, scanned on first request.
    fn catalog_of(&mut self, vfs: &Vfs, city: &str) -> &EventCatalog {
        self.events
            .entry(city.to_string())
            .or_insert_with(|| EventCatalog::scan(vfs, city))
    }

    /// A city's availability surface (shares the catalog scan).
    fn availability_of(&mut self, vfs: &Vfs, city: &str) -> &AvailabilityTable {
        if !self.availability.contains_key(city) {
            let table = mm2_content::availability_table(self.catalog_of(vfs, city));
            self.availability.insert(city.to_string(), table);
        }
        &self.availability[city]
    }

    /// Re-read the profile list from the store.
    fn refresh_profiles(&mut self) {
        self.profiles = self
            .store
            .as_ref()
            .and_then(|s| s.list().ok())
            .unwrap_or_default();
    }

    /// Whether `city`'s world can load at all — every session mode
    /// needs `city/<stem>.psdl`.
    fn city_loadable(&self, vfs: &Vfs, city: &str) -> Result<(), String> {
        if vfs.resolve(&format!("city/{city}.psdl")).is_some() {
            Ok(())
        } else {
            Err(format!("city/{city}.psdl not found in the mounted content"))
        }
    }
}

impl MenuShell {
    /// A shell parked on the root screen, active and needing its first
    /// rebuild. `vehicle`/`difficulty` come from the launch resolution
    /// (`--car`, the bound profile's remembered selections, or the
    /// stock defaults).
    pub fn new(vehicle: VehicleSelection, difficulty: Difficulty) -> Self {
        Self {
            active: true,
            screen: Screen::Root,
            stack: Vec::new(),
            focus: 0,
            rows: Vec::new(),
            status: None,
            vehicle,
            difficulty,
            dirty: true,
            pad_axis: 0.0,
        }
    }

    /// Reopen at the root — the post-session entry point.
    fn reopen(&mut self) {
        self.active = true;
        self.screen = Screen::Root;
        self.stack.clear();
        self.focus = 0;
        self.status = None;
        self.dirty = true;
    }

    fn push(&mut self, screen: Screen) {
        self.stack.push((self.screen.clone(), self.focus));
        self.screen = screen;
        self.focus = 0;
        self.status = None;
    }

    fn pop(&mut self) -> bool {
        if let Some((screen, focus)) = self.stack.pop() {
            self.screen = screen;
            self.focus = focus;
            self.status = None;
            true
        } else {
            false
        }
    }

    /// Apply one command; returns the effects the app executes.
    ///
    /// Focus clamps (no wrap), disabled rows stay focusable so their
    /// reason is visible, and `Activate` on one only sets the status —
    /// an unavailable capability never silently does something else.
    pub fn apply(&mut self, cmd: MenuCommand, data: &mut MenuData, vfs: &Vfs) -> Vec<MenuEffect> {
        if !self.active {
            return Vec::new();
        }
        let mut effects = Vec::new();
        match cmd {
            MenuCommand::Up => self.focus = self.focus.saturating_sub(1),
            MenuCommand::Down => {
                self.focus = (self.focus + 1).min(self.rows.len().saturating_sub(1))
            }
            MenuCommand::Left | MenuCommand::Right => {
                if matches!(
                    self.rows.get(self.focus).map(|r| &r.action),
                    Some(Action::ToggleDifficulty)
                ) {
                    self.difficulty = match self.difficulty {
                        Difficulty::Amateur => Difficulty::Professional,
                        Difficulty::Professional => Difficulty::Amateur,
                    };
                }
            }
            MenuCommand::Back => {
                if !self.pop() {
                    // Back at the root is the menu's exit path — Esc
                    // never strands the user on the top screen.
                    effects.push(MenuEffect::Exit);
                }
            }
            MenuCommand::Delete => {
                // Only profile rows are deletable; anything else is a
                // no-op so the key is safe everywhere else.
                if self.screen == Screen::Profiles
                    && let Some(row) = self.rows.get(self.focus)
                    && let Action::BindProfile(id) = &row.action
                {
                    let id = id.clone();
                    let label = row.text.clone();
                    self.push(Screen::ConfirmDelete { id, label });
                }
            }
            MenuCommand::Activate => {
                let picked = self.rows.get(self.focus).map(|row| match &row.enabled {
                    Err(reason) => Err(reason.clone()),
                    Ok(()) => Ok(row.action.clone()),
                });
                match picked {
                    Some(Err(reason)) => self.status = Some(reason),
                    Some(Ok(action)) => self.activate(action, data, vfs, &mut effects),
                    None => {}
                }
            }
        }
        self.dirty = true;
        effects
    }

    /// Execute one row action — the match that owns screen transitions
    /// and effect production.
    fn activate(
        &mut self,
        action: Action,
        data: &mut MenuData,
        vfs: &Vfs,
        effects: &mut Vec<MenuEffect>,
    ) {
        match action {
            Action::Push(screen) => self.push(screen),
            Action::Back => {
                self.pop();
            }
            Action::ToggleDifficulty => {
                self.difficulty = match self.difficulty {
                    Difficulty::Amateur => Difficulty::Professional,
                    Difficulty::Professional => Difficulty::Amateur,
                };
            }
            Action::LaunchCruise { city } => {
                self.launch(data, vfs, SessionMode::Cruise, city, effects)
            }
            Action::LaunchEvent(event_ref) => {
                let city = event_ref.city.clone();
                self.launch(data, vfs, SessionMode::Event(event_ref), city, effects)
            }
            Action::PickVehicle { id } => {
                self.vehicle.id = Some(id.clone());
                self.vehicle.paint = 0;
                self.push(Screen::Paints { car: id });
            }
            Action::PickPaint { index, .. } => {
                self.vehicle.paint = index;
                self.pop();
                self.status = Some(format!("paint {index} selected"));
            }
            Action::BindProfile(id) => {
                let Some(store) = &data.store else {
                    self.status = Some("profile store unavailable".into());
                    return;
                };
                match crate::profile::resolve(store, &ProfileRequest::Select(id.as_str().into())) {
                    Ok(Some(slot)) => {
                        info!(profile = %slot.profile.id, name = %slot.profile.name, "driver selected");
                        // The bound profile's rank is the difficulty
                        // baseline (DRV-2) — the Root row still
                        // overrides it per session.
                        self.difficulty = slot.profile.rank;
                        self.status = Some(format!("driving as {}", slot.profile.name));
                        data.bound = Some(slot.profile.clone());
                        effects.push(MenuEffect::Bind(Box::new(slot)));
                    }
                    Ok(None) => self.status = Some(format!("profile {id} not found")),
                    Err(e) => self.status = Some(e.to_string()),
                }
            }
            Action::CreateProfile => {
                let Some(store) = &data.store else {
                    self.status = Some("profile store unavailable".into());
                    return;
                };
                // Auto-named — a rename/text-entry flow is a later
                // slice; the id, not the name, is the identity.
                let name = format!("Driver {}", data.profiles.len() + 1);
                let request = ProfileRequest::Create {
                    name,
                    rank: self.difficulty,
                    kind: mm2_game::ProfileKind::Standard,
                };
                match crate::profile::resolve(store, &request) {
                    Ok(Some(slot)) => {
                        info!(profile = %slot.profile.id, name = %slot.profile.name, "driver created");
                        self.status = Some(format!("created {}", slot.profile.name));
                        data.bound = Some(slot.profile.clone());
                        effects.push(MenuEffect::Bind(Box::new(slot)));
                    }
                    Ok(None) => self.status = Some("profile create returned nothing".into()),
                    Err(e) => self.status = Some(e.to_string()),
                }
                data.refresh_profiles();
            }
            Action::DriveProfileless => {
                data.bound = None;
                effects.push(MenuEffect::Unbind);
                self.status = Some("no driver profile - progress will not be saved".into());
            }
            Action::AskDelete { id, label } => self.push(Screen::ConfirmDelete { id, label }),
            Action::ConfirmDelete(id) => {
                let Some(store) = &data.store else {
                    self.status = Some("profile store unavailable".into());
                    return;
                };
                let outcome = match store.delete(&id) {
                    Ok(()) => {
                        info!(profile = %id, "driver profile deleted");
                        if data.bound.as_ref().is_some_and(|p| p.id == id) {
                            data.bound = None;
                            effects.push(MenuEffect::Unbind);
                        }
                        "profile deleted".to_string()
                    }
                    // DRV-7's last-profile refusal lands here, as does
                    // any I/O failure — the reason stays visible.
                    Err(e) => e.to_string(),
                };
                data.refresh_profiles();
                // Back to the profile list either way — a refused
                // delete shows its reason on that screen's status
                // line. `pop` clears `status`, so it runs first.
                self.pop();
                self.status = Some(outcome);
            }
            Action::Quit => effects.push(MenuEffect::Exit),
        }
    }

    /// Build the launch effect for a mode/city pick. The vehicle is
    /// resolved here so a load failure is a menu status line, not a
    /// half-launched session.
    fn launch(
        &mut self,
        data: &MenuData,
        vfs: &Vfs,
        mode: SessionMode,
        city: String,
        effects: &mut Vec<MenuEffect>,
    ) {
        let config = SessionConfig {
            world: WorldMode::City {
                psdl: format!("city/{city}.psdl"),
            },
            mode,
            difficulty: self.difficulty,
            vehicle: self.vehicle.clone(),
            mods_active: data.has_mods,
            ..SessionConfig::default()
        };
        let car = match &self.vehicle.id {
            Some(id) => match mm2_content::load_by_id(vfs, id, self.vehicle.paint) {
                Ok(def) => Some(Box::new(def)),
                Err(e) => {
                    self.status = Some(format!("vehicle {id}: {e}"));
                    return;
                }
            },
            None => None,
        };
        effects.push(MenuEffect::Launch {
            config: Box::new(config),
            car,
            paint: self.vehicle.paint,
        });
    }
}

/// Display name for an event table.
fn table_name(table: EventTableKind) -> &'static str {
    match table {
        EventTableKind::Checkpoint => "Checkpoint",
        EventTableKind::Blitz => "Blitz",
        EventTableKind::Circuit => "Circuit",
        EventTableKind::CrashCourse => "Crash Course",
    }
}

const TABLE_KINDS: [EventTableKind; 4] = [
    EventTableKind::Checkpoint,
    EventTableKind::Blitz,
    EventTableKind::Circuit,
    EventTableKind::CrashCourse,
];

/// Rebuild the current screen's rows from the catalogs — called on
/// every `dirty` so the list always reflects the latest store/catalog
/// state.
fn rebuild(shell: &mut MenuShell, data: &mut MenuData, vfs: &Vfs) {
    data.ensure_scan(vfs);
    let rows = match &shell.screen {
        Screen::Root => root_rows(shell, data),
        Screen::CruiseCity => data
            .cities
            .iter()
            .map(|city| Row {
                text: city.clone(),
                enabled: data.city_loadable(vfs, city),
                action: Action::LaunchCruise { city: city.clone() },
            })
            .collect(),
        Screen::EventCity => data
            .race_cities
            .iter()
            .map(|city| Row {
                text: city.clone(),
                enabled: data.city_loadable(vfs, city),
                action: Action::Push(Screen::EventTable { city: city.clone() }),
            })
            .collect(),
        Screen::EventTable { city } => {
            let catalog = data.catalog_of(vfs, city);
            TABLE_KINDS
                .iter()
                .map(|kind| {
                    let status = catalog.tables.iter().find(|t| t.table == *kind);
                    let rows_found = status.map(|s| s.rows).unwrap_or(0);
                    let enabled = match status.and_then(|s| s.error.as_ref()) {
                        Some(e) => Err(e.clone()),
                        None if *kind == EventTableKind::CrashCourse => {
                            Err("crash course events are not loadable yet (F21)".to_string())
                        }
                        None if rows_found == 0 => Err("no authored rows".to_string()),
                        None => Ok(()),
                    };
                    Row {
                        text: format!("{} ({rows_found})", table_name(*kind)),
                        enabled,
                        action: Action::Push(Screen::EventList {
                            city: city.clone(),
                            table: *kind,
                        }),
                    }
                })
                .collect()
        }
        Screen::EventList { city, table } => {
            // Owned copies: the `catalog_of` borrow below is `&mut
            // data`, which outstanding `data` borrows would block.
            let availability = data.availability_of(vfs, city).clone();
            let bound = data.bound.clone();
            let catalog = data.catalog_of(vfs, city);
            catalog
                .events
                .iter()
                .filter(|e| e.event_ref.table == *table)
                .map(|e| {
                    let enabled = match &e.status {
                        mm2_content::EventStatus::Incomplete { missing } => {
                            Err(format!("incomplete: {}", missing.join(", ")))
                        }
                        mm2_content::EventStatus::Ready => {
                            let key = mm2_game::EventKey {
                                city: e.event_ref.city.clone(),
                                table: e.event_ref.table,
                                stem: e.stem.clone(),
                            };
                            let avail = match &bound {
                                Some(p) => availability.of(p, &key),
                                // No bound driver evaluates like a
                                // fresh profile — nothing beaten, still
                                // restricted (progress can't persist).
                                None => availability.of_unbound(&key),
                            };
                            match avail {
                                Some(a) if !a.unlocked => Err(format!(
                                    "beat {} first",
                                    a.blocked_by
                                        .iter()
                                        .map(|k| k.stem.as_str())
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                )),
                                _ => Ok(()),
                            }
                        }
                    };
                    Row {
                        text: format!("{} #{} ({})", table_name(*table), e.event_ref.index, e.stem),
                        enabled,
                        action: Action::LaunchEvent(e.event_ref.clone()),
                    }
                })
                .collect()
        }
        Screen::Garage => garage_rows(shell, data),
        Screen::Paints { car } => paint_rows(shell, data, car),
        Screen::Profiles => profile_rows(shell, data),
        Screen::ConfirmDelete { id, label } => vec![
            Row {
                text: format!("Delete {label} - this cannot be undone"),
                enabled: Ok(()),
                action: Action::ConfirmDelete(id.clone()),
            },
            Row {
                text: "Cancel".into(),
                enabled: Ok(()),
                action: Action::Back,
            },
        ],
    };
    shell.rows = rows;
    shell.focus = shell.focus.min(shell.rows.len().saturating_sub(1));
}

fn root_rows(shell: &MenuShell, data: &MenuData) -> Vec<Row> {
    let vehicle_label = match &shell.vehicle.id {
        Some(id) => data
            .catalog
            .as_ref()
            .and_then(|c| c.find(id).ok())
            .map(|e| e.display_name.clone())
            .unwrap_or_else(|| id.clone()),
        None => "synthetic dev car".to_string(),
    };
    let driver_label = match &data.bound {
        Some(p) => format!("{} ({})", p.name, p.id.as_str()),
        None => "none".to_string(),
    };
    vec![
        Row {
            text: "Cruise".into(),
            enabled: if data.cities.is_empty() {
                Err("no city data - pass --mm2-path <install>".to_string())
            } else {
                Ok(())
            },
            action: Action::Push(Screen::CruiseCity),
        },
        Row {
            text: "Events".into(),
            enabled: Ok(()),
            action: Action::Push(Screen::EventCity),
        },
        Row {
            text: format!("Vehicle: {vehicle_label}"),
            enabled: match &data.garage {
                Some(g) if g.rows.iter().any(|r| r.listed) => Ok(()),
                _ => Err("no vehicle data - pass --mm2-path <install>".to_string()),
            },
            action: Action::Push(Screen::Garage),
        },
        Row {
            text: format!("Driver: {driver_label}"),
            enabled: if data.store.is_some() {
                Ok(())
            } else {
                Err("profile store unavailable (--no-profile or unwritable data dir)".to_string())
            },
            action: Action::Push(Screen::Profiles),
        },
        Row {
            text: format!(
                "Difficulty: {}",
                match shell.difficulty {
                    Difficulty::Amateur => "Amateur",
                    Difficulty::Professional => "Professional",
                }
            ),
            enabled: Ok(()),
            action: Action::ToggleDifficulty,
        },
        Row {
            text: "Options".into(),
            enabled: Err("not implemented yet (F23)".to_string()),
            action: Action::Quit, // unreachable while disabled
        },
        Row {
            text: "Multiplayer".into(),
            enabled: Err("not implemented yet (F24)".to_string()),
            action: Action::Quit, // unreachable while disabled
        },
        Row {
            text: "Quit".into(),
            enabled: Ok(()),
            action: Action::Quit,
        },
    ]
}

fn garage_rows(shell: &MenuShell, data: &MenuData) -> Vec<Row> {
    let Some(garage) = &data.garage else {
        return Vec::new();
    };
    garage
        .rows
        .iter()
        // `listed` is the authored select roster — unlisted entries
        // are dev leftovers, not menu rows (F16-B.3's designed
        // reading).
        .filter(|row| row.listed)
        .map(|row| {
            let entry = data.catalog.as_ref().and_then(|c| c.find(&row.id).ok());
            let selected = shell.vehicle.id.as_deref() == Some(row.id.as_str());
            let avail = match &data.bound {
                Some(p) => garage.of(p, &row.id),
                None => garage.of_unbound(&row.id),
            };
            let enabled = if !entry.is_some_and(|e| e.is_ready()) {
                let missing = entry
                    .and_then(|e| match &e.status {
                        mm2_content::EntryStatus::Incomplete { missing } => {
                            Some(missing.join(", "))
                        }
                        _ => None,
                    })
                    .unwrap_or_else(|| "not in the catalog".to_string());
                Err(format!("incomplete: {missing}"))
            } else if !avail.is_some_and(|a| a.unlocked) {
                Err("locked - earned through event rewards".to_string())
            } else {
                Ok(())
            };
            Row {
                text: format!(
                    "{}{}",
                    entry.map(|e| e.display_name.as_str()).unwrap_or(&row.id),
                    if selected { "  *" } else { "" }
                ),
                enabled,
                action: Action::PickVehicle { id: row.id.clone() },
            }
        })
        .collect()
}

fn paint_rows(shell: &MenuShell, data: &MenuData, car: &str) -> Vec<Row> {
    let entry = data.catalog.as_ref().and_then(|c| c.find(car).ok());
    let avail = data.garage.as_ref().and_then(|g| match &data.bound {
        Some(p) => g.of(p, car),
        None => g.of_unbound(car),
    });
    let names: Vec<String> = entry
        .map(|e| e.paints.clone())
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .map(|(i, n)| {
            if n.is_empty() {
                format!("Paint {i}")
            } else {
                n
            }
        })
        .collect();
    let names = if names.is_empty() {
        vec!["Default".to_string()]
    } else {
        names
    };
    names
        .into_iter()
        .enumerate()
        .map(|(i, name)| {
            let enabled = match avail.as_ref().and_then(|a| a.paints.get(i)) {
                Some(false) => Err("locked - earned through event rewards".to_string()),
                _ => Ok(()),
            };
            Row {
                text: format!(
                    "{name}{}",
                    if shell.vehicle.paint == i { "  *" } else { "" }
                ),
                enabled,
                action: Action::PickPaint {
                    car: car.to_string(),
                    index: i,
                },
            }
        })
        .collect()
}

fn profile_rows(_shell: &MenuShell, data: &mut MenuData) -> Vec<Row> {
    data.refresh_profiles();
    let mut rows: Vec<Row> = data
        .profiles
        .iter()
        .map(|p| {
            let bound = data.bound.as_ref().is_some_and(|b| b.id == p.id);
            let text = match &p.meta {
                Some(m) => format!(
                    "{}{} ({}, {}{})",
                    m.name,
                    if bound { " *" } else { "" },
                    p.id.as_str(),
                    match m.rank {
                        Difficulty::Amateur => "Amateur",
                        Difficulty::Professional => "Professional",
                    },
                    match m.kind {
                        mm2_game::ProfileKind::Standard => "",
                        mm2_game::ProfileKind::Sandbox => ", sandbox",
                    }
                ),
                None => format!(
                    "{} (unreadable{})",
                    p.id.as_str(),
                    if bound { ", bound" } else { "" }
                ),
            };
            Row {
                text,
                enabled: Ok(()),
                action: Action::BindProfile(p.id.clone()),
            }
        })
        .collect();
    rows.push(Row {
        text: "New driver".into(),
        enabled: Ok(()),
        action: Action::CreateProfile,
    });
    rows.push(Row {
        text: "Drive without a profile".into(),
        enabled: if data.bound.is_some() {
            Ok(())
        } else {
            Err("already driving without a profile".to_string())
        },
        action: Action::DriveProfileless,
    });
    rows
}

/// Marker for menu UI entities — persistent interface owned by the
/// menu, never `SessionEntity`-stamped (it outlives sessions).
#[derive(Component)]
pub struct MenuUi;

/// Marker for the menu's `Camera2d` — the view `bevy_ui` draws the
/// `MenuUi` tree into. Session cameras are `SessionEntity`-stamped and
/// only exist in-game, so while the menu owns the screen nothing else
/// provides a render target. Deliberately not `MenuUi`: it survives the
/// per-redraw rebuild of the text tree and is despawned only when the
/// menu hands the screen to a session.
#[derive(Component)]
pub struct MenuCamera;

/// Reopen the shell whenever the session reaches `Menu` — this is what
/// makes a quit from a menu-launched session return to the menu, and
/// refreshes the bound-profile view from the resource the session
/// systems updated.
pub fn menu_watch(
    session: Res<Session>,
    active: Option<Res<ActiveProfile>>,
    mut shell: ResMut<MenuShell>,
    mut data: ResMut<MenuData>,
) {
    if matches!(session.phase(), SessionPhase::Menu) && !shell.active {
        shell.reopen();
        data.bound = active.map(|a| a.profile.clone());
    }
}

/// ECS targets a launch/bind/exit effect writes to — bundled so the
/// input system stays under the argument lint.
#[derive(bevy::ecs::system::SystemParam)]
pub struct MenuTarget<'w, 's> {
    session: ResMut<'w, Session>,
    selected: ResMut<'w, SelectedCar>,
    tuned: ResMut<'w, TunedVehicle>,
    commands: Commands<'w, 's>,
    exit: MessageWriter<'w, AppExit>,
}

/// Map keyboard + gamepad into [`MenuCommand`]s, run them through
/// `apply`, and execute the effects — launches call `Session::begin`,
/// which `load_session_world` picks up on the next update like any
/// other `Menu → Loading` transition.
pub fn menu_input(
    keys: Res<ButtonInput<KeyCode>>,
    pads: Query<&Gamepad>,
    mut shell: ResMut<MenuShell>,
    mut data: ResMut<MenuData>,
    vfs: Res<Mm2Vfs>,
    mut target: MenuTarget,
) {
    if !shell.active {
        return;
    }
    let mut cmds = Vec::new();
    if keys.just_pressed(KeyCode::ArrowUp) || keys.just_pressed(KeyCode::KeyW) {
        cmds.push(MenuCommand::Up);
    }
    if keys.just_pressed(KeyCode::ArrowDown) || keys.just_pressed(KeyCode::KeyS) {
        cmds.push(MenuCommand::Down);
    }
    if keys.just_pressed(KeyCode::ArrowLeft) || keys.just_pressed(KeyCode::KeyA) {
        cmds.push(MenuCommand::Left);
    }
    if keys.just_pressed(KeyCode::ArrowRight) || keys.just_pressed(KeyCode::KeyD) {
        cmds.push(MenuCommand::Right);
    }
    if keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::Space) {
        cmds.push(MenuCommand::Activate);
    }
    if keys.just_pressed(KeyCode::Escape) || keys.just_pressed(KeyCode::Backspace) {
        cmds.push(MenuCommand::Back);
    }
    if keys.just_pressed(KeyCode::Delete) || keys.just_pressed(KeyCode::KeyX) {
        cmds.push(MenuCommand::Delete);
    }
    if let Some(pad) = pads.iter().next() {
        if pad.just_pressed(GamepadButton::DPadUp) {
            cmds.push(MenuCommand::Up);
        }
        if pad.just_pressed(GamepadButton::DPadDown) {
            cmds.push(MenuCommand::Down);
        }
        if pad.just_pressed(GamepadButton::DPadLeft) {
            cmds.push(MenuCommand::Left);
        }
        if pad.just_pressed(GamepadButton::DPadRight) {
            cmds.push(MenuCommand::Right);
        }
        if pad.just_pressed(GamepadButton::South) {
            cmds.push(MenuCommand::Activate);
        }
        if pad.just_pressed(GamepadButton::East) {
            cmds.push(MenuCommand::Back);
        }
        if pad.just_pressed(GamepadButton::West) {
            cmds.push(MenuCommand::Delete);
        }
        // Left-stick nav on edge transitions, so holding the stick
        // doesn't run the list.
        let y = pad.get(GamepadAxis::LeftStickY).unwrap_or(0.0);
        if y > 0.6 && shell.pad_axis <= 0.6 {
            cmds.push(MenuCommand::Up);
        } else if y < -0.6 && shell.pad_axis >= -0.6 {
            cmds.push(MenuCommand::Down);
        }
        shell.pad_axis = y;
    }
    for cmd in cmds {
        for effect in shell.apply(cmd, &mut data, &vfs.0) {
            match effect {
                MenuEffect::Launch { config, car, paint } => {
                    let tune = car.as_ref().map(|d| d.config.clone()).unwrap_or_default();
                    *target.selected = SelectedCar {
                        def: car.map(|d| *d),
                        paint,
                    };
                    target.tuned.0 = tune;
                    match target.session.begin(*config) {
                        Ok(()) => shell.active = false,
                        Err(e) => {
                            warn!(error = %e, "menu launch produced an invalid session config");
                            shell.status = Some(format!("session config rejected: {e}"));
                        }
                    }
                }
                MenuEffect::Bind(slot) => {
                    target.commands.insert_resource(*slot);
                }
                MenuEffect::Unbind => {
                    target.commands.remove_resource::<ActiveProfile>();
                }
                MenuEffect::Exit => {
                    target.exit.write(AppExit::Success);
                }
            }
        }
    }
}

/// Title shown above a screen's rows.
fn screen_title(screen: &Screen) -> String {
    match screen {
        Screen::Root => "rust-mm2".to_string(),
        Screen::CruiseCity => "Cruise - pick a city".to_string(),
        Screen::EventCity => "Events - pick a city".to_string(),
        Screen::EventTable { city } => format!("Events - {city}"),
        Screen::EventList { city, table } => {
            format!("{} - {}", table_name(*table), city)
        }
        Screen::Garage => "Vehicle".to_string(),
        Screen::Paints { car } => format!("Paint - {car}"),
        Screen::Profiles => "Driver profiles - X deletes".to_string(),
        Screen::ConfirmDelete { label, .. } => format!("Delete {label}?"),
    }
}

/// (Re)draw the menu while it is active: keep a `Camera2d` up as the
/// UI's render target, rebuild the row model when `dirty`, then respawn
/// the text tree. Entities carry `MenuUi`, not `SessionEntity` — the
/// menu outlives sessions. The camera carries `MenuCamera` so the
/// teardown path can drop it with the shell while redraws leave it
/// alone.
pub fn menu_present(
    mut commands: Commands,
    mut shell: ResMut<MenuShell>,
    mut data: ResMut<MenuData>,
    vfs: Res<Mm2Vfs>,
    roots: Query<Entity, (With<MenuUi>, Without<ChildOf>)>,
    cameras: Query<Entity, With<MenuCamera>>,
) {
    if !shell.active {
        for root in &roots {
            commands.entity(root).despawn();
        }
        for camera in &cameras {
            commands.entity(camera).despawn();
        }
        return;
    }
    // The menu owns the screen — it must bring its own render target,
    // or the `Node`/`Text` tree below is built but never drawn (the
    // world holds zero cameras at boot and after each quit-to-menu).
    let camera = match cameras.iter().next() {
        Some(camera) => camera,
        None => commands.spawn((MenuCamera, Camera2d)).id(),
    };
    if !shell.dirty {
        return;
    }
    shell.dirty = false;
    rebuild(&mut shell, &mut data, &vfs.0);
    for root in &roots {
        commands.entity(root).despawn();
    }

    let mut lines: Vec<(String, f32, Color)> = Vec::new();
    lines.push((
        screen_title(&shell.screen),
        34.0,
        Color::srgb(0.95, 0.9, 0.6),
    ));
    lines.push((String::new(), 8.0, Color::NONE));
    for (i, row) in shell.rows.iter().enumerate() {
        let (text, color) = match &row.enabled {
            Ok(()) => (
                if i == shell.focus {
                    // The bundled font has no `›` glyph — it renders
                    // as tofu, which would erase the focus marker.
                    format!("> {}", row.text)
                } else {
                    format!("  {}", row.text)
                },
                if i == shell.focus {
                    Color::srgb(1.0, 1.0, 1.0)
                } else {
                    Color::srgb(0.75, 0.75, 0.8)
                },
            ),
            Err(reason) => (
                format!("  {} - {reason}", row.text),
                Color::srgb(0.45, 0.45, 0.5),
            ),
        };
        lines.push((text, 22.0, color));
    }
    lines.push((String::new(), 8.0, Color::NONE));
    if let Some(status) = &shell.status {
        lines.push((status.clone(), 18.0, Color::srgb(1.0, 0.75, 0.35)));
    }
    lines.push((
        "Up/Down move | Enter select | Esc back | X delete".to_string(),
        14.0,
        Color::srgb(0.5, 0.5, 0.55),
    ));

    commands
        .spawn((
            MenuUi,
            // Pin the tree to the menu camera — without it the UI
            // would fall back to the default camera, which is nothing
            // while no session world is loaded.
            UiTargetCamera(camera),
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(0.0),
                left: Val::Px(0.0),
                right: Val::Px(0.0),
                bottom: Val::Px(0.0),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                padding: UiRect::left(Val::Px(90.0)),
                row_gap: Val::Px(4.0),
                ..default()
            },
            BackgroundColor(Color::srgba(0.02, 0.03, 0.07, 0.88)),
        ))
        .with_children(|parent| {
            for (text, size, color) in lines {
                parent.spawn((
                    MenuUi,
                    Text::new(text),
                    TextFont {
                        font_size: bevy::text::FontSize::Px(size),
                        ..default()
                    },
                    TextColor(color),
                ));
            }
        });
}
