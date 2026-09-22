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
//!   exiting. `menu_mouse` adds the mouse path: hover focuses, left
//!   click activates, right click backs out — producing the same
//!   [`MenuCommand`]s so every effect still executes in `menu_input`.
//! - [`Screen::NewProfile`] is a text field rather than a row list:
//!   `menu_input` routes `KeyboardInput.text` into it so driver names
//!   are typed, not auto-generated.
//! - [`Screen::Records`] is the bound driver's race-records view
//!   (DRV-5's first leg): the persisted per-event finishes/best
//!   results with city and race-type filters; an enabled record row
//!   re-launches its event. The documented original's remaining sort
//!   keys (Amateur/Pro Times, Pro Points — nothing persists points)
//!   and Driver's Stats stay open in `docs/research/menu.md`.
//!
//! - [`Screen::Customize`] is the condition-options screen (UI-2):
//!   weather, time-of-day and traffic density — the authored option
//!   fields with runtime consumers today (lighting F18-A.2, ambient
//!   traffic F10). Cruise offers it unconditionally (RACE-4); an event
//!   offers it once its own record is beaten — RACE-3's
//!   `EventAvailability::customizable`. Picks ride the screen seeded
//!   from the session's defaults and launch through
//!   `SessionConfig::customization`; an unchanged pick set launches a
//!   default run so DRV-6 record eligibility is unaffected by a visit.
//!
//! Deferred to later slices (honest gaps, not placeholders):
//! pedestrian/cop density and Circuit laps/opponents options (no
//! consumers for the first pair — F19/F20 — and laps/opponents sit in
//! DRV-6's explicit exclusion list), Quick Race customization,
//! Driver's Stats (no aggregate stats are persisted), original menu
//! art and audio. The in-session overlays landed in their own modules
//! — `crate::pause`, `crate::results`.

use std::collections::BTreeMap;

use bevy::input::keyboard::KeyboardInput;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use mm2_assets::Vfs;
use mm2_content::{EventCatalog, VehicleCatalog, VehicleDef};
use mm2_game::{
    AvailabilityTable, Densities, Difficulty, EventRef, EventTableKind, GarageTable,
    MAX_NAME_CHARS, Mm2Vfs, PlayerProfile, ProfileId, ProfileStore, ProfileSummary, Session,
    SessionConditions, SessionConfig, SessionCustomization, SessionMode, SessionPhase, TimeOfDay,
    VehicleSelection, Weather, WorldMode,
};
use tracing::{info, warn};

use crate::profile::{ActiveProfile, ProfileRequest};
use crate::session::{SelectedCar, SessionControl, SessionNote, TunedVehicle};

/// One user intent. Keyboard, gamepad, mouse and tests all produce
/// these — the model never reads devices.
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
    /// Focus a row by index — the mouse hover path (`menu_mouse`)
    /// produces this; keyboard/gamepad use relative moves instead.
    /// Out-of-range or already-focused indices are no-ops.
    FocusAt(usize),
    /// Append a character to a text field — only [`Screen::NewProfile`]
    /// accepts text today. Production input feeds this from
    /// `KeyboardInput.text` so layout, Shift and dead keys resolve to
    /// the character the OS intended.
    Type(char),
    /// Erase the last character of a text field (Backspace).
    Erase,
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
    /// Name-entry step for a new driver — a text field, not a row
    /// list. `name` is the edit buffer; Enter creates and binds, Esc
    /// cancels. The created profile takes the shell's current rank
    /// selection, matching the auto-name path it replaced.
    NewProfile {
        /// The name being typed.
        name: String,
    },
    /// The bound driver's persisted race records (DRV-5's first leg).
    /// `city`/`table` are the active filters — `None` means "all";
    /// they ride the screen like `NewProfile`'s buffer so Left/Right
    /// cycle them in place.
    Records {
        /// City filter.
        city: Option<String>,
        /// Race-type filter.
        table: Option<EventTableKind>,
    },
    /// Condition options for a cruise or a customization-unlocked
    /// event (UI-2, RACE-3/RACE-4). `conditions`/`densities` are the
    /// working picks Left/Right adjusts in place; the `seed_*` fields
    /// record what the session runs without customization, so a
    /// launch whose picks equal the seed stays a default run (DRV-6).
    Customize {
        /// Which session the options configure.
        target: CustomizeTarget,
        /// Working weather/time-of-day picks — seeded from the
        /// event's authored params, or the neutral defaults on cruise.
        conditions: SessionConditions,
        /// Working densities — `traffic` is the only exposed control
        /// (ambient traffic consumes it); `pedestrians` rides the
        /// seed untouched pending its F19 consumer.
        densities: Densities,
        /// The session's default conditions for this target.
        seed_conditions: SessionConditions,
        /// The session's default densities for this target.
        seed_densities: Densities,
    },
}

/// What a [`Screen::Customize`] configures — the screen's launch row
/// turns it back into the session mode.
#[derive(Debug, Clone, PartialEq)]
pub enum CustomizeTarget {
    /// Free-roam options for a city (RACE-4 — always open).
    Cruise {
        /// City stem.
        city: String,
    },
    /// Per-event options (RACE-3 — gated on `customizable`).
    Event {
        /// The event being customized.
        event_ref: EventRef,
        /// Catalog stem, for the title.
        stem: String,
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
    /// Cycle the Records screen's city filter (Left/Right or Activate).
    RecordsCityFilter,
    /// Cycle the Records screen's race-type filter.
    RecordsTableFilter,
    /// Cycle the Customize screen's weather selector.
    CycleWeather,
    /// Cycle the Customize screen's time-of-day selector.
    CycleTimeOfDay,
    /// Cycle the Customize screen's traffic-density pick.
    CycleTrafficDensity,
    /// Launch the session the Customize screen configures.
    LaunchCustomize,
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
    /// Commands produced by device systems outside `menu_input` (the
    /// mouse path) — drained first every update so every effect still
    /// executes in `menu_input`.
    pub pending: Vec<MenuCommand>,
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
            pending: Vec::new(),
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
        // Stale device commands must not fire against the reopened
        // shell — a click queued while a session ran has no screen.
        self.pending.clear();
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
        // The name-entry screen is a text field, not a row list: it
        // owns typing, erase, Enter (create) and Esc (cancel) and
        // ignores the nav commands. Other screens ignore text.
        if let Screen::NewProfile { name } = &mut self.screen {
            match cmd {
                MenuCommand::Type(c) => {
                    // The bundled font is ASCII-only — accepting a
                    // character it cannot draw would store a name that
                    // renders as tofu.
                    if !c.is_ascii() || c.is_ascii_control() {
                        self.status = Some("names use ASCII characters only".into());
                    } else if name.chars().count() >= MAX_NAME_CHARS {
                        self.status =
                            Some(format!("names are at most {MAX_NAME_CHARS} characters"));
                    } else {
                        name.push(c);
                        self.status = None;
                    }
                }
                MenuCommand::Erase => {
                    name.pop();
                    self.status = None;
                }
                MenuCommand::Back => {
                    self.pop();
                }
                MenuCommand::Activate => {
                    let name = name.trim().to_string();
                    if name.is_empty() {
                        self.status = Some("type a name first".into());
                    } else {
                        self.create_profile(data, name, &mut effects);
                    }
                }
                _ => {}
            }
            self.dirty = true;
            return effects;
        }
        match cmd {
            MenuCommand::Up => self.focus = self.focus.saturating_sub(1),
            MenuCommand::Down => {
                self.focus = (self.focus + 1).min(self.rows.len().saturating_sub(1))
            }
            MenuCommand::FocusAt(i) => {
                // Mouse hover lands here. A no-op (same row, off-list
                // index) must not dirty — hover alone cannot justify
                // a redraw, so this arm skips the unconditional mark.
                if i != self.focus && i < self.rows.len() {
                    self.focus = i;
                    self.dirty = true;
                }
                return effects;
            }
            MenuCommand::Left | MenuCommand::Right => {
                let forward = cmd == MenuCommand::Right;
                if let Some(action) = self.rows.get(self.focus).map(|r| r.action.clone()) {
                    self.adjust_with(data, &action, forward);
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
            // Row screens carry no text field — typing is inert.
            MenuCommand::Type(_) | MenuCommand::Erase => {}
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
            // Activate on a filter or option row cycles forward, same
            // as Right.
            action @ (Action::RecordsCityFilter
            | Action::RecordsTableFilter
            | Action::CycleWeather
            | Action::CycleTimeOfDay
            | Action::CycleTrafficDensity) => self.adjust_with(data, &action, true),
            Action::LaunchCustomize => {
                let Screen::Customize {
                    target,
                    conditions,
                    densities,
                    seed_conditions,
                    seed_densities,
                } = &self.screen
                else {
                    return;
                };
                let (mode, city) = match target {
                    CustomizeTarget::Cruise { city } => (SessionMode::Cruise, city.clone()),
                    CustomizeTarget::Event { event_ref, .. } => (
                        SessionMode::Event(event_ref.clone()),
                        event_ref.city.clone(),
                    ),
                };
                // Picks that match the session's defaults launch a
                // default run — a visit that changed nothing is not a
                // customized run (DRV-6 eligibility).
                let customization = (conditions != seed_conditions || densities != seed_densities)
                    .then_some(SessionCustomization {
                        conditions: *conditions,
                        densities: *densities,
                    });
                self.launch(data, vfs, mode, city, customization, effects);
            }
            Action::LaunchCruise { city } => {
                self.launch(data, vfs, SessionMode::Cruise, city, None, effects)
            }
            Action::LaunchEvent(event_ref) => {
                let city = event_ref.city.clone();
                self.launch(
                    data,
                    vfs,
                    SessionMode::Event(event_ref),
                    city,
                    None,
                    effects,
                )
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

    /// Create and bind a profile from the name-entry buffer. Success
    /// pops back to the profile list; a store/validation failure keeps
    /// the entry screen open with the reason on its status line so the
    /// typed name isn't lost.
    fn create_profile(&mut self, data: &mut MenuData, name: String, effects: &mut Vec<MenuEffect>) {
        let Some(store) = &data.store else {
            self.status = Some("profile store unavailable".into());
            return;
        };
        let request = ProfileRequest::Create {
            name,
            rank: self.difficulty,
            kind: mm2_game::ProfileKind::Standard,
        };
        match crate::profile::resolve(store, &request) {
            Ok(Some(slot)) => {
                info!(profile = %slot.profile.id, name = %slot.profile.name, "driver created");
                let status = format!("created {}", slot.profile.name);
                data.bound = Some(slot.profile.clone());
                effects.push(MenuEffect::Bind(Box::new(slot)));
                data.refresh_profiles();
                self.pop();
                self.status = Some(status);
            }
            Ok(None) => self.status = Some("profile create returned nothing".into()),
            Err(e) => self.status = Some(e.to_string()),
        }
    }

    /// The Left/Right (and Activate-on-option-row) adjustment shared by
    /// every value row — difficulty, Records filters and the Customize
    /// screen's condition picks.
    fn adjust_with(&mut self, data: &mut MenuData, action: &Action, forward: bool) {
        match action {
            Action::ToggleDifficulty => {
                self.difficulty = match self.difficulty {
                    Difficulty::Amateur => Difficulty::Professional,
                    Difficulty::Professional => Difficulty::Amateur,
                };
            }
            Action::RecordsCityFilter => self.cycle_record_filter(data, true, forward),
            Action::RecordsTableFilter => self.cycle_record_filter(data, false, forward),
            Action::CycleWeather => {
                if let Screen::Customize { conditions, .. } = &mut self.screen {
                    conditions.weather =
                        Weather::new(step4(conditions.weather.get(), forward)).unwrap();
                }
            }
            Action::CycleTimeOfDay => {
                if let Screen::Customize { conditions, .. } = &mut self.screen {
                    conditions.time_of_day =
                        TimeOfDay::new(step4(conditions.time_of_day.get(), forward)).unwrap();
                }
            }
            Action::CycleTrafficDensity => {
                if let Screen::Customize { densities, .. } = &mut self.screen {
                    densities.traffic = step_density(densities.traffic, forward);
                }
            }
            _ => {}
        }
    }

    /// Cycle one of the Records screen's filters through `None` (all)
    /// plus the values actually present in the bound driver's records.
    /// `city` picks which filter; `forward` the direction — Left steps
    /// back, Right and Activate step forward.
    fn cycle_record_filter(&mut self, data: &MenuData, city: bool, forward: bool) {
        let Some(bound) = &data.bound else { return };
        let Screen::Records {
            city: city_filter,
            table: table_filter,
        } = &mut self.screen
        else {
            return;
        };
        if city {
            let mut choices: Vec<&str> = bound
                .progress
                .events
                .iter()
                .map(|r| r.key.city.as_str())
                .collect();
            choices.sort();
            choices.dedup();
            *city_filter = cycle_choice(&choices, city_filter.as_deref(), forward).map(Into::into);
        } else {
            // Authored table order, not lexicographic.
            let choices: Vec<EventTableKind> = TABLE_KINDS
                .iter()
                .copied()
                .filter(|k| bound.progress.events.iter().any(|r| r.key.table == *k))
                .collect();
            *table_filter = cycle_choice(&choices, *table_filter, forward);
        }
    }

    /// Build the launch effect for a mode/city pick. The vehicle is
    /// resolved here so a load failure is a menu status line, not a
    /// half-launched session. `customization` is the Customize
    /// screen's picks — `None` on every direct launch, which is also
    /// what an unchanged visit produces.
    fn launch(
        &mut self,
        data: &MenuData,
        vfs: &Vfs,
        mode: SessionMode,
        city: String,
        customization: Option<SessionCustomization>,
        effects: &mut Vec<MenuEffect>,
    ) {
        let config = SessionConfig {
            world: WorldMode::City {
                psdl: format!("city/{city}.psdl"),
            },
            mode,
            difficulty: self.difficulty,
            vehicle: self.vehicle.clone(),
            customization,
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
        Screen::Root => root_rows(shell, data, vfs),
        Screen::CruiseCity => data
            .cities
            .iter()
            .flat_map(|city| {
                let enabled = data.city_loadable(vfs, city);
                [
                    Row {
                        text: city.clone(),
                        enabled: enabled.clone(),
                        action: Action::LaunchCruise { city: city.clone() },
                    },
                    // RACE-4: cruise condition options are always open.
                    Row {
                        text: "  options".to_string(),
                        enabled,
                        action: Action::Push(Screen::Customize {
                            target: CustomizeTarget::Cruise { city: city.clone() },
                            conditions: SessionConditions::default(),
                            densities: Densities::DEFAULT,
                            seed_conditions: SessionConditions::default(),
                            seed_densities: Densities::DEFAULT,
                        }),
                    },
                ]
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
            let difficulty = shell.difficulty;
            let catalog = data.catalog_of(vfs, city);
            catalog
                .events
                .iter()
                .filter(|e| e.event_ref.table == *table)
                .flat_map(|e| {
                    let avail = match &e.status {
                        mm2_content::EventStatus::Ready => {
                            let key = mm2_game::EventKey {
                                city: e.event_ref.city.clone(),
                                table: e.event_ref.table,
                                stem: e.stem.clone(),
                            };
                            match &bound {
                                Some(p) => availability.of(p, &key),
                                // No bound driver evaluates like a
                                // fresh profile — nothing beaten, still
                                // restricted (progress can't persist).
                                None => availability.of_unbound(&key),
                            }
                        }
                        mm2_content::EventStatus::Incomplete { .. } => None,
                    };
                    let enabled = match &e.status {
                        mm2_content::EventStatus::Incomplete { missing } => {
                            Err(format!("incomplete: {}", missing.join(", ")))
                        }
                        mm2_content::EventStatus::Ready => availability_reason(avail.clone()),
                    };
                    [
                        Row {
                            text: format!(
                                "{} #{} ({})",
                                table_name(*table),
                                e.event_ref.index,
                                e.stem
                            ),
                            enabled: enabled.clone(),
                            action: Action::LaunchEvent(e.event_ref.clone()),
                        },
                        options_row(e, &enabled, bound.as_ref(), &avail, difficulty),
                    ]
                })
                .collect()
        }
        Screen::Records { city, table } => {
            let city = city.clone();
            let table = *table;
            record_rows(data, vfs, city.as_deref(), table)
        }
        Screen::Customize {
            target,
            conditions,
            densities,
            ..
        } => {
            let start = match target {
                CustomizeTarget::Cruise { .. } => "Start cruise",
                CustomizeTarget::Event { .. } => "Start race",
            };
            vec![
                Row {
                    text: format!("Weather: {}", conditions.weather.name()),
                    enabled: Ok(()),
                    action: Action::CycleWeather,
                },
                Row {
                    text: format!("Time of day: {}", conditions.time_of_day.name()),
                    enabled: Ok(()),
                    action: Action::CycleTimeOfDay,
                },
                Row {
                    text: format!("Traffic density: {:.0}%", densities.traffic * 100.0),
                    enabled: Ok(()),
                    action: Action::CycleTrafficDensity,
                },
                Row {
                    text: start.to_string(),
                    enabled: Ok(()),
                    action: Action::LaunchCustomize,
                },
            ]
        }
        Screen::Garage => garage_rows(shell, data),
        Screen::Paints { car } => paint_rows(shell, data, car),
        Screen::Profiles => profile_rows(shell, data),
        // A text field, not a row list — the buffer lives on the
        // screen and `menu_present` draws it.
        Screen::NewProfile { .. } => Vec::new(),
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

fn root_rows(shell: &MenuShell, data: &mut MenuData, vfs: &Vfs) -> Vec<Row> {
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
        quick_race_row(data, vfs),
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
            text: "Race Records".into(),
            enabled: if data.bound.is_some() {
                Ok(())
            } else {
                Err("no driver profile - records are kept per driver".to_string())
            },
            action: Action::Push(Screen::Records {
                city: None,
                table: None,
            }),
        },
        // AC05's tracked capability: the original's stats screen has
        // no persisted data to draw on yet, so the row names the gap
        // rather than opening an empty page.
        Row {
            text: "Driver's Stats".into(),
            enabled: Err("not implemented yet (menu audit: docs/research/menu.md)".to_string()),
            action: Action::Quit, // unreachable while disabled
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

/// The Quick Race row (DRV-8): relaunch the bound profile's
/// `last_event` with the current vehicle/paint/difficulty selections.
/// The documented original inserts a vehicle-select screen between the
/// pick and the launch; this shell already keeps vehicle/difficulty as
/// persistent root selections, so the row launches directly — an
/// enhanced-layout choice, not an original-rules claim.
///
/// The stem-keyed [`mm2_game::EventKey`] is resolved back through the
/// live catalog: a mod that deleted or broke the event disables the row
/// with the reason instead of launching whatever row now sits at the
/// saved index (the whole point of storing the stem, spec req 4).
fn quick_race_row(data: &mut MenuData, vfs: &Vfs) -> Row {
    let disabled = |text: String, reason: String| Row {
        text,
        enabled: Err(reason),
        action: Action::Back, // unreachable while disabled
    };
    let bound = data.bound.clone();
    let Some(key) = bound.as_ref().and_then(|p| p.selections.last_event.clone()) else {
        return disabled(
            "Quick Race".into(),
            if data.bound.is_none() {
                "no driver profile - quick race replays the last event".to_string()
            } else {
                "no event played yet".to_string()
            },
        );
    };
    let text = format!("Quick Race: {}", key.stem);
    if key.table == EventTableKind::CrashCourse {
        return disabled(
            text,
            "crash course events are not loadable yet (F21)".to_string(),
        );
    }
    if let Err(reason) = data.city_loadable(vfs, &key.city) {
        return disabled(text, reason);
    }
    let availability = data.availability_of(vfs, &key.city).clone();
    let Some(event) = data
        .catalog_of(vfs, &key.city)
        .events
        .iter()
        .find(|e| e.event_ref.table == key.table && e.stem == key.stem)
    else {
        return disabled(
            text,
            format!("{} is not in the {} catalog any more", key.stem, key.city),
        );
    };
    let enabled = match &event.status {
        mm2_content::EventStatus::Incomplete { missing } => {
            Err(format!("incomplete: {}", missing.join(", ")))
        }
        mm2_content::EventStatus::Ready => {
            let profile = bound.as_ref().expect("a key implies a bound profile");
            availability_reason(availability.of(profile, &key))
        }
    };
    Row {
        text: format!(
            "Quick Race: {} #{} ({})",
            table_name(key.table),
            event.event_ref.index,
            key.stem
        ),
        enabled,
        action: Action::LaunchEvent(event.event_ref.clone()),
    }
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
        action: Action::Push(Screen::NewProfile {
            name: String::new(),
        }),
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

/// `current`'s next value through `None → choices[0] → … → None`,
/// wrapping — `forward` walks the list, `!forward` walks it back. A
/// `current` that is absent from `choices` behaves as `None`.
fn cycle_choice<T: PartialEq + Copy>(
    choices: &[T],
    current: Option<T>,
    forward: bool,
) -> Option<T> {
    let len = choices.len() + 1;
    let pos = current
        .and_then(|c| choices.iter().position(|v| *v == c))
        .map(|i| i + 1)
        .unwrap_or(0);
    let next = if forward {
        (pos + 1) % len
    } else {
        (pos + len - 1) % len
    };
    (next > 0).then(|| choices[next - 1])
}

/// The RACE-3 per-event options row: condition options open once the
/// event's own record is beaten (`EventAvailability::customizable`),
/// disabled with the reason otherwise — the capability stays visible
/// instead of hiding (AC05). The pushed screen seeds its picks from
/// the difficulty's authored block so a zero-change launch stays a
/// default run.
fn options_row(
    e: &mm2_content::CatalogEvent,
    event_enabled: &Result<(), String>,
    bound: Option<&PlayerProfile>,
    avail: &Option<mm2_game::EventAvailability>,
    difficulty: Difficulty,
) -> Row {
    let gate = match event_enabled {
        Err(reason) => Err(reason.clone()),
        Ok(()) => match (bound, avail) {
            (None, _) => Err("no driver profile - options unlock per driver".to_string()),
            (_, Some(a)) if !a.customizable => {
                Err("beat this race to unlock its options".to_string())
            }
            (_, None) => Err("no availability rule for this event".to_string()),
            _ => Ok(()),
        },
    };
    // A seed that cannot be read as authored params disables the row
    // rather than fabricating defaults — the event's own build would
    // reject the same values at load.
    let (gate, seed) = match gate {
        Ok(()) => match authored_seed(e.race_params(difficulty)) {
            Some(seed) => (Ok(()), seed),
            None => (
                Err("authored conditions are out of range".to_string()),
                (SessionConditions::default(), Densities::DEFAULT),
            ),
        },
        Err(reason) => (
            Err(reason),
            (SessionConditions::default(), Densities::DEFAULT),
        ),
    };
    Row {
        text: "  options".to_string(),
        enabled: gate,
        action: Action::Push(Screen::Customize {
            target: CustomizeTarget::Event {
                event_ref: e.event_ref.clone(),
                stem: e.stem.clone(),
            },
            conditions: seed.0,
            densities: seed.1,
            seed_conditions: seed.0,
            seed_densities: seed.1,
        }),
    }
}

/// Read an authored parameter block into the customization seed —
/// the same distillation `event_params` performs (selector 0-3,
/// density 0..=1), `None` when a value sits outside the authored
/// ranges so the row disables instead of guessing.
fn authored_seed(p: &mm2_formats::racedata::RaceParams) -> Option<(SessionConditions, Densities)> {
    let conditions = SessionConditions {
        time_of_day: TimeOfDay::new(u8::try_from(p.time_of_day).ok()?).ok()?,
        weather: Weather::new(u8::try_from(p.weather).ok()?).ok()?,
    };
    let densities = Densities {
        traffic: p.ambient,
        pedestrians: p.peds,
    };
    densities.validate().ok()?;
    Some((conditions, densities))
}

/// Step a 0-3 selector one place in `forward`'s direction, wrapping.
fn step4(current: u8, forward: bool) -> u8 {
    (current as i8 + if forward { 1 } else { -1 }).rem_euclid(4) as u8
}

/// The traffic-density picks the options row cycles — authored values
/// are 0..=1 fractions (`Densities::validate`), so the steps cover the
/// legal range in quarters. `current` is the authored seed, which may
/// sit between steps; the first press snaps to the nearest step in
/// `forward`'s direction, then walks them, wrapping at the ends.
fn step_density(current: f32, forward: bool) -> f32 {
    const STEPS: [f32; 5] = [0.0, 0.25, 0.5, 0.75, 1.0];
    if forward {
        STEPS
            .iter()
            .copied()
            .find(|s| *s > current + 1e-3)
            .unwrap_or(STEPS[0])
    } else {
        STEPS
            .iter()
            .rev()
            .copied()
            .find(|s| *s < current - 1e-3)
            .unwrap_or(STEPS[STEPS.len() - 1])
    }
}

/// The gate check every event row shares — a locked event names the
/// unbeaten prerequisites (`blocked_by`); no row in the availability
/// table means nothing gates it.
fn availability_reason(avail: Option<mm2_game::EventAvailability>) -> Result<(), String> {
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

/// Race time from ticks — `42.9s` under a minute, `1:05.0` above.
fn fmt_race_time(ticks: u64) -> String {
    let secs = ticks as f32 / mm2_game::RACE_TICK_HZ as f32;
    let mins = (secs / 60.0) as u64;
    if mins == 0 {
        format!("{secs:.1}s")
    } else {
        format!("{mins}:{:04.1}", secs % 60.0)
    }
}

/// The Records screen (DRV-5's first leg): the bound driver's
/// persisted per-event results. Two filter rows sit on top (city,
/// race type) cycling through `all` plus the values actually present
/// in the records; each record row shows its stored numbers and
/// re-launches the event when it still resolves. The documented
/// original's Amateur Times / Pro Times / Pro Points sort keys are
/// not on the persisted record — the screen shows what is stored
/// rather than fabricating columns (the remaining original-filter
/// audit is `docs/research/menu.md`).
fn record_rows(
    data: &mut MenuData,
    vfs: &Vfs,
    city: Option<&str>,
    table: Option<EventTableKind>,
) -> Vec<Row> {
    let Some(bound) = data.bound.clone() else {
        // The root row is disabled without a bound driver; reaching
        // this anyway gets the same honest row.
        return vec![Row {
            text: "no driver profile - records are kept per driver".into(),
            enabled: Err("no driver profile - records are kept per driver".into()),
            action: Action::Back,
        }];
    };
    if bound.progress.events.is_empty() {
        return vec![Row {
            text: "no recorded results yet - finish an event".into(),
            enabled: Err("no recorded results yet".into()),
            action: Action::Back,
        }];
    }
    let mut rows = vec![
        Row {
            text: format!("City: {}", city.unwrap_or("all")),
            enabled: Ok(()),
            action: Action::RecordsCityFilter,
        },
        Row {
            text: format!("Race type: {}", table.map(table_name).unwrap_or("all")),
            enabled: Ok(()),
            action: Action::RecordsTableFilter,
        },
    ];
    // Deterministic order — city, authored table order, stem — so a
    // record's position never depends on finish chronology.
    let mut records: Vec<&mm2_game::EventRecord> = bound
        .progress
        .events
        .iter()
        .filter(|r| city.is_none_or(|c| c == r.key.city))
        .filter(|r| table.is_none_or(|t| t == r.key.table))
        .collect();
    records.sort_by(|a, b| {
        let at = TABLE_KINDS
            .iter()
            .position(|k| *k == a.key.table)
            .unwrap_or(usize::MAX);
        let bt = TABLE_KINDS
            .iter()
            .position(|k| *k == b.key.table)
            .unwrap_or(usize::MAX);
        (&a.key.city, at, &a.key.stem).cmp(&(&b.key.city, bt, &b.key.stem))
    });
    if records.is_empty() {
        rows.push(Row {
            text: "no records match these filters".into(),
            enabled: Err("no records match these filters".into()),
            action: Action::Back,
        });
        return rows;
    }
    for record in records {
        rows.push(record_row(data, vfs, &bound, record));
    }
    rows
}

/// One persisted result as a row. The recorded numbers always show —
/// they are the screen's point; the row only enables (and thereby
/// re-launches) when the event still resolves through the live
/// catalog and clears the current gates. Gated, broken or absent
/// events keep their records visible with the reason, like Quick
/// Race's unresolvable-`last_event` leg.
fn record_row(
    data: &mut MenuData,
    vfs: &Vfs,
    bound: &PlayerProfile,
    record: &mm2_game::EventRecord,
) -> Row {
    let key = &record.key;
    let stats = format!(
        "best {} place {} x{}{}",
        record
            .best_race_ticks
            .map(fmt_race_time)
            .unwrap_or_else(|| "-".into()),
        record
            .best_place
            .map(|p| p.to_string())
            .unwrap_or_else(|| "-".into()),
        record.finishes,
        match (record.beaten_amateur, record.beaten_professional) {
            (true, true) => " [A+P]",
            (true, false) => " [A]",
            (false, true) => " [P]",
            (false, false) => "",
        },
    );
    let disabled = |reason: String| Row {
        text: format!("{} ({}) - {stats}", key.stem, key.city),
        enabled: Err(reason),
        action: Action::Back, // unreachable while disabled
    };
    if key.table == EventTableKind::CrashCourse {
        return disabled("crash course events are not loadable yet (F21)".into());
    }
    if let Err(reason) = data.city_loadable(vfs, &key.city) {
        return disabled(reason);
    }
    let availability = data.availability_of(vfs, &key.city).clone();
    let Some(event) = data
        .catalog_of(vfs, &key.city)
        .events
        .iter()
        .find(|e| e.event_ref.table == key.table && e.stem == key.stem)
        .cloned()
    else {
        return disabled(format!(
            "{} is not in the {} catalog any more",
            key.stem, key.city
        ));
    };
    let enabled = match &event.status {
        mm2_content::EventStatus::Incomplete { missing } => {
            Err(format!("incomplete: {}", missing.join(", ")))
        }
        mm2_content::EventStatus::Ready => availability_reason(availability.of(bound, key)),
    };
    Row {
        text: format!(
            "{} ({}) - {} #{} - {stats}",
            key.stem,
            key.city,
            table_name(key.table),
            event.event_ref.index,
        ),
        enabled,
        action: Action::LaunchEvent(event.event_ref),
    }
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

/// A selectable row entity — `menu_present` tags each screen row with
/// its `shell.rows` index so `menu_mouse` can map a cursor hit back
/// to a [`MenuCommand::FocusAt`]. Title/status/footer lines carry no
/// marker and are never hover targets.
#[derive(Component)]
pub struct MenuRow {
    /// Index into `MenuShell::rows`.
    pub index: usize,
}

/// Keep the shell's `active` flag honest: open exactly while the
/// session sits at `Menu`, closed everywhere else — this is what makes
/// a quit from a menu-launched session return to the menu, and it
/// refreshes the bound-profile view from the resource the session
/// systems updated. Two subtleties:
///
/// - A pending `control.restart` means the session is only *transiting*
///   `Menu` — `drive_session` re-`begin`s it on the next update.
///   Reopening mid-transit would leave the shell active over the new
///   session (menu keys fighting the session's, a menu camera spawning
///   mid-game), so a restart never reopens it.
/// - The `_` arm force-closes the shell outside `Menu` — `active` is
///   the "the menu owns the screen" claim, so it cannot outlive the
///   phase that owns it, whatever ordering produced the stray flag.
///
/// A carried [`SessionNote`] — set when a `Failed` session tears down —
/// lands on the status line, so a failed load returns to the menu with
/// the reason instead of silence (F17-AC04).
pub fn menu_watch(
    session: Res<Session>,
    control: Res<SessionControl>,
    active: Option<Res<ActiveProfile>>,
    mut note: Option<ResMut<SessionNote>>,
    mut shell: ResMut<MenuShell>,
    mut data: ResMut<MenuData>,
) {
    match session.phase() {
        SessionPhase::Menu if !control.restart => {
            if !shell.active {
                shell.reopen();
                data.bound = active.map(|a| a.profile.clone());
                if let Some(note) = note.as_mut()
                    && let Some(reason) = note.failure.take()
                {
                    shell.status = Some(format!("load failed: {reason}"));
                }
            }
        }
        _ => shell.active = false,
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
///
/// The phase check is belt-and-braces: `menu_watch` already keeps
/// `active` true only at `Menu`, but a one-frame straddle (the shell
/// closed this update while the phase still reads `Menu`... or vice
/// versa) must never let a menu `Back` reach `Exit` inside a session.
pub fn menu_input(
    keys: Res<ButtonInput<KeyCode>>,
    mut key_events: MessageReader<KeyboardInput>,
    pads: Query<&Gamepad>,
    mut shell: ResMut<MenuShell>,
    mut data: ResMut<MenuData>,
    vfs: Res<Mm2Vfs>,
    mut target: MenuTarget,
) {
    if !shell.active || !matches!(target.session.phase(), SessionPhase::Menu) {
        key_events.clear();
        shell.pending.clear();
        return;
    }
    // Device systems that ran before this one (the mouse path) queue
    // here — drain them first so all commands execute through the one
    // `apply` + effect loop.
    let mut cmds: Vec<MenuCommand> = std::mem::take(&mut shell.pending);
    let name_entry = matches!(shell.screen, Screen::NewProfile { .. });
    // The stream is drained every frame — on other screens typed text
    // is discarded so a nav key's character (WASD all carry text)
    // can't leak into a freshly opened name field. On the entry
    // screen `KeyboardInput.text` appends the OS-resolved characters
    // (layout/Shift/dead keys included) and Backspace erases — both
    // honouring held-key repeats — while Enter creates and Esc
    // cancels. The nav bindings are off there: Space is a character,
    // not Activate, and arrows/WASD move no focus.
    for ev in key_events.read() {
        if !name_entry || !ev.state.is_pressed() {
            continue;
        }
        if ev.key_code == KeyCode::Backspace {
            cmds.push(MenuCommand::Erase);
        }
        if let Some(text) = &ev.text {
            for c in text.chars().filter(|c| !c.is_control()) {
                cmds.push(MenuCommand::Type(c));
            }
        }
    }
    if name_entry {
        if keys.just_pressed(KeyCode::Enter) {
            cmds.push(MenuCommand::Activate);
        }
        if keys.just_pressed(KeyCode::Escape) {
            cmds.push(MenuCommand::Back);
        }
        if let Some(pad) = pads.iter().next() {
            if pad.just_pressed(GamepadButton::South) {
                cmds.push(MenuCommand::Activate);
            }
            if pad.just_pressed(GamepadButton::East) {
                cmds.push(MenuCommand::Back);
            }
        }
    } else {
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

/// Read-only view of the cursor and the laid-out rows `menu_mouse`
/// needs — bundled so the system stays under the argument lint.
#[derive(bevy::ecs::system::SystemParam)]
pub struct MenuPointer<'w, 's> {
    cameras: Query<'w, 's, &'static Camera, With<MenuCamera>>,
    windows: Query<'w, 's, &'static Window, With<PrimaryWindow>>,
    rows: Query<
        'w,
        's,
        (
            &'static MenuRow,
            &'static ComputedNode,
            &'static UiGlobalTransform,
        ),
    >,
}

/// Mouse navigation (F17 spec req 5): hovering a row focuses it,
/// left-click activates it, right-click backs out. Runs before
/// `menu_input` in the same chain and feeds it through
/// `shell.pending`, so clicks execute through the same
/// `apply`/`MenuEffect` path as Enter — a click on a disabled row
/// shows its reason, and a click on Quit quits.
///
/// Two deliberate semantics:
///
/// - Hover is an *edge*, not a pin: only a cursor that moved this
///   frame can refocus — a cursor resting on a row never re-asserts
///   focus, so mouse and keyboard/gamepad coexist without fighting.
/// - The hit test reads the layout the renderer produced — each
///   [`MenuRow`] entity's `ComputedNode` rect in the physical-pixel
///   space `UiGlobalTransform` maps into — the same convention
///   `bevy_ui`'s picking backend uses (logical cursor × the camera's
///   target scaling factor, then the viewport clip). Layout runs in
///   `PostUpdate`, so the test is one frame stale at worst.
pub fn menu_mouse(
    session: Res<Session>,
    mut shell: ResMut<MenuShell>,
    mouse: Option<Res<ButtonInput<MouseButton>>>,
    pointer: MenuPointer,
    mut last: Local<Option<Vec2>>,
) {
    if !shell.active || !matches!(session.phase(), SessionPhase::Menu) {
        *last = None;
        return;
    }
    let Some(window) = pointer.windows.iter().next() else {
        *last = None;
        return;
    };
    let Some(cursor) = window.cursor_position() else {
        *last = None;
        return;
    };
    // bevy_ui's picking backend maps logical pointer → physical
    // through the camera's target scaling factor, then clips to the
    // viewport — `ComputedNode` rects live in that space. The camera's
    // `computed.target_info` is a render-side product, so headless
    // runs fall back to the window's own scale factor and skip the
    // clip — with no custom viewport the whole window is the target.
    let camera = pointer.cameras.iter().next();
    let scale = camera
        .and_then(|c| c.target_scaling_factor())
        .unwrap_or_else(|| window.scale_factor());
    let mut pos = cursor * scale;
    if let Some(viewport) = camera.and_then(|c| c.physical_viewport_rect()) {
        if !viewport.as_rect().contains(pos) {
            *last = None;
            return;
        }
        pos -= viewport.min.as_vec2();
    }
    let moved = *last != Some(pos);
    *last = Some(pos);
    if mouse
        .as_ref()
        .is_some_and(|m| m.just_pressed(MouseButton::Right))
    {
        // Right-click backs out from anywhere — the mouse's Esc. Stop
        // here: falling through would also queue the hover FocusAt,
        // which applies *after* the pop and clobbers the restored
        // focus on the parent screen with a stale child row index.
        shell.pending.push(MenuCommand::Back);
        return;
    }
    let Some(index) = pointer
        .rows
        .iter()
        .filter(|(_, node, transform)| node.contains_point(**transform, pos))
        .map(|(row, _, _)| row.index)
        .min()
    else {
        return;
    };
    if mouse
        .as_ref()
        .is_some_and(|m| m.just_pressed(MouseButton::Left))
    {
        shell.pending.push(MenuCommand::FocusAt(index));
        shell.pending.push(MenuCommand::Activate);
    } else if moved && index != shell.focus {
        shell.pending.push(MenuCommand::FocusAt(index));
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
        Screen::NewProfile { .. } => "New driver".to_string(),
        Screen::Records { .. } => "Race records".to_string(),
        Screen::Customize { target, .. } => match target {
            CustomizeTarget::Cruise { city } => format!("Cruise options - {city}"),
            CustomizeTarget::Event { stem, .. } => format!("Race options - {stem}"),
        },
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

    // Each line carries the `shell.rows` index it draws, when it is
    // one — `menu_mouse` hit-tests `MenuRow` entities back to it.
    let mut lines: Vec<(String, f32, Color, Option<usize>)> = Vec::new();
    lines.push((
        screen_title(&shell.screen),
        34.0,
        Color::srgb(0.95, 0.9, 0.6),
        None,
    ));
    lines.push((String::new(), 8.0, Color::NONE, None));
    if let Screen::NewProfile { name } = &shell.screen {
        lines.push((
            format!("  Name: {name}_"),
            22.0,
            Color::srgb(1.0, 1.0, 1.0),
            None,
        ));
    }
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
        lines.push((text, 22.0, color, Some(i)));
    }
    lines.push((String::new(), 8.0, Color::NONE, None));
    if let Some(status) = &shell.status {
        lines.push((status.clone(), 18.0, Color::srgb(1.0, 0.75, 0.35), None));
    }
    let footer = if matches!(shell.screen, Screen::NewProfile { .. }) {
        "Type a name | Enter create | Esc cancel"
    } else if matches!(shell.screen, Screen::Records { .. }) {
        "Enter race again | Left/Right cycle filters | Esc back"
    } else if matches!(shell.screen, Screen::Customize { .. }) {
        "Left/Right change | Enter select | Esc back"
    } else {
        "Up/Down move | Enter select | Esc back | X delete | click works"
    };
    lines.push((footer.to_string(), 14.0, Color::srgb(0.5, 0.5, 0.55), None));

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
            for (text, size, color, row) in lines {
                let mut line = parent.spawn((
                    MenuUi,
                    Text::new(text),
                    // Full-width rows: the mouse hit box covers the
                    // whole line, not just the glyphs (a click right
                    // of a short label still picks the row).
                    Node {
                        width: Val::Percent(100.0),
                        ..default()
                    },
                    TextFont {
                        font_size: bevy::text::FontSize::Px(size),
                        ..default()
                    },
                    TextColor(color),
                ));
                if let Some(index) = row {
                    line.insert(MenuRow { index });
                }
            }
        });
}
