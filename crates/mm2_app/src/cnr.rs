//! F27-B.2 — the Bevy half of the Cops & Robbers gold rules.
//!
//! The rules live in [`mm2_game::gold::GoldMatch`]; this module is where
//! they meet the simulated cars, the same split [`crate::recovery`] and
//! [`crate::stuck`] use:
//!
//! - [`cnr_host_step`] runs on the authority only. Once per fixed step
//!   it advances the match clock, notices a participant whose car has
//!   gone, turns the *host's own* car positions into pickup
//!   [`Contact`]s and a delivery attempt, drops the gold when its
//!   carrier is wrecked or rammed, re-places gold that lies below the
//!   world floor, and publishes what happened as [`CnrEvent`]s. No
//!   position here comes from a client; the wire that carries a
//!   client's request and round (F27-B.3) is not built yet, so every
//!   participant — local, remote-driven or otherwise — is observed the
//!   same way.
//! - [`reconcile_gold_load`] makes each car's mass agree with
//!   [`GoldMatch::load_for`]. The load is *derived* every tick from the
//!   match state and applied from a recorded base
//!   ([`GoldLoadApplied`]), never added on top of itself, so it is
//!   applied once, ends with the carrying and cannot outlive the match
//!   (F27-AC04). The immutable [`mm2_vehicle::VehicleConfig`] is never
//!   touched.
//!
//! **Provenance.** What knocks gold loose is unrecovered (ledger
//! CNR-11): [`DEFAULT_DISLODGE_SEVERITY`] — a car-on-car impact at or
//! above it drops the carrier's gold — is an *enhanced policy*, not an
//! original rule, and a wall or prop hit never counts. The load's
//! `handling_scalar` is carried in [`GoldLoadApplied`] but **not**
//! applied to the physics: what the original scales with it is
//! unidentified, and applying a guess would pass for evidence. Only the
//! mass (by the documented reading of the host option labels) reaches
//! the body.
//!
//! Nothing creates a [`CnrHost`] yet — the lobby that chooses the
//! variant, sides and limits is F27-B.4 — so in the shipped app these
//! systems idle; the integration tests insert one directly.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use avian3d::prelude::*;
use bevy::prelude::*;
use mm2_assets::Vfs;
use mm2_content::cnr::{CnrContent, CnrSettings, MARKER_MODELS};
use mm2_game::gold::{CarrierLoad, Contact, DropCause, GoldError, GoldEvent, GoldMatch, Side};
use mm2_game::{
    DamageTier, ImpactEvent, ObjectId, ObjectIdentity, Player, PlayerId, Session, SessionEntity,
    VehicleDamage,
};

use crate::city::{MovableModels, WorldFloor, v3};

/// Approach speed, m/s, at or above which another participant's car
/// striking the carrier knocks the gold loose. *Enhanced policy* — the
/// original's threshold is unrecovered; chosen above the impact
/// filter's touch floor so a bump does not strip a carrier.
pub const DEFAULT_DISLODGE_SEVERITY: f32 = 8.0;

/// The host's live match: the authoritative gold state plus the little
/// the Bevy side must remember between steps. Inserted when a
/// Cops & Robbers session starts and removed at teardown
/// ([`crate::session::drive_session`]) so nothing leaks into the next
/// session.
#[derive(Resource, Debug)]
pub struct CnrHost {
    /// The authoritative rules state.
    pub game: GoldMatch,
    /// Impact approach speed that dislodges a carrier (m/s).
    pub dislodge_severity: f32,
    /// Where each participant's car was last observed — the drop
    /// position when one vanishes (a disconnect despawns its car).
    last_seen: BTreeMap<PlayerId, Vec3>,
}

impl CnrHost {
    /// A host around a freshly built match, with the default
    /// dislodge policy.
    pub fn new(game: GoldMatch) -> Self {
        Self {
            game,
            dislodge_severity: DEFAULT_DISLODGE_SEVERITY,
            last_seen: BTreeMap::new(),
        }
    }

    /// A host for a city's match: the rules the lobby's `settings`
    /// choose, played over the city's authored site pool
    /// (`multicopwaypoints.csv`, [`CnrContent::sites`]) in the world's
    /// own axes. Refuses a pool that cannot seed a round; the
    /// content's other issues (a missing marker model, a short
    /// commentary table) do not stop play, they are the audit's to
    /// count.
    pub fn from_content(
        content: &CnrContent,
        settings: &CnrSettings,
        tick_hz: u32,
        generation: u64,
        gold: ObjectId,
        seed: u64,
        participants: &[(PlayerId, Side)],
    ) -> Result<Self, GoldError> {
        let pool = content.sites.iter().copied().map(v3).collect();
        let game = GoldMatch::new(
            generation,
            gold,
            settings.rules(tick_hz),
            pool,
            seed,
            participants,
        )?;
        Ok(Self::new(game))
    }
}

/// One thing that happened to the gold, in order, for the wire, HUD and
/// commentary consumers.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct CnrEvent(pub GoldEvent);

/// What [`reconcile_gold_load`] has done to a car's body: the values it
/// started from and the load it applied. Present exactly while the load
/// is on the car.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct GoldLoadApplied {
    /// The body mass before the load, kg.
    pub base_mass: f32,
    /// The principal inertia before the load.
    pub base_inertia: Vec3,
    /// The load applied. Its `handling_scalar` is recorded, not applied
    /// (see the module docs).
    pub load: CarrierLoad,
}

/// Fixed-step authority leg: drive the gold match from the host's cars.
///
/// Order within a step matters and is the same every step: clock, a
/// vanished participant leaves, carrier destroyed/rammed drops the gold,
/// pickups resolve (a striker may take what it just knocked loose — the
/// dropper is locked out by the rules, the striker is not), the carrier
/// tries to deliver, out-of-bounds gold is re-placed, and the events
/// publish. Idle without a [`CnrHost`], while the session is not
/// `Playing`, and on a non-authority process.
pub fn cnr_host_step(
    session: Res<Session>,
    host: Option<ResMut<CnrHost>>,
    floor: Option<Res<WorldFloor>>,
    cars: Query<(&ObjectIdentity, &Player, &Position, Option<&VehicleDamage>)>,
    mut impacts: MessageReader<ImpactEvent>,
    mut out: MessageWriter<CnrEvent>,
) {
    // Impacts are read unconditionally so a pause or a missing match
    // cannot leave a stale one to strike a carrier later.
    let impacts: Vec<ImpactEvent> = impacts.read().copied().collect();
    let Some(mut host) = host else {
        return;
    };
    if !session.is_playing() || !session.authority_role().is_authority() {
        return;
    }
    let host = &mut *host;
    let game = &mut host.game;
    game.tick();

    let connected: BTreeSet<PlayerId> = game
        .standings()
        .into_iter()
        .filter(|s| s.connected)
        .map(|s| s.player)
        .collect();
    let mut at: BTreeMap<PlayerId, Vec3> = BTreeMap::new();
    let mut present: BTreeSet<PlayerId> = BTreeSet::new();
    let mut who: HashMap<ObjectId, PlayerId> = HashMap::new();
    let mut wrecked: BTreeSet<PlayerId> = BTreeSet::new();
    for (identity, player, position, damage) in &cars {
        if !connected.contains(&player.id) {
            continue;
        }
        present.insert(player.id);
        who.insert(identity.0, player.id);
        // A car with a non-finite pose is still in the match — it just
        // has no usable position this step, so it neither reaches the
        // gold nor counts as gone.
        if !position.0.is_finite() {
            continue;
        }
        at.insert(player.id, position.0);
        if damage.is_some_and(|d| d.condition() == DamageTier::Disabled) {
            wrecked.insert(player.id);
        }
    }

    // A participant seen before whose car is gone has left; the gold
    // drops where their car last was. Never-seen participants have not
    // spawned yet and are not leavers.
    for &id in &connected {
        if let Some(&p) = at.get(&id) {
            host.last_seen.insert(id, p);
        } else if !present.contains(&id)
            && let Some(last) = host.last_seen.remove(&id)
        {
            let _ = game.leave(id, last);
        }
    }

    if game.outcome().is_none() {
        // Wrecked or rammed carrier loses the gold.
        if let Some(carrier) = game.carrier()
            && let Some(&pos) = at.get(&carrier)
        {
            let cause = if wrecked.contains(&carrier) {
                Some(DropCause::Destroyed)
            } else {
                impacts
                    .iter()
                    .filter(|i| i.generation == session.generation())
                    .filter(|i| i.severity.is_finite() && i.severity >= host.dislodge_severity)
                    .find_map(|i| {
                        let a = who.get(&i.participants.0).copied();
                        let b = who.get(&i.participants.1).copied();
                        match (a, b) {
                            (Some(x), Some(y)) if x == carrier && y != carrier => Some(y),
                            (Some(x), Some(y)) if y == carrier && x != carrier => Some(x),
                            _ => None,
                        }
                    })
                    .map(|by| DropCause::Knocked { by: Some(by) })
            };
            if let Some(cause) = cause {
                let _ = game.dislodge(carrier, pos, cause);
            }
        }

        // Pickups: every connected car in reach asks; the rules decide.
        if let Some(gold_at) = game.gold_position() {
            let reach = game.rules().pickup_radius;
            let round = game.round();
            let contacts: Vec<Contact> = connected
                .iter()
                .filter_map(|id| {
                    // A wreck takes nothing: once its lockout ran out it
                    // would otherwise re-grant (and re-score) forever.
                    if wrecked.contains(id) {
                        return None;
                    }
                    let position = *at.get(id)?;
                    (position.distance(gold_at) <= reach).then_some(Contact {
                        player: *id,
                        round,
                        position,
                    })
                })
                .collect();
            if !contacts.is_empty() {
                game.resolve_pickups(&contacts);
            }
        }

        // Delivery, after pickup so a grab and a delivery in one step
        // are both honoured in that order.
        if let Some(carrier) = game.carrier()
            && !wrecked.contains(&carrier)
            && let Some(&pos) = at.get(&carrier)
        {
            game.deliver(carrier, game.round(), pos);
        }

        // Gold that lies below the world floor is re-placed so the
        // objective is never lost for good.
        if let Some(floor) = floor
            && let Some(g) = game.gold_position()
            && (!g.y.is_finite() || g.y < floor.0)
        {
            let _ = game.gold_out_of_bounds();
        }
    }

    for event in game.drain_events() {
        out.write(CnrEvent(event));
    }
}

/// Fixed-step: make each car's body agree with the match's load. The
/// target is [`GoldMatch::load_for`] (nothing at all once the
/// [`CnrHost`] is gone); the body is always written from the recorded
/// base, so repeated steps neither stack the load nor drift the mass.
pub fn reconcile_gold_load(
    mut commands: Commands,
    host: Option<Res<CnrHost>>,
    mut cars: Query<(
        Entity,
        &Player,
        &mut Mass,
        &mut AngularInertia,
        Option<&GoldLoadApplied>,
    )>,
) {
    for (entity, player, mut mass, mut inertia, applied) in &mut cars {
        let want = host
            .as_ref()
            .and_then(|h| h.game.load_for(player.id))
            .filter(|l| l.added_mass_kg.is_finite() && l.added_mass_kg > 0.0);
        match (want, applied) {
            (Some(load), None) => {
                let base = mass.0;
                if !(base.is_finite() && base > 0.0) {
                    continue;
                }
                let record = GoldLoadApplied {
                    base_mass: base,
                    base_inertia: inertia.principal,
                    load,
                };
                apply(&record, &mut mass, &mut inertia);
                commands.entity(entity).insert(record);
            }
            (Some(load), Some(applied)) if applied.load != load => {
                let record = GoldLoadApplied { load, ..*applied };
                apply(&record, &mut mass, &mut inertia);
                commands.entity(entity).insert(record);
            }
            (None, Some(applied)) => {
                mass.0 = applied.base_mass;
                inertia.principal = applied.base_inertia;
                commands.entity(entity).remove::<GoldLoadApplied>();
            }
            _ => {}
        }
    }
}

/// Write `base + load` onto the body: the mass grows by the load and the
/// inertia tensor scales with it, as a heavier body of the same shape.
fn apply(record: &GoldLoadApplied, mass: &mut Mass, inertia: &mut AngularInertia) {
    let total = record.base_mass + record.load.added_mass_kg;
    mass.0 = total;
    inertia.principal = record.base_inertia * (total / record.base_mass);
}

/// What a marker stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkerRole {
    /// The gold, wherever it lies.
    Gold,
    /// The hideout a robber delivers to.
    Hideout,
    /// The bank a cop delivers to.
    Bank,
}

impl MarkerRole {
    /// Every role, in [`MARKER_MODELS`] order.
    pub const ALL: [MarkerRole; 3] = [MarkerRole::Gold, MarkerRole::Hideout, MarkerRole::Bank];

    /// The retail marker model (`geometry/<name>.pkg`) the mode's setup
    /// binds to this role (`wpobj_gold`, `pt_hideout`, `pt_bank`).
    pub fn model(self) -> &'static str {
        match self {
            MarkerRole::Gold => MARKER_MODELS[0],
            MarkerRole::Hideout => MARKER_MODELS[1],
            MarkerRole::Bank => MARKER_MODELS[2],
        }
    }
}

/// A rendered marker for one of the match's three sites, positioned by
/// [`sync_cnr_markers`] from the host's match. Pure presentation: it
/// has no collider and no authority.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct CnrMarker {
    /// What the marker stands for.
    pub role: MarkerRole,
}

/// What [`spawn_cnr_markers`] produced, every miss counted.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CnrMarkerReport {
    /// Marker entities spawned.
    pub spawned: usize,
    /// Roles whose model resolved to nothing drawable — that marker is
    /// absent from the world, not substituted.
    pub missing_models: Vec<&'static str>,
    /// Texture stems the models named that failed to resolve.
    pub missing_textures: Vec<String>,
}

/// Spawn the gold, hideout and bank markers from the retail marker
/// models through the same PKG→mesh path stamped props use. Each is a
/// session-owned root (the render parts are children), placed at the
/// match's current sites; [`sync_cnr_markers`] keeps them there. The
/// models' collision is deliberately not built — a marker a car could
/// strike would be a prop, and the original's marker banger records
/// are not read here.
pub fn spawn_cnr_markers(
    commands: &mut Commands,
    vfs: &Vfs,
    host: &CnrHost,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
    owner: SessionEntity,
) -> CnrMarkerReport {
    let mut report = CnrMarkerReport::default();
    let mut models = MovableModels::new(vfs, meshes, images, materials);
    let sites = host.game.sites();
    for role in MarkerRole::ALL {
        let Some(model) = models.load(role.model(), Vec3::ZERO) else {
            report.missing_models.push(role.model());
            continue;
        };
        let at = match role {
            MarkerRole::Gold => host.game.gold_position().unwrap_or(sites.gold),
            MarkerRole::Hideout => sites.hideout,
            MarkerRole::Bank => sites.bank,
        };
        let root = commands
            .spawn((
                owner,
                CnrMarker { role },
                Transform::from_translation(at),
                Visibility::default(),
                Name::new(format!("cnr-{}", role.model())),
            ))
            .id();
        for (mesh, material) in &model.parts {
            let part = commands
                .spawn((
                    Mesh3d(mesh.clone()),
                    MeshMaterial3d(material.clone()),
                    Transform::IDENTITY,
                ))
                .id();
            commands.entity(root).add_child(part);
        }
        report.spawned += 1;
    }
    report.missing_textures = models.finish(commands, owner).into_iter().collect();
    report
}

/// Keep the markers where the host's match says they are: hideout and
/// bank at the round's drawn sites (they move when a delivery draws new
/// ones), the gold where it rests or was dropped. While a car carries
/// the gold its marker is hidden — *implementation choice*: what the
/// original draws on a carrier is unrecovered, and the carrier/score
/// presentation is the HUD's (F27-B.4), not a marker pinned to a body.
/// Without a [`CnrHost`] the markers keep their last pose.
pub fn sync_cnr_markers(
    host: Option<Res<CnrHost>>,
    mut markers: Query<(&CnrMarker, &mut Transform, &mut Visibility)>,
) {
    let Some(host) = host else {
        return;
    };
    let sites = host.game.sites();
    let gold = host.game.gold_position();
    for (marker, mut transform, mut visibility) in &mut markers {
        let (at, shown) = match marker.role {
            MarkerRole::Gold => (gold.unwrap_or(sites.gold), gold.is_some()),
            MarkerRole::Hideout => (sites.hideout, true),
            MarkerRole::Bank => (sites.bank, true),
        };
        if transform.translation != at {
            transform.translation = at;
        }
        let want = if shown {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *visibility != want {
            *visibility = want;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use mm2_game::gold::{CnrVariant, EndRule, GoldRules, Side};
    use mm2_game::{PlayerControl, SessionConfig, SessionPhase};

    const A: PlayerId = PlayerId(1);
    const B: PlayerId = PlayerId(2);
    const MASS: f32 = 1300.0;
    const INERTIA: Vec3 = Vec3::new(900.0, 1500.0, 1200.0);

    fn rules(end: EndRule) -> GoldRules {
        GoldRules {
            variant: CnrVariant::FreeForAll,
            end,
            load: CarrierLoad {
                added_mass_kg: 250.0,
                handling_scalar: 0.9,
            },
            pickup_points: 25,
            delivery_points: 100,
            pickup_radius: 5.0,
            delivery_radius: 12.0,
            drop_lockout_ticks: 120,
        }
    }

    fn pool() -> Vec<Vec3> {
        (0..8)
            .map(|i| Vec3::new(i as f32 * 100.0, 0.0, (i * i) as f32 * 7.0))
            .collect()
    }

    struct Rig {
        app: App,
        a: Entity,
        b: Entity,
        a_obj: ObjectId,
        b_obj: ObjectId,
    }

    fn rig(end: EndRule, players: &[PlayerId]) -> Rig {
        let mut session = Session::new();
        session.begin(SessionConfig::default()).unwrap();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Playing).unwrap();
        let gold = session.mint_object_id();
        let a_obj = session.mint_object_id();
        let b_obj = session.mint_object_id();
        let participants: Vec<(PlayerId, Side)> =
            players.iter().map(|&p| (p, Side::Solo)).collect();
        let game = GoldMatch::new(
            session.generation(),
            gold,
            rules(end),
            pool(),
            5,
            &participants,
        )
        .unwrap();
        let mut app = App::new();
        app.insert_resource(session)
            .insert_resource(CnrHost::new(game))
            .add_message::<ImpactEvent>()
            .add_message::<CnrEvent>()
            .add_systems(Update, (cnr_host_step, reconcile_gold_load).chain());
        let far = Vec3::new(5000.0, 0.0, 5000.0);
        let spawn = |app: &mut App, id: PlayerId, obj: ObjectId, at: Vec3| {
            app.world_mut()
                .spawn((
                    ObjectIdentity(obj),
                    Player {
                        id,
                        control: PlayerControl::Local,
                    },
                    Position(at),
                    Mass(MASS),
                    AngularInertia {
                        principal: INERTIA,
                        local_frame: Quat::IDENTITY,
                    },
                ))
                .id()
        };
        let a = spawn(&mut app, A, a_obj, far);
        let b = spawn(&mut app, B, b_obj, far + Vec3::X * 1000.0);
        Rig {
            app,
            a,
            b,
            a_obj,
            b_obj,
        }
    }

    impl Rig {
        fn gold_at(&self) -> Vec3 {
            self.app
                .world()
                .resource::<CnrHost>()
                .game
                .gold_position()
                .unwrap()
        }
        fn game(&self) -> &GoldMatch {
            &self.app.world().resource::<CnrHost>().game
        }
        fn put(&mut self, e: Entity, p: Vec3) {
            self.app.world_mut().get_mut::<Position>(e).unwrap().0 = p;
        }
        fn mass(&self, e: Entity) -> f32 {
            self.app.world().get::<Mass>(e).unwrap().0
        }
        fn step(&mut self) {
            self.app.update();
        }
        fn events(&mut self) -> Vec<GoldEvent> {
            self.app
                .world_mut()
                .resource_mut::<Messages<CnrEvent>>()
                .drain()
                .map(|e| e.0)
                .collect()
        }
        fn impact(&mut self, a: ObjectId, b: ObjectId, severity: f32) {
            let generation = self.app.world().resource::<Session>().generation();
            self.app
                .world_mut()
                .resource_mut::<Messages<ImpactEvent>>()
                .write(ImpactEvent {
                    id: mm2_game::ImpactId(1),
                    generation,
                    tick: 0,
                    participants: (a, b),
                    point: Vec3::ZERO,
                    normal: Vec3::Y,
                    severity,
                    surface: mm2_game::SurfaceState::default(),
                });
        }
    }

    #[test]
    fn a_car_in_reach_takes_the_gold_and_the_load_follows_it() {
        let mut r = rig(EndRule::None, &[A, B]);
        r.step();
        assert_eq!(r.game().carrier(), None);
        assert_eq!(r.mass(r.a), MASS);
        let g = r.gold_at();
        r.put(r.a, g + Vec3::X);
        r.step();
        assert_eq!(r.game().carrier(), Some(A));
        assert_eq!(r.mass(r.a), MASS + 250.0);
        assert_eq!(r.mass(r.b), MASS, "only the carrier is loaded");
        let ev = r.events();
        assert!(matches!(
            ev.as_slice(),
            [GoldEvent::Picked {
                player: A,
                recovered: false,
                ..
            }]
        ));
    }

    #[test]
    fn the_load_is_applied_once_however_many_steps_pass() {
        let mut r = rig(EndRule::None, &[A, B]);
        let g = r.gold_at();
        r.put(r.a, g);
        for _ in 0..50 {
            r.step();
        }
        assert_eq!(r.mass(r.a), MASS + 250.0);
        let applied = *r.app.world().get::<GoldLoadApplied>(r.a).unwrap();
        assert_eq!(applied.base_mass, MASS);
        let inertia = r.app.world().get::<AngularInertia>(r.a).unwrap().principal;
        let scale = (MASS + 250.0) / MASS;
        assert!((inertia - INERTIA * scale).abs().max_element() < 1e-3);
    }

    #[test]
    fn the_load_ends_exactly_with_the_carrying() {
        let mut r = rig(EndRule::None, &[A, B]);
        let g = r.gold_at();
        r.put(r.a, g);
        r.step();
        assert_eq!(r.game().carrier(), Some(A));
        // B rams A hard enough to knock the gold loose.
        let (ao, bo) = (r.a_obj, r.b_obj);
        r.impact(bo, ao, DEFAULT_DISLODGE_SEVERITY + 1.0);
        r.step();
        assert_eq!(r.game().carrier(), None);
        assert_eq!(r.mass(r.a), MASS, "back to the exact base mass");
        assert_eq!(
            r.app.world().get::<AngularInertia>(r.a).unwrap().principal,
            INERTIA
        );
        assert!(r.app.world().get::<GoldLoadApplied>(r.a).is_none());
    }

    #[test]
    fn a_soft_or_world_hit_does_not_knock_the_gold_loose() {
        let mut r = rig(EndRule::None, &[A, B]);
        let g = r.gold_at();
        r.put(r.a, g);
        r.step();
        let (ao, bo) = (r.a_obj, r.b_obj);
        r.impact(bo, ao, DEFAULT_DISLODGE_SEVERITY - 0.5);
        r.impact(ObjectId::WORLD, ao, 100.0);
        r.impact(ao, ao, 100.0);
        r.step();
        assert_eq!(r.game().carrier(), Some(A));
    }

    #[test]
    fn two_cars_in_reach_yield_one_carrier_the_nearer() {
        let mut r = rig(EndRule::None, &[A, B]);
        let g = r.gold_at();
        r.put(r.a, g + Vec3::X * 3.0);
        r.put(r.b, g + Vec3::Z * 1.0);
        r.step();
        assert_eq!(r.game().carrier(), Some(B));
        let picked = r
            .events()
            .into_iter()
            .filter(|e| matches!(e, GoldEvent::Picked { .. }))
            .count();
        assert_eq!(picked, 1);
        assert_eq!(r.mass(r.a), MASS);
        assert_eq!(r.mass(r.b), MASS + 250.0);
    }

    #[test]
    fn a_rammed_carrier_drops_the_gold_and_the_striker_can_recover_it() {
        let mut r = rig(EndRule::None, &[A, B]);
        let g = r.gold_at();
        r.put(r.a, g);
        r.step();
        // B arrives beside A and rams it.
        r.put(r.b, g + Vec3::X * 2.0);
        let (ao, bo) = (r.a_obj, r.b_obj);
        r.impact(bo, ao, 20.0);
        r.step();
        // The dropper A is locked out; B, in reach, recovers it in the
        // same step.
        assert_eq!(r.game().carrier(), Some(B));
        let ev = r.events();
        assert!(ev.iter().any(|e| matches!(
            e,
            GoldEvent::Dropped {
                player: A,
                cause: DropCause::Knocked { by: Some(B) },
                ..
            }
        )));
        assert!(ev.iter().any(|e| matches!(
            e,
            GoldEvent::Picked {
                player: B,
                recovered: true,
                ..
            }
        )));
        assert_eq!(r.mass(r.a), MASS);
        assert_eq!(r.mass(r.b), MASS + 250.0);
    }

    #[test]
    fn a_disabled_carrier_drops_the_gold_and_cannot_retake_it_at_once() {
        let mut r = rig(EndRule::None, &[A, B]);
        let g = r.gold_at();
        r.put(r.a, g);
        r.step();
        let mut damage = VehicleDamage::new(mm2_game::DamageSpec {
            impact_threshold: 1500.0,
            med_damage: 150_000.0,
            max_damage: 321_300.0,
            regenerate_rate: 0.0,
        });
        damage.apply(mm2_game::ImpactId(1), 400_000.0);
        assert_eq!(damage.condition(), DamageTier::Disabled);
        r.app.world_mut().entity_mut(r.a).insert(damage);
        r.step();
        assert_eq!(r.game().carrier(), None);
        assert!(r.events().iter().any(|e| matches!(
            e,
            GoldEvent::Dropped {
                player: A,
                cause: DropCause::Destroyed,
                ..
            }
        )));
        assert_eq!(r.mass(r.a), MASS);
        // Still sitting on the gold, still wrecked: locked out.
        r.step();
        assert_eq!(r.game().carrier(), None);
    }

    #[test]
    fn a_wreck_never_retakes_the_gold_after_the_lockout_ends() {
        let mut r = rig(EndRule::None, &[A, B]);
        let g = r.gold_at();
        r.put(r.a, g);
        r.step();
        let mut damage = VehicleDamage::new(mm2_game::DamageSpec {
            impact_threshold: 1500.0,
            med_damage: 150_000.0,
            max_damage: 321_300.0,
            regenerate_rate: 0.0,
        });
        damage.apply(mm2_game::ImpactId(1), 400_000.0);
        r.app.world_mut().entity_mut(r.a).insert(damage);
        r.step();
        r.events();
        let score = |r: &Rig| {
            r.game()
                .standings()
                .into_iter()
                .find(|s| s.player == A)
                .unwrap()
                .score
        };
        let before = score(&r);
        let lockout = r.game().rules().drop_lockout_ticks;
        for _ in 0..(lockout * 3) {
            r.step();
        }
        assert_eq!(r.game().carrier(), None);
        assert_eq!(score(&r), before);
        assert!(r.events().is_empty(), "no Picked/Dropped loop");
        assert_eq!(r.mass(r.a), MASS);
    }

    #[test]
    fn a_carrier_that_disappears_leaves_and_the_gold_drops_where_it_was() {
        let mut r = rig(EndRule::None, &[A, B]);
        let g = r.gold_at();
        r.put(r.a, g);
        r.step();
        r.put(r.a, g + Vec3::new(30.0, 0.0, 0.0));
        r.step();
        r.app.world_mut().despawn(r.a);
        r.step();
        assert_eq!(r.game().carrier(), None);
        assert_eq!(
            r.game().gold_position(),
            Some(g + Vec3::new(30.0, 0.0, 0.0))
        );
        assert!(
            r.events()
                .iter()
                .any(|e| matches!(e, GoldEvent::Left { player: A }))
        );
        let standing = r
            .game()
            .standings()
            .into_iter()
            .find(|s| s.player == A)
            .unwrap();
        assert!(!standing.connected);
        assert_eq!(standing.score, 25, "a leaver's points stay");
    }

    #[test]
    fn a_participant_not_yet_spawned_is_not_a_leaver() {
        let mut r = rig(EndRule::None, &[A, B]);
        r.app.world_mut().despawn(r.b);
        r.step();
        r.step();
        assert!(r.game().standings().iter().all(|s| s.connected));
    }

    #[test]
    fn delivery_scores_once_and_the_load_ends() {
        let mut r = rig(EndRule::None, &[A, B]);
        let g = r.gold_at();
        r.put(r.a, g);
        r.step();
        let hideout = r.game().sites().hideout;
        r.put(r.a, hideout);
        r.step();
        r.step();
        assert_eq!(r.game().carrier(), None);
        assert_eq!(r.game().score(A), Some(125));
        assert_eq!(r.mass(r.a), MASS);
        assert_eq!(r.game().round(), 1);
        let delivered = r
            .events()
            .into_iter()
            .filter(|e| matches!(e, GoldEvent::Delivered { .. }))
            .count();
        assert_eq!(delivered, 1);
    }

    #[test]
    fn gold_below_the_floor_is_replaced_not_lost() {
        let mut r = rig(EndRule::None, &[A, B]);
        // Every pool site is at y = 0; a floor above it puts the gold
        // out of bounds.
        r.app.insert_resource(WorldFloor(10.0));
        r.step();
        assert_eq!(r.game().round(), 1);
        assert!(r.game().gold_position().is_some());
        assert!(
            r.events()
                .iter()
                .any(|e| matches!(e, GoldEvent::Lost { .. }))
        );
    }

    #[test]
    fn a_pause_freezes_the_match() {
        let mut r = rig(EndRule::Ticks(2), &[A, B]);
        r.app
            .world_mut()
            .resource_mut::<Session>()
            .transition(SessionPhase::Paused)
            .unwrap();
        for _ in 0..5 {
            r.step();
        }
        assert_eq!(
            r.game().elapsed_ticks(),
            0,
            "a pause freezes the match clock"
        );
        assert!(r.events().is_empty());
    }

    #[test]
    fn the_match_clock_ends_the_match_and_publishes_it() {
        let mut r = rig(EndRule::Ticks(3), &[A, B]);
        let mut ended = 0;
        for _ in 0..6 {
            r.step();
            ended += r
                .events()
                .into_iter()
                .filter(|e| matches!(e, GoldEvent::Ended(_)))
                .count();
        }
        assert!(r.game().outcome().is_some());
        assert_eq!(ended, 1);
    }

    #[test]
    fn removing_the_host_strips_every_load() {
        let mut r = rig(EndRule::None, &[A, B]);
        let g = r.gold_at();
        r.put(r.a, g);
        r.step();
        assert_eq!(r.mass(r.a), MASS + 250.0);
        r.app.world_mut().remove_resource::<CnrHost>();
        r.step();
        assert_eq!(r.mass(r.a), MASS, "no load survives the match");
        assert!(r.app.world().get::<GoldLoadApplied>(r.a).is_none());
    }

    #[test]
    fn a_non_finite_car_position_is_ignored_and_does_not_eject_the_player() {
        let mut r = rig(EndRule::None, &[A, B]);
        let g = r.gold_at();
        r.put(r.a, g);
        r.step();
        assert_eq!(r.game().carrier(), Some(A));
        r.put(r.a, Vec3::new(f32::NAN, 0.0, 0.0));
        r.step();
        assert_eq!(
            r.game().carrier(),
            Some(A),
            "still carrying, still a member"
        );
        assert!(r.game().standings().iter().all(|s| s.connected));
    }

    fn content(sites: Vec<[f32; 3]>) -> CnrContent {
        CnrContent {
            city: "sf".into(),
            sites,
            dependencies: Vec::new(),
            cues_present: 0,
            issues: Vec::new(),
        }
    }

    #[test]
    fn a_host_plays_over_the_authored_pool_under_the_chosen_settings() {
        let sites: Vec<[f32; 3]> = (0..5)
            .map(|i| [i as f32 * 40.0, 1.0, -(i as f32)])
            .collect();
        let mut session = Session::new();
        session.begin(SessionConfig::default()).unwrap();
        let gold = session.mint_object_id();
        let settings = CnrSettings {
            gold_mass: mm2_content::cnr::GoldMass::Weightless,
            ..CnrSettings::default()
        };
        let host = CnrHost::from_content(
            &content(sites.clone()),
            &settings,
            60,
            session.generation(),
            gold,
            3,
            &[(A, Side::Solo)],
        )
        .unwrap();
        let drawn = host.game.sites();
        for p in [drawn.gold, drawn.hideout, drawn.bank] {
            assert!(
                sites.iter().any(|s| Vec3::from(*s) == p),
                "{p} is not an authored site"
            );
        }
        assert_eq!(host.game.rules().variant, settings.variant);
        assert_eq!(
            host.game.rules().pickup_radius,
            settings.rules(60).pickup_radius
        );
    }

    #[test]
    fn a_pool_too_short_for_a_round_is_refused() {
        let mut session = Session::new();
        session.begin(SessionConfig::default()).unwrap();
        let gold = session.mint_object_id();
        let err = CnrHost::from_content(
            &content(vec![[0.0; 3], [1.0; 3]]),
            &CnrSettings::default(),
            60,
            session.generation(),
            gold,
            3,
            &[],
        )
        .unwrap_err();
        assert_eq!(err, GoldError::PoolTooSmall(2));
    }

    /// One triangle, enough for the PKG→mesh path to build a part.
    fn marker_pkg() -> Vec<u8> {
        let verts: [[f32; 3]; 3] = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 2.0, 0.0]];
        let indices: [u16; 3] = [0, 1, 2];
        let mut geo = Vec::new();
        geo.extend_from_slice(&1u32.to_le_bytes()); // n_sections
        geo.extend_from_slice(&(verts.len() as u32).to_le_bytes());
        geo.extend_from_slice(&(indices.len() as u32).to_le_bytes());
        geo.extend_from_slice(&0u32.to_le_bytes()); // sections_duplicate
        geo.extend_from_slice(&0x002u32.to_le_bytes()); // FVF_XYZ
        geo.extend_from_slice(&1u16.to_le_bytes()); // n_strips
        geo.extend_from_slice(&0u16.to_le_bytes()); // flags
        geo.extend_from_slice(&(-1i32).to_le_bytes()); // shader_offset: fallback
        geo.extend_from_slice(&3i32.to_le_bytes()); // triangles
        geo.extend_from_slice(&(verts.len() as u32).to_le_bytes());
        for v in &verts {
            for f in v {
                geo.extend_from_slice(&f.to_le_bytes());
            }
        }
        geo.extend_from_slice(&(indices.len() as u32).to_le_bytes());
        for i in &indices {
            geo.extend_from_slice(&i.to_le_bytes());
        }
        let mut pkg = Vec::new();
        pkg.extend_from_slice(b"PKG3");
        pkg.extend_from_slice(b"FILE");
        pkg.push(2);
        pkg.extend_from_slice(b"H\0");
        pkg.extend_from_slice(&(geo.len() as u32).to_le_bytes());
        pkg.extend_from_slice(&geo);
        pkg
    }

    fn spawn_markers(r: &mut Rig, models: &[&str]) -> CnrMarkerReport {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("geometry")).unwrap();
        for m in models {
            std::fs::write(dir.path().join(format!("geometry/{m}.pkg")), marker_pkg()).unwrap();
        }
        let mut vfs = Vfs::new();
        vfs.mount_dir(dir.path(), 0).unwrap();
        let world = r.app.world_mut();
        world.init_resource::<Assets<Mesh>>();
        world.init_resource::<Assets<Image>>();
        world.init_resource::<Assets<StandardMaterial>>();
        world
            .run_system_once(
                move |mut commands: Commands,
                      host: Res<CnrHost>,
                      mut meshes: ResMut<Assets<Mesh>>,
                      mut images: ResMut<Assets<Image>>,
                      mut materials: ResMut<Assets<StandardMaterial>>| {
                    spawn_cnr_markers(
                        &mut commands,
                        &vfs,
                        &host,
                        &mut meshes,
                        &mut images,
                        &mut materials,
                        SessionEntity(1),
                    )
                },
            )
            .unwrap()
    }

    fn marker_pose(r: &mut Rig, role: MarkerRole) -> (Vec3, Visibility) {
        let mut q = r
            .app
            .world_mut()
            .query::<(&CnrMarker, &Transform, &Visibility)>();
        let (_, t, v) = q
            .iter(r.app.world())
            .find(|(m, ..)| m.role == role)
            .expect("marker present");
        (t.translation, *v)
    }

    #[test]
    fn markers_load_from_the_retail_model_names_and_a_missing_one_is_counted() {
        let mut r = rig(EndRule::None, &[A]);
        r.app.add_systems(Update, sync_cnr_markers);
        let report = spawn_markers(&mut r, &["wpobj_gold", "pt_hideout"]);
        assert_eq!(report.spawned, 2);
        assert_eq!(report.missing_models, vec!["pt_bank"]);
        let sites = r.game().sites();
        assert_eq!(marker_pose(&mut r, MarkerRole::Gold).0, sites.gold);
        assert_eq!(marker_pose(&mut r, MarkerRole::Hideout).0, sites.hideout);
        let banks = r
            .app
            .world_mut()
            .query::<&CnrMarker>()
            .iter(r.app.world())
            .filter(|m| m.role == MarkerRole::Bank)
            .count();
        assert_eq!(banks, 0, "an absent model is not substituted");
        // Each root carries its render part as a child, no collider.
        let mut roots = r
            .app
            .world_mut()
            .query_filtered::<&Children, With<CnrMarker>>();
        assert!(roots.iter(r.app.world()).all(|c| c.len() == 1));
        let mut colliders = r
            .app
            .world_mut()
            .query_filtered::<Entity, (With<CnrMarker>, With<Collider>)>();
        assert_eq!(colliders.iter(r.app.world()).count(), 0);
    }

    #[test]
    fn the_gold_marker_follows_the_gold_and_hides_while_it_is_carried() {
        let mut r = rig(EndRule::None, &[A, B]);
        r.app
            .add_systems(Update, sync_cnr_markers.after(reconcile_gold_load));
        spawn_markers(&mut r, &["wpobj_gold", "pt_hideout", "pt_bank"]);
        r.step();
        let g = r.gold_at();
        let (at, vis) = marker_pose(&mut r, MarkerRole::Gold);
        assert_eq!((at, vis), (g, Visibility::Inherited));

        r.put(r.a, g);
        r.step();
        assert_eq!(r.game().carrier(), Some(A));
        let (_, vis) = marker_pose(&mut r, MarkerRole::Gold);
        assert_eq!(vis, Visibility::Hidden, "nothing lies on the ground");

        // Rammed loose by a car outside pickup reach, the gold lies
        // where the carrier was struck and its marker comes back there.
        let loose = g + Vec3::X * 2.0;
        r.put(r.a, loose);
        r.put(r.b, loose + Vec3::X * 6.0);
        let (ao, bo) = (r.a_obj, r.b_obj);
        r.impact(bo, ao, 20.0);
        r.step();
        assert_eq!(r.game().carrier(), None);
        let dropped = r.game().gold_position().expect("the gold lies dropped");
        assert_eq!(dropped, loose);
        let (at, vis) = marker_pose(&mut r, MarkerRole::Gold);
        assert_eq!((at, vis), (dropped, Visibility::Inherited));
    }

    #[test]
    fn hideout_and_bank_markers_stay_on_the_drawn_sites_and_idle_without_a_host() {
        let mut r = rig(EndRule::None, &[A]);
        r.app
            .add_systems(Update, sync_cnr_markers.after(reconcile_gold_load));
        spawn_markers(&mut r, &["wpobj_gold", "pt_hideout", "pt_bank"]);
        r.step();
        let sites = r.game().sites();
        assert_eq!(marker_pose(&mut r, MarkerRole::Hideout).0, sites.hideout);
        assert_eq!(marker_pose(&mut r, MarkerRole::Bank).0, sites.bank);
        // Teardown removes the host: the markers stay where they were
        // rather than snapping anywhere, and nothing panics.
        r.app.world_mut().remove_resource::<CnrHost>();
        r.step();
        assert_eq!(marker_pose(&mut r, MarkerRole::Bank).0, sites.bank);
    }
}
