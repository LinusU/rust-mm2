//! Empirical analysis of PSDL facade orientation and texture usage on real
//! city data. Not a shipped tool — a research aid run against `retail/`.

use mm2_assets::{InstallMount, Vfs, mount_install};
use mm2_formats::psdl::{AttributeType, Psdl};

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| "retail".into());
    let city = std::env::args().nth(2).unwrap_or_else(|| "london".into());
    let mut vfs = Vfs::new();
    mount_install(&mut vfs, dir.as_ref(), &InstallMount::default()).unwrap();
    let (bytes, _) = vfs.read_path(&format!("city/{city}.psdl")).unwrap();
    let psdl = Psdl::parse(&bytes).unwrap();
    println!(
        "{}: {} rooms, {} verts, {} heights, {} textures",
        city,
        psdl.rooms.len(),
        psdl.vertices.len(),
        psdl.heights.len(),
        psdl.textures.len()
    );

    let v = |i: u16| -> (f32, f32) {
        let p = psdl.vertices[i as usize];
        (p[0], p[2])
    };

    // Which side of directed edge (l→r) is `p` on? >0 = left of (dx,dz)
    // in an x-right/z-up plot.
    let side = |l: (f32, f32), r: (f32, f32), p: (f32, f32)| -> f32 {
        let (dx, dz) = (r.0 - l.0, r.1 - l.1);
        -dz * (p.0 - l.0) + dx * (p.1 - l.1)
    };

    let mut n_rooms_with_roads = 0usize;
    let mut fac_road_side = [0usize; 2]; // [normal away from road, toward road]
    let mut fac_in_roadroom = 0usize;
    let mut fac_in_bldgroom = 0usize;
    let bldg_outward = 0usize;
    let mut bound_inward = 0usize;
    let mut bound_total = 0usize;
    let mut sliver_inward = 0usize;
    let mut sliver_total = 0usize;
    let mut fac_orient = Orient::default();
    let mut slv_orient = Orient::default();
    let mut fan_flat = 0usize;
    let mut fan_vertical = 0usize;
    let mut roadfan_vertical = 0usize;
    let mut fanvert_normal_in = 0usize;
    let mut fanvert_normal_out = 0usize;
    let mut fanvert_ambig = 0usize;
    let mut road_tex: std::collections::HashMap<usize, usize> = Default::default();

    for room in &psdl.rooms {
        // Collect this room's road-surface vertices (authored x,z).
        let mut road_pts: Vec<(f32, f32)> = Vec::new();
        for attr in &room.attributes {
            let per_item = match attr.kind {
                AttributeType::RoadWithSidewalks => 4,
                AttributeType::RoadNoSidewalks | AttributeType::SidewalkStrip => 2,
                AttributeType::DividedRoad => 6,
                _ => 0,
            };
            if per_item == 0 {
                if matches!(attr.kind, AttributeType::RoadFan) {
                    let refs: &[u16] = if attr.subtype == 0 {
                        attr.data.get(1..).unwrap_or(&[])
                    } else {
                        &attr.data
                    };
                    for &i in refs {
                        if (i as usize) < psdl.vertices.len() {
                            road_pts.push(v(i));
                        }
                    }
                }
                continue;
            }
            let refs: &[u16] = if attr.subtype == 0 {
                attr.data.get(1..).unwrap_or(&[])
            } else {
                &attr.data
            };
            for chunk in refs.chunks_exact(per_item) {
                // road centreline ≈ inner pair(s)
                for &i in chunk.iter().skip(1).take(per_item - 2) {
                    if (i as usize) < psdl.vertices.len() {
                        road_pts.push(v(i));
                    }
                }
            }
        }
        if !road_pts.is_empty() {
            n_rooms_with_roads += 1;
        }

        for attr in &room.attributes {
            let (l, r) = match attr.kind {
                AttributeType::Facade if attr.data.len() == 6 => (attr.data[4], attr.data[5]),
                AttributeType::Sliver if attr.data.len() == 4 => (attr.data[2], attr.data[3]),
                AttributeType::FacadeBound if attr.data.len() == 4 => (attr.data[2], attr.data[3]),
                _ => continue,
            };
            if l as usize >= psdl.vertices.len() || r as usize >= psdl.vertices.len() {
                continue;
            }
            let (lp, rp) = (v(l), v(r));
            // Which side do the room's road points sit on?
            let mut left = 0i32;
            let mut right = 0i32;
            for &p in &road_pts {
                let s = side(lp, rp, p);
                if s > 0.01 {
                    left += 1;
                } else if s < -0.01 {
                    right += 1;
                }
            }
            if matches!(attr.kind, AttributeType::Facade) && road_pts.is_empty() {
                fac_in_bldgroom += 1;
            }
            if left == right {
                continue; // undecidable
            }
            // Emitted normal (authored space) = right side of edge.
            let road_on_left = left > right;
            let normal_to_road = !road_on_left;
            match attr.kind {
                AttributeType::Facade => {
                    fac_in_roadroom += 1;
                    fac_road_side[normal_to_road as usize] += 1;
                }
                AttributeType::FacadeBound => {
                    bound_total += 1;
                    bound_inward += normal_to_road as usize;
                }
                AttributeType::Sliver => {
                    sliver_total += 1;
                    sliver_inward += normal_to_road as usize;
                }
                _ => {}
            }
        }

        // Does this room's road geometry contain the spawn point?
        // (authored x,z; spawn ≈ Bevy (98.9, 1.5, 176.6) → authored
        // (98.9, -176.6)).
        let (sx, sz) = (98.9f32, -176.6f32);
        if !road_pts.is_empty() {
            let minx = road_pts.iter().map(|p| p.0).fold(f32::MAX, f32::min);
            let maxx = road_pts.iter().map(|p| p.0).fold(f32::MIN, f32::max);
            let minz = road_pts.iter().map(|p| p.1).fold(f32::MAX, f32::min);
            let maxz = road_pts.iter().map(|p| p.1).fold(f32::MIN, f32::max);
            if sx >= minx - 30.0 && sx <= maxx + 30.0 && sz >= minz - 30.0 && sz <= maxz + 30.0 {
                let mut cur_tex: i64 = -1;
                let mut kinds = std::collections::HashMap::new();
                for attr in &room.attributes {
                    if attr.kind == AttributeType::TextureRef {
                        let raw = attr.data.first().copied().unwrap_or(0) as i64
                            + (attr.subtype as i64) * 256;
                        cur_tex = if raw == 0 { -2 } else { raw - 1 };
                        continue;
                    }
                    *kinds.entry(format!("{:?}", attr.kind)).or_insert(0) += 1;
                    if matches!(
                        attr.kind,
                        AttributeType::RoadWithSidewalks
                            | AttributeType::RoadNoSidewalks
                            | AttributeType::RoadFan
                            | AttributeType::DividedRoad
                            | AttributeType::Fan
                    ) {
                        let name = if cur_tex >= 0 {
                            psdl.textures
                                .get(cur_tex as usize)
                                .cloned()
                                .unwrap_or_default()
                        } else {
                            format!("(state {cur_tex})")
                        };
                        println!(
                            "  spawn-near room: {:?} → tex[{cur_tex}] {name:?}",
                            attr.kind
                        );
                    }
                }
                println!("  room attr kinds: {kinds:?}");
            }
        }

        // Per-wall orientation check: offset the edge midpoint along each
        // perpendicular; the side landing OUTSIDE the room's perimeter
        // polygon is the street side. After the winding flip, emitted
        // normals point left of l→r.
        let poly: Vec<(f32, f32)> = room.perimeter.iter().map(|p| v(p.vertex)).collect();
        for attr in &room.attributes {
            let (l, r, is_fac) = match attr.kind {
                AttributeType::Facade if attr.data.len() == 6 => (attr.data[4], attr.data[5], true),
                AttributeType::Sliver if attr.data.len() == 4 => {
                    (attr.data[2], attr.data[3], false)
                }
                _ => continue,
            };
            if l as usize >= psdl.vertices.len() || r as usize >= psdl.vertices.len() {
                continue;
            }
            let (lp, rp) = (v(l), v(r));
            let (dx, dz) = (rp.0 - lp.0, rp.1 - lp.1);
            let len = (dx * dx + dz * dz).sqrt();
            if len < 1e-3 {
                continue;
            }
            let mid = ((lp.0 + rp.0) / 2.0, (lp.1 + rp.1) / 2.0);
            let eps = 0.5f32;
            // left normal = (-dz, dx) normalized
            let nl = (-dz / len, dx / len);
            let inside_l = point_in_poly((mid.0 + nl.0 * eps, mid.1 + nl.1 * eps), &poly);
            let inside_r = point_in_poly((mid.0 - nl.0 * eps, mid.1 - nl.1 * eps), &poly);
            let t = if is_fac {
                &mut fac_orient
            } else {
                &mut slv_orient
            };
            match (inside_l, inside_r) {
                (true, false) => t.out_left += 1, // outside on right → flipped normal wrong
                (false, true) => t.out_right += 1, // outside on left → flipped normal right
                (true, true) => t.chord += 1,     // interior wall — undecidable
                (false, false) => t.outside += 1, // edge outside polygon entirely
            }
        }

        // Generic fans: are they all horizontal? Vertical fans emitted
        // with ground winding would be back-face culled → holes.
        for attr in &room.attributes {
            if attr.kind != AttributeType::Fan && attr.kind != AttributeType::RoadFan {
                continue;
            }
            let refs: &[u16] = if attr.subtype == 0 {
                attr.data.get(1..).unwrap_or(&[])
            } else {
                &attr.data
            };
            if refs.len() < 3 {
                continue;
            }
            let p0 = psdl.vertices[refs[0] as usize];
            let p1 = psdl.vertices[refs[1] as usize];
            let p2 = psdl.vertices[refs[2] as usize];
            let e1 = [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]];
            let e2 = [p2[0] - p0[0], p2[1] - p0[1], p2[2] - p0[2]];
            let n = [
                e1[1] * e2[2] - e1[2] * e2[1],
                e1[2] * e2[0] - e1[0] * e2[2],
                e1[0] * e2[1] - e1[1] * e2[0],
            ];
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            if len < 1e-6 {
                continue;
            }
            let ny = n[1].abs() / len;
            if attr.kind == AttributeType::Fan {
                if ny < 0.3 {
                    fan_vertical += 1;
                    // Which side of the fan plane holds the room interior?
                    // (midpoint ± geometric normal vs perimeter polygon)
                    let mid = ((p0[0] + p1[0] + p2[0]) / 3.0, (p0[2] + p1[2] + p2[2]) / 3.0);
                    let nl = (n[0] / len, n[2] / len); // authored normal xz
                    let inside_p = point_in_poly((mid.0 + nl.0 * 0.5, mid.1 + nl.1 * 0.5), &poly);
                    let inside_m = point_in_poly((mid.0 - nl.0 * 0.5, mid.1 - nl.1 * 0.5), &poly);
                    if inside_p != inside_m {
                        // authored fan order in (x,z): CW → front normal -y
                        // in authored space... record whether interior is
                        // on the geometric-normal side
                        if inside_p {
                            fanvert_normal_in += 1;
                        } else {
                            fanvert_normal_out += 1;
                        }
                    } else {
                        fanvert_ambig += 1;
                    }
                } else {
                    fan_flat += 1;
                }
            } else if ny < 0.3 {
                roadfan_vertical += 1;
            }
        }

        // Report rooms with tunnel attributes or facades near the spawn
        // (candidate causes of remaining see-through holes).
        let (sx, sz) = (98.9f32, -176.6f32);
        let has_tunnel = room
            .attributes
            .iter()
            .any(|a| a.kind == AttributeType::Tunnel);
        let n_fac = room
            .attributes
            .iter()
            .filter(|a| a.kind == AttributeType::Facade)
            .count();
        let n_fan = room
            .attributes
            .iter()
            .filter(|a| a.kind == AttributeType::Fan)
            .count();
        if (has_tunnel || n_fac > 0 || n_fan > 0) && !room.perimeter.is_empty() {
            let xs: Vec<f32> = room.perimeter.iter().map(|p| v(p.vertex).0).collect();
            let zs: Vec<f32> = room.perimeter.iter().map(|p| v(p.vertex).1).collect();
            let (minx, maxx) = (
                xs.iter().fold(f32::MAX, |a, &b| a.min(b)),
                xs.iter().fold(f32::MIN, |a, &b| a.max(b)),
            );
            let (minz, maxz) = (
                zs.iter().fold(f32::MAX, |a, &b| a.min(b)),
                zs.iter().fold(f32::MIN, |a, &b| a.max(b)),
            );
            if sx >= minx - 60.0 && sx <= maxx + 60.0 && sz >= minz - 60.0 && sz <= maxz + 60.0 {
                println!(
                    "  room near spawn: tunnel={has_tunnel} facades={n_fac} fans={n_fan} bbox=({minx:.0}..{maxx:.0}, {minz:.0}..{maxz:.0})"
                );
            }
        }

        // texture accounting for road attrs
        let mut cur_tex: i64 = -1;
        for attr in &room.attributes {
            if attr.kind == AttributeType::TextureRef {
                let raw =
                    attr.data.first().copied().unwrap_or(0) as i64 + (attr.subtype as i64) * 256;
                cur_tex = if raw == 0 { -2 } else { raw - 1 };
                continue;
            }
            if matches!(
                attr.kind,
                AttributeType::RoadWithSidewalks
                    | AttributeType::RoadNoSidewalks
                    | AttributeType::RoadFan
                    | AttributeType::DividedRoad
            ) && cur_tex >= 0
            {
                *road_tex.entry(cur_tex as usize).or_insert(0) += 1;
            }
        }
    }

    // Tunnel attributes: subtype, flags, heights, and (for junctions) the
    // enabled-wall bit pattern vs perimeter length.
    let mut tsub: std::collections::BTreeMap<u8, usize> = Default::default();
    for (ri, room) in psdl.rooms.iter().enumerate() {
        for attr in &room.attributes {
            if attr.kind != AttributeType::Tunnel {
                continue;
            }
            *tsub.entry(attr.subtype).or_insert(0) += 1;
            if attr.subtype == 0 && attr.data.len() >= 5 {
                let nsize = attr.data[0];
                let flags = attr.data[1];
                let h1 = attr.data[2];
                let h2 = attr.data[3];
                let unk3 = attr.data[4];
                let bits: Vec<u16> = attr.data[5..].to_vec();
                let set: usize = bits.iter().map(|w| w.count_ones() as usize).sum();
                let cx: f32 = room
                    .perimeter
                    .iter()
                    .map(|p| psdl.vertices[p.vertex as usize][0])
                    .sum::<f32>()
                    / room.perimeter.len().max(1) as f32;
                let cz: f32 = room
                    .perimeter
                    .iter()
                    .map(|p| psdl.vertices[p.vertex as usize][2])
                    .sum::<f32>()
                    / room.perimeter.len().max(1) as f32;
                let perim_verts: Vec<u16> = room.perimeter.iter().map(|p| p.vertex).collect();
                let dupes = perim_verts.len()
                    - perim_verts
                        .iter()
                        .collect::<std::collections::BTreeSet<_>>()
                        .len();
                println!(
                    "  junction-tunnel room {ri} nperim={} dupes={dupes} nsize={nsize} flags={flags:#06x} h1={h1} h2={h2} unk3={unk3} bits_set={set} centre=({cx:.0},{cz:.0}) perim={perim_verts:?} bits={bits:04x?}",
                    room.perimeter.len()
                );
            } else if attr.subtype == 3 && attr.data.len() >= 3 {
                let flags = attr.data[0];
                let h1 = attr.data[1];
                let h2 = attr.data[2];
                println!(
                    "  road-tunnel room {ri} flags={flags:#06x} h1={h1} h2={h2} words={}",
                    attr.data.len()
                );
            } else {
                println!(
                    "  tunnel room {ri} subtype={} words={} data={:?}",
                    attr.subtype,
                    attr.data.len(),
                    &attr.data[..attr.data.len().min(8)]
                );
            }
        }
    }
    println!("tunnel subtypes: {tsub:?}");

    // Sidewalk strip (0x01) height check: which of the pair is authored at
    // top height vs ground (lifted by the engine)?
    let mut dy = Vec::new();
    for room in &psdl.rooms {
        for attr in &room.attributes {
            if attr.kind != AttributeType::SidewalkStrip {
                continue;
            }
            let refs: &[u16] = if attr.subtype == 0 {
                attr.data.get(1..).unwrap_or(&[])
            } else {
                &attr.data
            };
            if refs.len() >= 4 && refs[0] == refs[1] && refs[0] <= 1 {
                continue; // end cap
            }
            for s in refs.chunks_exact(2) {
                let a = psdl.vertices.get(s[0] as usize);
                let b = psdl.vertices.get(s[1] as usize);
                if let (Some(a), Some(b)) = (a, b) {
                    dy.push(b[1] - a[1]);
                }
            }
        }
    }
    dy.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if !dy.is_empty() {
        println!(
            "sidewalk pair b.y - a.y: min={:.3} median={:.3} max={:.3} (n={})",
            dy[0],
            dy[dy.len() / 2],
            dy[dy.len() - 1],
            dy.len()
        );
    }

    // Probe: attributes whose vertex refs fall inside a box around the
    // visible plaza hole (authored coords). Reports the texture state so
    // suppressed (-2) attrs are visible.
    let (bx0, bx1, bz0, bz1) = (40.0f32, 95.0f32, -45.0f32, -25.0f32);
    for (ri, room) in psdl.rooms.iter().enumerate() {
        let in_box = |i: u16| {
            psdl.vertices
                .get(i as usize)
                .map(|p| p[0] >= bx0 && p[0] <= bx1 && p[2] >= bz0 && p[2] <= bz1)
                .unwrap_or(false)
        };
        let perim_hit = room.perimeter.iter().any(|p| in_box(p.vertex));
        let mut cur_tex: i64 = -1;
        let mut printed = false;
        for attr in &room.attributes {
            if attr.kind == AttributeType::TextureRef {
                let raw =
                    attr.data.first().copied().unwrap_or(0) as i64 + (attr.subtype as i64) * 256;
                cur_tex = if raw == 0 { -2 } else { raw - 1 };
                continue;
            }
            let refs: &[u16] = if attr.subtype == 0 && !attr.data.is_empty() {
                &attr.data[1..]
            } else {
                &attr.data
            };
            let hits = refs.iter().filter(|&&i| in_box(i)).count();
            if hits > 0 || (perim_hit && cur_tex == -2) {
                if !printed {
                    printed = true;
                    println!("  room {ri} (perim_hit={perim_hit}):");
                }
                let tname = if cur_tex >= 0 {
                    psdl.textures
                        .get(cur_tex as usize)
                        .cloned()
                        .unwrap_or_default()
                } else {
                    format!("(state {cur_tex})")
                };
                println!(
                    "    {:?} subtype={} hits={hits} tex={cur_tex} {tname:?} data={:?}",
                    attr.kind,
                    attr.subtype,
                    &attr.data[..attr.data.len().min(12)]
                );
            }
        }
    }

    // Coverage probe: grid-sample the island strip (Bevy coords) and
    // report points where NO emitted triangle covers the ground — the
    // visible holes. Triangles are projected to (x, z).
    let import = mm2_app::city::emit_psdl(&psdl, None);
    let (x0, x1, z0, z1) = (40.0f32, 100.0f32, 20.0f32, 60.0f32);
    // Flatten all triangles that could intersect the region once.
    type Tri2D = ([f32; 2], [f32; 2], [f32; 2], usize);
    let mut region_tris: Vec<Tri2D> = Vec::new();
    for m in &import.meshes {
        for tri in m.indices.chunks_exact(3) {
            let a = m.positions[tri[0] as usize];
            let b = m.positions[tri[1] as usize];
            let c = m.positions[tri[2] as usize];
            let (minx, maxx) = (a[0].min(b[0]).min(c[0]), a[0].max(b[0]).max(c[0]));
            let (minz, maxz) = (a[2].min(b[2]).min(c[2]), a[2].max(b[2]).max(c[2]));
            if maxx >= x0 && minx <= x1 && maxz >= z0 && minz <= z1 {
                region_tris.push(([a[0], a[2]], [b[0], b[2]], [c[0], c[2]], m.room + 1));
            }
        }
    }
    println!("region tris: {}", region_tris.len());
    let mut uncovered = 0usize;
    let mut covered = 0usize;
    let mut uncovered_pts: Vec<(f32, f32)> = Vec::new();
    let mut z = z0;
    while z <= z1 {
        let mut x = x0;
        while x <= x1 {
            let p = (x, z);
            let mut hit = false;
            for &(a, b, c, room) in &region_tris {
                let d1 = (b[0] - a[0]) * (p.1 - a[1]) - (b[1] - a[1]) * (p.0 - a[0]);
                let d2 = (c[0] - b[0]) * (p.1 - b[1]) - (c[1] - b[1]) * (p.0 - b[0]);
                let d3 = (a[0] - c[0]) * (p.1 - c[1]) - (a[1] - c[1]) * (p.0 - c[0]);
                let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
                let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
                if !(neg && pos) {
                    hit = true;
                    let _ = room;
                    break;
                }
            }
            if hit {
                covered += 1;
            } else {
                uncovered += 1;
                uncovered_pts.push(p);
            }
            x += 0.5;
        }
        z += 0.5;
    }
    println!("coverage grid x[{x0}..{x1}] z[{z0}..{z1}]: covered={covered} uncovered={uncovered}");
    if !uncovered_pts.is_empty() {
        // Cluster: bounding box of the uncovered points.
        let (mut ux0, mut ux1, mut uz0, mut uz1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for p in &uncovered_pts {
            ux0 = ux0.min(p.0);
            ux1 = ux1.max(p.0);
            uz0 = uz0.min(p.1);
            uz1 = uz1.max(p.1);
        }
        println!(
            "  uncovered bbox x[{ux0:.1}..{ux1:.1}] z[{uz0:.1}..{uz1:.1}] n={}",
            uncovered_pts.len()
        );
        println!(
            "  sample: {:?}",
            &uncovered_pts[..uncovered_pts.len().min(30)]
        );
    }

    println!("rooms with road geometry: {n_rooms_with_roads}");
    println!(
        "facades: outside-left {} (flipped=correct), outside-right {} (flipped=wrong), chord {}, outside {}",
        fac_orient.out_left, fac_orient.out_right, fac_orient.chord, fac_orient.outside
    );
    println!(
        "slivers: outside-left {}, outside-right {}, chord {}, outside {}",
        slv_orient.out_left, slv_orient.out_right, slv_orient.chord, slv_orient.outside
    );
    println!(
        "generic fans: {fan_flat} flat, {fan_vertical} vertical | road fans vertical: {roadfan_vertical}"
    );
    println!(
        "vertical fans: interior on authored-normal side {fanvert_normal_in}, on far side {fanvert_normal_out}, ambiguous {fanvert_ambig}"
    );
    println!(
        "facades in road rooms: {fac_in_roadroom}; normal toward road side: {} ({:.1}%)",
        fac_road_side[1],
        100.0 * fac_road_side[1] as f64 / fac_road_side.iter().sum::<usize>().max(1) as f64
    );
    println!("facades in road-less rooms (skipped side test): {fac_in_bldgroom}");
    println!(
        "facade bounds: {bound_total} decidable, {} toward road ({:.1}%)",
        bound_inward,
        100.0 * bound_inward as f64 / bound_total.max(1) as f64
    );
    println!(
        "slivers: {sliver_total} decidable, {} toward road ({:.1}%)",
        sliver_inward,
        100.0 * sliver_inward as f64 / sliver_total.max(1) as f64
    );
    let _ = bldg_outward;

    let mut tex_list: Vec<(usize, usize)> = road_tex.into_iter().collect();
    tex_list.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
    println!("top road textures:");
    for (idx, count) in tex_list.iter().take(12) {
        let name = psdl.textures.get(*idx).cloned().unwrap_or_default();
        println!("  tex[{idx}] {name:?} × {count}");
    }

    // Scan every referenced texture for alpha content — an alpha-bearing
    // facade texture would be wrongly cut out by Mask mode.
    let mut alpha_tex = 0usize;
    let mut alpha_examples = Vec::new();
    for (i, name) in psdl.textures.iter().enumerate() {
        let Some(r) = vfs.resolve_preferred(&format!("texture/{name}"), &["tex"]) else {
            continue;
        };
        let Ok(bytes) = vfs.read(&r) else { continue };
        let Ok(tex) = mm2_formats::tex::TexFile::parse(&bytes) else {
            continue;
        };
        let Some(rgba) = tex.decode_rgba(0) else {
            continue;
        };
        let transparent = rgba.chunks_exact(4).filter(|px| px[3] < 128).count();
        let total = rgba.len() / 4;
        if transparent > 0 {
            alpha_tex += 1;
            if alpha_examples.len() < 15 {
                alpha_examples.push(format!(
                    "{name}[{i}] fmt={:?} transparent={transparent}/{total}",
                    tex.header.format
                ));
            }
        }
    }
    println!("textures with transparent pixels: {alpha_tex}");
    for e in &alpha_examples {
        println!("  {e}");
    }

    // Show texture table entries around the dominant road index (n, n+1 =
    // road, sidewalk) and decode them to sanity-check colours.
    for idx in 118..=122 {
        println!("  textures[{idx}] = {:?}", psdl.textures.get(idx));
    }
    for name in [
        "rinter_l",
        "r2_l",
        "r1_stone256_l",
        "rinter_red_l",
        "sinter_l",
        "sinter2_l",
        psdl.textures.get(120).cloned().unwrap_or_default().as_str(),
        psdl.textures.get(121).cloned().unwrap_or_default().as_str(),
    ] {
        let Some(r) = vfs.resolve_preferred(&format!("texture/{name}"), &["tex"]) else {
            println!("texture/{name}: unresolved");
            continue;
        };
        let bytes = vfs.read(&r).unwrap();
        let tex = mm2_formats::tex::TexFile::parse(&bytes).unwrap();
        let rgba = tex.decode_rgba(0).unwrap();
        let n = rgba.len() / 4;
        let (mut sr, mut sg, mut sb, mut sa) = (0u64, 0u64, 0u64, 0u64);
        for px in rgba.chunks_exact(4) {
            sr += px[0] as u64;
            sg += px[1] as u64;
            sb += px[2] as u64;
            sa += px[3] as u64;
        }
        println!(
            "texture/{name}: {}x{} fmt={:?} mips={} avg=({},{},{},{})",
            tex.header.width,
            tex.header.height,
            tex.header.format,
            tex.levels.len(),
            sr / n as u64,
            sg / n as u64,
            sb / n as u64,
            sa / n as u64
        );
        // Dump mip0 as a PPM for visual inspection.
        let path = format!("/tmp/tex_{name}.ppm");
        let mut out = format!("P6\n{} {}\n255\n", tex.header.width, tex.header.height).into_bytes();
        for px in rgba.chunks_exact(4) {
            out.extend_from_slice(&px[..3]);
        }
        std::fs::write(&path, out).unwrap();
    }
}

/// Ray-cast point-in-polygon over the authored (x, z) perimeter.
fn point_in_poly(p: (f32, f32), poly: &[(f32, f32)]) -> bool {
    let mut inside = false;
    let n = poly.len();
    for i in 0..n {
        let (xi, zi) = poly[i];
        let (xj, zj) = poly[(i + 1) % n];
        if (zi > p.1) != (zj > p.1) && p.0 < (xj - xi) * (p.1 - zi) / (zj - zi) + xi {
            inside = !inside;
        }
    }
    inside
}

#[derive(Default)]
struct Orient {
    out_left: usize,
    out_right: usize,
    chord: usize,
    outside: usize,
}
