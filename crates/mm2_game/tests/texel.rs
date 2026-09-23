//! F05-B.9 unit coverage — the recovered `fxTexelDamage::ApplyDamage`
//! mechanics (per-tri `TextelDamageRadius` vertex test + random
//! barycentric UV pick) and the designed splat blit (DSN-32 — the
//! retail `ApplyBirdPoopDamage` shape is an unrecovered binary call,
//! so these tests pin the designed contract, not original behavior).

use bevy::prelude::{Vec2, Vec3};
use mm2_game::{NavRng, TexelDamageMesh, TexelDamagePolicy, TexelDamageTri};

/// A unit tri in the XZ plane at the origin, slot 0, covering the
/// texture's left half in UV.
fn origin_tri() -> TexelDamageTri {
    TexelDamageTri {
        positions: [
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
        ],
        uvs: [
            Vec2::new(0.0, 0.0),
            Vec2::new(0.5, 0.0),
            Vec2::new(0.0, 0.5),
        ],
        slot: 0,
    }
}

/// A tri 10 m away on +X bound to a second slot.
fn far_tri() -> TexelDamageTri {
    TexelDamageTri {
        positions: [
            Vec3::new(10.0, 0.0, 0.0),
            Vec3::new(11.0, 0.0, 0.0),
            Vec3::new(10.0, 0.0, 1.0),
        ],
        uvs: [
            Vec2::new(0.5, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(0.5, 0.5),
        ],
        slot: 1,
    }
}

#[test]
fn splats_hit_only_tris_within_the_radius() {
    let mesh = TexelDamageMesh {
        tris: vec![origin_tri(), far_tri()],
    };
    let mut rng = NavRng::new(1);
    // TextelDamageRadius 0.5 — the impact sits on the origin tri; the
    // far tri's nearest vertex is 9 m away.
    let splats = mesh.splats(Vec3::new(0.2, 0.0, 0.2), 0.5, &mut rng);
    assert_eq!(splats.len(), 1, "only the near tri splats");
    assert_eq!(splats[0].slot, 0);
    // The random barycentric lands inside the tri's UV triangle.
    assert!(splats[0].uv.x >= 0.0 && splats[0].uv.y >= 0.0);
    assert!(splats[0].uv.x + splats[0].uv.y <= 0.5 + 1e-4);
}

#[test]
fn a_wide_radius_splats_every_near_tri() {
    let mesh = TexelDamageMesh {
        tris: vec![origin_tri(), far_tri()],
    };
    let mut rng = NavRng::new(2);
    // vp4x4's authored 20.0 — both tris qualify.
    let splats = mesh.splats(Vec3::ZERO, 20.0, &mut rng);
    assert_eq!(splats.len(), 2);
    assert_eq!(splats[0].slot, 0);
    assert_eq!(splats[1].slot, 1);
}

#[test]
fn splats_replay_identically_for_a_seed() {
    let mesh = TexelDamageMesh {
        tris: vec![origin_tri()],
    };
    let a = mesh.splats(Vec3::ZERO, 0.5, &mut NavRng::new(7));
    let b = mesh.splats(Vec3::ZERO, 0.5, &mut NavRng::new(7));
    assert_eq!(a, b, "the same impact replays the same splats");
}

#[test]
fn garbage_impacts_splat_nothing() {
    let mesh = TexelDamageMesh {
        tris: vec![origin_tri()],
    };
    for (point, radius) in [
        (Vec3::splat(f32::NAN), 0.5),
        (Vec3::ZERO, 0.0),
        (Vec3::ZERO, -1.0),
        (Vec3::ZERO, f32::INFINITY),
        (Vec3::ZERO, f32::NAN),
    ] {
        assert!(
            mesh.splats(point, radius, &mut NavRng::new(3)).is_empty(),
            "point {point:?} radius {radius} must splat nothing"
        );
    }
}

#[test]
fn degenerate_tris_can_still_splat_but_never_panic() {
    // A zero-area tri whose verts share a point: the vertex test is
    // distance-based, so it qualifies like any other — the random
    // barycentric still lands on its (shared) UV.
    let tri = TexelDamageTri {
        positions: [Vec3::ZERO; 3],
        uvs: [Vec2::splat(0.25); 3],
        slot: 0,
    };
    let mesh = TexelDamageMesh { tris: vec![tri] };
    let splats = mesh.splats(Vec3::new(0.1, 0.0, 0.0), 0.5, &mut NavRng::new(4));
    assert_eq!(splats.len(), 1);
    assert!((splats[0].uv - Vec2::splat(0.25)).length() < 1e-5);
}

fn chain(w: usize, h: usize, px: [u8; 4]) -> Vec<u8> {
    // Mip-major RGBA chain down to 1x1.
    let mut data = Vec::new();
    let (mut w, mut h) = (w, h);
    loop {
        for _ in 0..w * h {
            data.extend_from_slice(&px);
        }
        if w == 1 && h == 1 {
            break;
        }
        w = (w / 2).max(1);
        h = (h / 2).max(1);
    }
    data
}

#[test]
fn the_blit_stamps_a_centered_disc_from_the_damage_texture() {
    let policy = TexelDamagePolicy {
        splat_radius: 2,
        edge_probability: 0.1,
    };
    let clean = [10u8, 10, 10, 255];
    let damage = [200u8, 0, 0, 255];
    let mut dst = chain(16, 16, clean);
    let src = chain(16, 16, damage);
    let written = mm2_game::texel::splat_blit(
        &policy,
        &mut dst,
        (16, 16),
        &src,
        (16, 16),
        Vec2::splat(0.5),
        &mut NavRng::new(9),
    );
    assert!(written > 0);
    // Level 0 centre is always written (p = 1.1).
    let centre = ((8 * 16) + 8) * 4;
    assert_eq!(&dst[centre..centre + 4], &damage);
    // Pixels outside the disc stay clean.
    assert_eq!(&dst[0..4], &clean);
    // Lower mips stamped too — level 1 (8x8) starts at 16*16*4.
    let l1 = 16 * 16 * 4 + ((4 * 8) + 4) * 4;
    assert_eq!(&dst[l1..l1 + 4], &damage, "mip level 1 splatted");
}

#[test]
fn the_blit_clips_to_bounds() {
    let policy = TexelDamagePolicy {
        splat_radius: 4,
        edge_probability: 0.1,
    };
    let mut dst = chain(8, 8, [0u8; 4]);
    let src = chain(8, 8, [255u8; 4]);
    // UV off-texture near the corner — only the reachable border writes.
    let written = mm2_game::texel::splat_blit(
        &policy,
        &mut dst,
        (8, 8),
        &src,
        (8, 8),
        Vec2::new(-0.05, -0.05),
        &mut NavRng::new(5),
    );
    assert!(written > 0, "the rim still reaches the corner");
    assert_eq!(&dst[0..4], &[255u8; 4]);
    // The level-0 far corner is untouched (the last bytes are the 1x1
    // mip, which the clipped disc legitimately reaches).
    let far = ((7 * 8) + 7) * 4;
    assert_eq!(&dst[far..far + 4], &[0u8; 4]);
}

#[test]
fn the_blit_is_deterministic_per_seed() {
    let policy = TexelDamagePolicy::default();
    let mut a = chain(8, 8, [0u8; 4]);
    let mut b = chain(8, 8, [0u8; 4]);
    let src = chain(8, 8, [255u8; 4]);
    for dst in [&mut a, &mut b] {
        mm2_game::texel::splat_blit(
            &policy,
            dst,
            (8, 8),
            &src,
            (8, 8),
            Vec2::splat(0.5),
            &mut NavRng::new(11),
        );
    }
    assert_eq!(a, b, "same seed, same disc dither");
}
