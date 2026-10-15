//! #1450 original-content validation: the bound of a London bridge
//! approach (`wl_lambethbrdge_rt_l`, `cl_waterloobrdge_rt_l`, tower
//! bridge) puts nothing above the closed drawbridge leaf's deck.
//!
//! Evidence level: original-content validation of the loaded London
//! collision plus the spawned leaves (downward rays). It does not judge
//! how a car drives over a leaf that is rising: a timed leaf is a ramp
//! while it opens (`docs/research/placement.md` § Bridge ends).
//!
//! Opt-in (`MM2_RETAIL=<dir>`); skipped without the operator's install,
//! and says so.

use avian3d::prelude::*;
use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
use mm2_assets::Vfs;
use mm2_game::SessionEntity;

use crate::golden_gate::{VfsRes, retail_city_app};

/// What a car's wheels and hull can meet above the deck: a collider
/// rising less than this over it is a kerb, not an obstacle.
const KERB: f32 = 0.2;
/// Headroom a car needs; collision above this is a gantry, not a wall.
const CLEARANCE: f32 = 3.0;
/// Lateral half-width sampled either side of the leaf's axis.
const HALF_WIDTH: f32 = 9.0;
/// Reach sampled beyond each hinge, onto the approach.
const APPROACH: f32 = 6.0;

#[test]
fn no_bound_stands_proud_of_a_closed_drawbridge_deck() {
    let Some(retail) = std::env::var_os("MM2_RETAIL").map(std::path::PathBuf::from) else {
        eprintln!("skipped: MM2_RETAIL is not set; the London bridge ends were NOT checked");
        return;
    };
    let mut vfs = Vfs::new();
    mm2_assets::mount_install(&mut vfs, &retail, &mm2_assets::InstallMount::default()).unwrap();
    let mut app = retail_city_app(vfs, "city/london.psdl");
    app.world_mut()
        .run_system_once(
            |mut commands: Commands,
             vfs: Res<VfsRes>,
             mut meshes: ResMut<Assets<Mesh>>,
             mut images: ResMut<Assets<Image>>,
             mut materials: ResMut<Assets<StandardMaterial>>| {
                mm2_app::drawbridge::spawn_drawbridges(
                    &mut commands,
                    &vfs.0,
                    "london",
                    None,
                    &mut meshes,
                    &mut images,
                    &mut materials,
                    SessionEntity(1),
                );
            },
        )
        .unwrap();
    for _ in 0..3 {
        app.update();
    }

    app.world_mut()
        .run_system_once(
            |spatial: SpatialQuery,
             names: Query<&Name>,
             leaves: Query<(&Name, &GlobalTransform)>| {
                // Leaves come in hinged pairs `drawbridge-<model>-<path>-<0|1>`.
                let mut hinges: std::collections::BTreeMap<String, Vec<(usize, Vec3)>> =
                    Default::default();
                for (n, t) in &leaves {
                    let n = n.to_string();
                    let Some(rest) = n.strip_prefix("drawbridge-") else {
                        continue;
                    };
                    let Some((path, leaf)) = rest.rsplit_once('-') else {
                        continue;
                    };
                    hinges
                        .entry(path.to_string())
                        .or_default()
                        .push((leaf.parse().unwrap(), t.translation()));
                }
                let mut spans = 0;
                let mut sampled = 0usize;
                let mut offences = Vec::new();
                for (path, mut h) in hinges {
                    h.sort_by_key(|(i, _)| *i);
                    let [(_, a), (_, b)] = h[..] else {
                        continue; // a lone dock leaf has no span
                    };
                    spans += 1;
                    let axis = (b - a).with_y(0.0);
                    let len = axis.length();
                    let dir = axis / len;
                    let side = Vec3::new(-dir.z, 0.0, dir.x);
                    let mut s = -APPROACH;
                    while s <= len + APPROACH {
                        let mut l = -HALF_WIDTH;
                        while l <= HALF_WIDTH {
                            let p = a + dir * s + side * l;
                            let top = a.y + 40.0;
                            let mut hits = spatial.ray_hits(
                                Vec3::new(p.x, top, p.z),
                                Dir3::NEG_Y,
                                80.0,
                                64,
                                true,
                                &SpatialQueryFilter::default(),
                            );
                            hits.sort_by(|x, y| x.distance.total_cmp(&y.distance));
                            let name =
                                |e: Entity| names.get(e).map(|n| n.to_string()).unwrap_or_default();
                            // The deck is the highest non-prop surface: road or leaf.
                            if let Some(deck) = hits
                                .iter()
                                .find(|h| !name(h.entity).starts_with("prop-"))
                                .map(|h| top - h.distance)
                            {
                                sampled += 1;
                                for h in &hits {
                                    let y = top - h.distance;
                                    let n = name(h.entity);
                                    if n.starts_with("prop-")
                                        && y > deck + KERB
                                        && y < deck + CLEARANCE
                                    {
                                        offences.push(format!(
                                            "{path} ({:.1}, {:.1}): {n} at +{:.2} m over the deck",
                                            p.x,
                                            p.z,
                                            y - deck
                                        ));
                                    }
                                }
                            }
                            l += 1.5;
                        }
                        s += 1.5;
                    }
                }
                assert!(spans >= 3, "only {spans} leaf pairs spawned");
                assert!(sampled > 500, "only {sampled} deck samples found a surface");
                assert!(
                    offences.is_empty(),
                    "{} obstacles proud of a closed leaf deck, first 10:\n{}",
                    offences.len(),
                    offences
                        .iter()
                        .take(10)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("\n")
                );
            },
        )
        .unwrap();
}
