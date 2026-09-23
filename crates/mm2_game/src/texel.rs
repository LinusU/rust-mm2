//! Texel damage contracts (F05-B.9) — the `fxTexelDamage` leg of the
//! authored `vehCarDamage` record.
//!
//! MM2Hook recovers `fxTexelDamage` as a `vehCarModel` member fed by
//! `vehCarDamage`'s `ImpactsTable`/`TextelDamageRadius`:
//!
//! - `Init` pairs every `<stem>_dmg` shader texture with its clean stem
//!   (the F02-C.4 binding — `MaterialCache::shader_material`), stashes
//!   the `_dmg` texture per shader slot in `DamageTextures[]`, and
//!   clones the clean texture as the car's writable render texture
//!   (`CurrentShaders`).
//! - `ApplyDamage(position, maxDist)` walks the high-LOD body's
//!   triangles: every triangle with a vertex within `maxDist` — the
//!   authored `TextelDamageRadius`, in car space — of the impact point
//!   gets one splat at a random barycentric point's UV, blitting a disc
//!   of the `_dmg` texture onto the car's clone.
//! - `Reset` re-blits the clean texture over the clone — the skin
//!   follows the damage state a repair clears.
//!
//! The triangle test and the random-barycentric pick are the recovered
//! shipping path. The splat itself — `ApplyBirdPoopDamage`, a binary
//! call — is unrecovered, so [`TexelDamagePolicy`]'s disc shape is
//! designed (DSN-32); mm2hook's debug reimplementation blits a
//! probability-dithered disc and the constants follow it. UNK-13
//! stands for the original splat and the table's fill rules.

use bevy::prelude::*;

use crate::nav::NavRng;

/// One splat an impact asks the renderer to stamp: the damage slot and
/// the UV the random barycentric pick landed on. UVs are in the mesh's
/// convention — `v` already complemented for the top-down decode — so
/// `uv * dims` is the pixel coordinate directly.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TexelSplat {
    /// Damage-slot index (`MeshGroup::shader_offset` at rig build).
    pub slot: usize,
    /// UV on the slot's texture.
    pub uv: Vec2,
}

/// A damage-tracked triangle — the recovered `TexelDamageTri`:
/// car-space positions and UVs, tagged with its damage slot.
#[derive(Debug, Clone, Copy)]
pub struct TexelDamageTri {
    /// Car-space vertex positions (the recovered `Positions` adjunct
    /// array is model-space; the part's attach offset is baked in here).
    pub positions: [Vec3; 3],
    /// Vertex UVs (the recovered `TexCoords` adjunct array).
    pub uvs: [Vec2; 3],
    /// Damage-slot index this triangle renders through.
    pub slot: usize,
}

/// The triangle soup texel damage reads — the recovered `DamageTris`
/// array. Only triangles bound to damage-capable slots are recorded
/// (the retail loop skips `DamageTextures[i] == null` the same way),
/// so every recorded tri can splat.
#[derive(Debug, Clone, Default)]
pub struct TexelDamageMesh {
    /// Tris in authored order.
    pub tris: Vec<TexelDamageTri>,
}

impl TexelDamageMesh {
    /// `fxTexelDamage::ApplyDamage` (the recovered shipping path): every
    /// tri with a vertex within `radius` of the car-space `point` earns
    /// one splat at a random barycentric UV — three `frand()` draws
    /// normalized by their sum. `rng` is the per-vehicle stream, so a
    /// recorded impact replays identically (F05 req 6).
    ///
    /// Garbage-safe: a non-finite point or a non-positive/non-finite
    /// radius splats nothing; a tri carrying a non-finite vertex can
    /// never satisfy the distance test.
    pub fn splats(&self, point: Vec3, radius: f32, rng: &mut NavRng) -> Vec<TexelSplat> {
        if !(point.is_finite() && radius.is_finite() && radius > 0.0) {
            return Vec::new();
        }
        let mut out = Vec::new();
        for tri in &self.tris {
            let near = tri.positions.iter().any(|p| p.distance(point) < radius);
            if !near {
                continue;
            }
            let r = [rng.next_f32(), rng.next_f32(), rng.next_f32()];
            let sum = r[0] + r[1] + r[2];
            if sum.partial_cmp(&f32::EPSILON) != Some(std::cmp::Ordering::Greater) {
                continue;
            }
            let uv = (r[0] * tri.uvs[0] + r[1] * tri.uvs[1] + r[2] * tri.uvs[2]) / sum;
            out.push(TexelSplat { slot: tri.slot, uv });
        }
        out
    }
}

/// Designed splat policy (DSN-32): `ApplyBirdPoopDamage` is an
/// unrecovered binary call (`0x5923C0`), so the disc shape is ours.
/// mm2hook's debug-only reimplementation stamps a 48 px radius disc
/// with a `1.0 − d/r` write probability lifted by a 0.1 edge floor;
/// the constants follow it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TexelDamagePolicy {
    /// Splat disc radius in pixels at mip level 0.
    pub splat_radius: u32,
    /// Write-probability floor added across the disc — keeps a faint
    /// rim instead of a hard cutoff.
    pub edge_probability: f32,
}

impl Default for TexelDamagePolicy {
    fn default() -> Self {
        Self {
            splat_radius: 48,
            edge_probability: 0.1,
        }
    }
}

/// `ApplyBirdPoopDamage`'s shape (designed — DSN-32): stamp a
/// probability-dithered disc of `src` texels onto `dst` centred on
/// `uv`, then repeat down the shared mip chain (radius halving per
/// level) so the lower mips stay consistent with level 0.
///
/// Both buffers are 4-byte-per-pixel texel data in mip-major level
/// order — the `decode_tex` layout — and `dims` are the level-0
/// dimensions. Iteration stops at the first level either buffer does
/// not carry. A UV outside `[0, 1]` centres off-texture; the disc is
/// clipped to each image's bounds like the retail blit, so only the
/// reachable border is written.
///
/// Returns the number of pixels written across all levels.
pub fn splat_blit(
    policy: &TexelDamagePolicy,
    dst: &mut [u8],
    dst_dims: (u32, u32),
    src: &[u8],
    src_dims: (u32, u32),
    uv: Vec2,
    rng: &mut NavRng,
) -> usize {
    if !(uv.is_finite() && dst_dims.0 > 0 && dst_dims.1 > 0 && src_dims.0 > 0 && src_dims.1 > 0) {
        return 0;
    }
    let mut written = 0usize;
    let (mut dw, mut dh, mut doff) = (dst_dims.0 as usize, dst_dims.1 as usize, 0usize);
    let (mut sw, mut sh, mut soff) = (src_dims.0 as usize, src_dims.1 as usize, 0usize);
    let mut level = 0u32;
    loop {
        let dsz = dw * dh * 4;
        let ssz = sw * sh * 4;
        if doff + dsz > dst.len() || soff + ssz > src.len() {
            break;
        }
        let radius = (policy.splat_radius >> level).max(1) as usize;
        let radius_sq = (radius * radius) as f32;
        // The retail call takes one UV for both textures — the pair is
        // authored at the same layout. With mismatched sizes the disc
        // keeps its pixel size and centres on each image's own `uv`.
        let dcx = uv.x * dw as f32;
        let dcy = uv.y * dh as f32;
        let scx = uv.x * sw as f32;
        let scy = uv.y * sh as f32;
        let r = radius as i64;
        for dy in -r..=r {
            for dx in -r..=r {
                let dist_sq = (dx * dx + dy * dy) as f32;
                if dist_sq > radius_sq {
                    continue;
                }
                // `1 − d/r + edge` — always writes the centre, thins
                // toward the rim (the recovered debug reading).
                let p = 1.0 - dist_sq.sqrt() / radius as f32 + policy.edge_probability;
                let (dxi, dyi) = ((dcx + dx as f32) as i64, (dcy + dy as f32) as i64);
                let (sxi, syi) = ((scx + dx as f32) as i64, (scy + dy as f32) as i64);
                if !(0..dw as i64).contains(&dxi)
                    || !(0..dh as i64).contains(&dyi)
                    || !(0..sw as i64).contains(&sxi)
                    || !(0..sh as i64).contains(&syi)
                {
                    continue;
                }
                if rng.next_f32() <= p {
                    let di = doff + (dyi as usize * dw + dxi as usize) * 4;
                    let si = soff + (syi as usize * sw + sxi as usize) * 4;
                    dst[di..di + 4].copy_from_slice(&src[si..si + 4]);
                    written += 1;
                }
            }
        }
        doff += dsz;
        soff += ssz;
        dw = (dw / 2).max(1);
        dh = (dh / 2).max(1);
        sw = (sw / 2).max(1);
        sh = (sh / 2).max(1);
        level += 1;
    }
    written
}
