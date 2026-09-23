//! F05-B.9: impact texel damage — the `fxTexelDamage::ApplyDamage`/
//! `Reset` leg of the authored `vehCarDamage` record, the direct
//! follow-on of F02-C.4's clean-stem binding.
//!
//! MM2Hook recovers the structure: `fxTexelDamage` is a `vehCarModel`
//! member that `Init` populates from the high-LOD body's shaders —
//! every texture paired under the `<stem>`/`<stem>_dmg` convention
//! gets a `DamageTextures[]` entry plus a cloned "current" texture the
//! car renders (`CurrentShaders`); `ApplyDamage(position, maxDist)`
//! — fed by the car's `ImpactsTable` with `TextelDamageRadius` as
//! `maxDist` — stamps a radial blit of the damage texture onto the
//! clone for every nearby triangle; `Reset` re-blits the clean
//! texture. `mm2_game::texel` owns the recovered mechanics
//! ([`TexelDamageMesh::splats`]) and the designed splat
//! ([`TexelDamagePolicy`], DSN-32 — the retail `ApplyBirdPoopDamage`
//! is an unrecovered binary call); this module owns the Bevy state:
//! per-vehicle cloned textures/materials ([`TexelDamageRig`]), the
//! impact feed ([`apply_texel_damage`]) and the repair-path reset
//! ([`reset_texels`], called from `damage::resolve_disabled`'s three
//! `damage.reset()` sites).
//!
//! Ambient traffic and trailers spawn without the rig (`None`) — their
//! `vehcardamage` records, where they exist, never bound a texel rig
//! in retail either (the ambient path is a different damage class).

use std::collections::HashMap;

use bevy::prelude::*;
use mm2_formats::veh::VehCarDamage;
use mm2_game::{
    ImpactEvent, NavRng, ObjectId, ObjectIdentity, Player, PlayerControl, Session, TexelDamageMesh,
    TexelDamagePolicy, TexelDamageTri, texel::splat_blit,
};

use crate::city::MaterialCache;

/// One shader slot's damage binding — the recovered
/// `CleanShaders`/`DamageTextures`/`CurrentShaders` triple.
#[derive(Debug, Clone)]
pub struct TexelSlot {
    /// The material this vehicle's groups bind — a per-vehicle clone
    /// of the shared shader material whose `base_color_texture` is
    /// `current`.
    pub material: Handle<StandardMaterial>,
    /// The resolved clean texture — `Reset`'s blit source. Equals
    /// `damage` when the authored `_dmg` name has no clean pair
    /// (retail keeps the authored texture on a failed clean lookup).
    pub clean: Handle<Image>,
    /// The paired `<stem>_dmg` texture — the splat source.
    pub damage: Handle<Image>,
    /// The vehicle's writable clone of `clean` — the render texture.
    pub current: Handle<Image>,
}

/// Component: a vehicle's texel-damage rig — the `fxTexelDamage`
/// equivalent. Present only when the vehicle carries an authored
/// `vehcardamage` record AND its body's shaders pair `_dmg` textures,
/// matching the authored-presence policy of `VehicleDamage`/
/// `VehicleSmoke`/`VehicleSparks`.
#[derive(Component)]
pub struct TexelDamageRig {
    /// Authored `TextelDamageRadius` — `ApplyDamage`'s `maxDist`,
    /// metres in car space.
    pub radius: f32,
    /// The damage-capable shader slots, in bind order.
    pub slots: Vec<TexelSlot>,
    /// Car-space triangle soup — damage-capable slots only.
    pub mesh: TexelDamageMesh,
    /// Splat shape — designed (DSN-32).
    pub policy: TexelDamagePolicy,
    /// Per-vehicle deterministic stream — barycentric picks and the
    /// disc dither replay identically per spawn (F05 req 6).
    rng: NavRng,
}

impl TexelDamageRig {
    pub fn new(radius: f32, seed: u64, slots: Vec<TexelSlot>, mesh: TexelDamageMesh) -> Self {
        Self {
            radius,
            slots,
            mesh,
            policy: TexelDamagePolicy::default(),
            rng: NavRng::new(seed),
        }
    }

    /// `ApplyDamage` for one `ImpactsTable` entry: splat every
    /// qualifying triangle onto its slot's `current` texture. Returns
    /// the number of splats stamped.
    pub fn apply(&mut self, point: Vec3, images: &mut Assets<Image>) -> usize {
        let splats = self.mesh.splats(point, self.radius, &mut self.rng);
        if splats.is_empty() {
            return 0;
        }
        // Slot-order iteration, not a map walk: the dither draws below
        // consume `self.rng`, so the order has to be deterministic for
        // a recorded impact to replay identically.
        let mut by_slot: Vec<Vec<Vec2>> = (0..self.slots.len()).map(|_| Vec::new()).collect();
        for splat in splats {
            if let Some(uvs) = by_slot.get_mut(splat.slot) {
                uvs.push(splat.uv);
            }
        }
        let mut stamped = 0;
        for (slot, uvs) in self.slots.iter().zip(&by_slot) {
            if uvs.is_empty() {
                continue;
            }
            let Some(src) = images
                .get(&slot.damage)
                .and_then(|img| img.data.as_ref().map(|d| (img_size(img), d.clone())))
            else {
                continue;
            };
            let dims = {
                let Some(dst) = images.get(&slot.current) else {
                    continue;
                };
                img_size(dst)
            };
            let Some(mut dst) = images.get_mut(&slot.current) else {
                continue;
            };
            let Some(dst_data) = dst.data.as_deref_mut() else {
                continue;
            };
            for uv in uvs {
                splat_blit(
                    &self.policy,
                    dst_data,
                    dims,
                    &src.1,
                    src.0,
                    *uv,
                    &mut self.rng,
                );
                stamped += 1;
            }
        }
        stamped
    }

    /// `Reset`: blit each slot's clean texture back over the car's
    /// clone — the skin follows the damage state a repair clears.
    /// Returns `false` when nothing restored (no slots or every
    /// texture missing).
    pub fn reset(&mut self, images: &mut Assets<Image>) -> bool {
        let mut restored = false;
        for slot in &self.slots {
            let Some(data) = images.get(&slot.clean).and_then(|img| img.data.clone()) else {
                continue;
            };
            if let Some(mut cur) = images.get_mut(&slot.current) {
                cur.data = Some(data);
                restored = true;
            }
        }
        restored
    }
}

fn img_size(img: &Image) -> (u32, u32) {
    (
        img.texture_descriptor.size.width,
        img.texture_descriptor.size.height,
    )
}

/// Session-scoped counters the headless record reports (`txl=`).
/// Same lifecycle contract as [`crate::damage::DamageReport`] —
/// `drive_session` resets it during teardown.
#[derive(Resource, Default, Debug)]
pub struct TexelDamageReport {
    /// Impact deliveries that stamped at least one splat.
    pub impacts: u64,
    /// Individual triangle splats written.
    pub splats: u64,
    /// Repairs that re-blitted the clean texture.
    pub resets: u64,
}

impl TexelDamageReport {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Accumulates a [`TexelDamageRig`] while `car_visual` walks the body
/// groups: `bind_group` swaps each damage-capable group's material for
/// a per-vehicle clone and records its triangles; `finish` emits the
/// rig only when at least one slot paired a `_dmg` texture.
pub struct TexelDamageBuilder {
    radius: f32,
    seed: u64,
    slots: Vec<TexelSlot>,
    by_offset: HashMap<usize, usize>,
    tris: Vec<TexelDamageTri>,
}

impl TexelDamageBuilder {
    pub fn new(damage: &VehCarDamage, seed: u64) -> Self {
        Self {
            radius: damage.textel_damage_radius,
            seed,
            slots: Vec::new(),
            by_offset: HashMap::new(),
            tris: Vec::new(),
        }
    }

    /// Bind a body mesh group through the texel path: returns the
    /// material the group renders with — the per-vehicle clone when
    /// the shader slot pairs a `_dmg` texture, the shared `base`
    /// otherwise — and records the group's triangles against the slot
    /// (`car_offset` is `attach − recenter`, the raw-vertex→car-space
    /// shift the render path bakes).
    pub fn bind_group(
        &mut self,
        mats: &mut MaterialCache<'_>,
        model: &mm2_content::model::VehicleModel,
        paint: usize,
        group: &mm2_content::model::MeshGroup,
        car_offset: Vec3,
        base: Handle<StandardMaterial>,
    ) -> Handle<StandardMaterial> {
        let slot_i = match self.by_offset.get(&group.shader_offset) {
            Some(&i) => Some(i),
            None => {
                let idx = paint * model.shaders_per_paint_job + group.shader_offset;
                let slot = model
                    .shaders
                    .get(idx)
                    .and_then(|s| mats.texel_binding(s, &base));
                slot.map(|slot| {
                    self.slots.push(slot);
                    let i = self.slots.len() - 1;
                    self.by_offset.insert(group.shader_offset, i);
                    i
                })
            }
        };
        let Some(i) = slot_i else {
            return base;
        };
        // Only the damage-capable slots' tris are recorded — the
        // retail loop's `DamageTextures[i] == null` skip, folded into
        // the build.
        for tri in group.indices.as_chunks::<3>().0 {
            let get = |i: u32| {
                Some((
                    car_offset + Vec3::from(*group.positions.get(i as usize)?),
                    Vec2::from(*group.uvs.get(i as usize)?),
                ))
            };
            let (Some((p0, uv0)), Some((p1, uv1)), Some((p2, uv2))) =
                (get(tri[0]), get(tri[1]), get(tri[2]))
            else {
                continue;
            };
            self.tris.push(TexelDamageTri {
                positions: [p0, p1, p2],
                uvs: [uv0, uv1, uv2],
                slot: i,
            });
        }
        self.slots[i].material.clone()
    }

    /// Emit the rig — `None` when no slot paired a damage texture, so
    /// undamageable-textured cars carry no component (the authored
    /// `vehcardamage` may still gate `VehicleDamage` separately).
    pub fn finish(self) -> Option<TexelDamageRig> {
        if self.slots.is_empty() {
            return None;
        }
        Some(TexelDamageRig::new(
            self.radius,
            self.seed,
            self.slots,
            TexelDamageMesh { tris: self.tris },
        ))
    }
}

/// Fixed-step: feed the deduplicated [`ImpactEvent`] stream into each
/// participant's [`TexelDamageRig`] — the `ImpactsTable`→`ApplyDamage`
/// chain. The impact's world point lands in car space through the
/// body's transform (the recovered `LastImpactPos` is car-space too).
/// Authority-gated and Remote-skipped like `apply_impact_damage`: a
/// remote participant's skin belongs to its own client.
pub fn apply_texel_damage(
    mut reader: MessageReader<ImpactEvent>,
    session: Res<Session>,
    identities: Query<(Entity, &ObjectIdentity, Option<&Player>)>,
    mut images: ResMut<Assets<Image>>,
    mut rigs: Query<(&GlobalTransform, &mut TexelDamageRig)>,
    mut report: ResMut<TexelDamageReport>,
) {
    if !session.is_playing() || !session.authority_role().is_authority() {
        reader.read().for_each(drop);
        return;
    }
    let generation = session.generation();
    let index: HashMap<ObjectId, (Entity, Option<PlayerControl>)> = identities
        .iter()
        .map(|(entity, id, player)| (id.0, (entity, player.map(|p| p.control))))
        .collect();
    for event in reader.read() {
        if event.generation != generation {
            continue;
        }
        for object in [event.participants.0, event.participants.1] {
            let Some(&(entity, control)) = index.get(&object) else {
                continue;
            };
            if control == Some(PlayerControl::Remote) {
                continue;
            }
            let Ok((xf, mut rig)) = rigs.get_mut(entity) else {
                continue;
            };
            let local = xf.to_matrix().inverse().transform_point3(event.point);
            let stamped = rig.apply(local, &mut images);
            if stamped > 0 {
                report.impacts += 1;
                report.splats += stamped as u64;
            }
        }
    }
}

/// The repair-side handles `resolve_disabled` threads — bundled so the
/// system stays under the fn-system param limit. `images` is optional:
/// headless test rigs may run the damage pipeline without the asset
/// store, in which case the reset is a no-op.
#[derive(bevy::ecs::system::SystemParam)]
pub struct TexelRepair<'w, 's> {
    rigs: Query<'w, 's, &'static mut TexelDamageRig>,
    images: Option<ResMut<'w, Assets<Image>>>,
    report: ResMut<'w, TexelDamageReport>,
}

impl TexelRepair<'_, '_> {
    /// `fxTexelDamage::Reset` at a repair site — `resolve_disabled`
    /// calls this next to each `damage.reset()`/`restore_rig` pair so
    /// the skin clears with the mechanical state.
    pub fn reset(&mut self, entity: Entity) {
        let Some(images) = self.images.as_deref_mut() else {
            return;
        };
        let Ok(mut rig) = self.rigs.get_mut(entity) else {
            return;
        };
        if rig.reset(images) {
            self.report.resets += 1;
        }
    }
}
