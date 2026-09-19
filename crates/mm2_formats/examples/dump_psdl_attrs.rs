//! Dump a histogram + samples of PSDL room attributes from a `.psdl` file.
//!
//! Research tool: `cargo run -p mm2_formats --example dump_psdl_attrs -- <file.psdl> [type]`

use mm2_formats::psdl::{AttributeType, Psdl};

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("usage: dump_psdl_attrs <file.psdl> [type]");
    let want: Option<u8> = args.next().map(|s| s.parse().unwrap());
    let data = std::fs::read(&path).unwrap();
    let psdl = Psdl::parse(&data).unwrap();

    let mut hist = std::collections::BTreeMap::new();
    for room in &psdl.rooms {
        for a in &room.attributes {
            let raw = match a.kind {
                AttributeType::Unknown(u) => u,
                k => kind_raw(k),
            };
            let key = (raw, a.subtype, a.data.len());
            *hist.entry(key).or_insert(0usize) += 1;
        }
    }
    for ((raw, subtype, len), count) in &hist {
        println!("type={raw:#04x} subtype={subtype} len={len}  x{count}");
    }

    if let Some(t) = want {
        println!("\nsamples of type {t:#x}:");
        let mut shown = 0;
        'outer: for (ri, room) in psdl.rooms.iter().enumerate() {
            for a in &room.attributes {
                let raw = match a.kind {
                    AttributeType::Unknown(u) => u,
                    k => kind_raw(k),
                };
                if raw == t {
                    println!(
                        "room {ri} subtype={} last={} data={:?}",
                        a.subtype, a.last, a.data
                    );
                    shown += 1;
                    if shown >= 12 {
                        break 'outer;
                    }
                }
            }
        }
    }
}

fn kind_raw(k: AttributeType) -> u8 {
    match k {
        AttributeType::RoadWithSidewalks => 0,
        AttributeType::SidewalkStrip => 1,
        AttributeType::RoadNoSidewalks => 2,
        AttributeType::Sliver => 3,
        AttributeType::Crosswalk => 4,
        AttributeType::RoadFan => 5,
        AttributeType::Fan => 6,
        AttributeType::FacadeBound => 7,
        AttributeType::DividedRoad => 8,
        AttributeType::Tunnel => 9,
        AttributeType::TextureRef => 10,
        AttributeType::Facade => 11,
        AttributeType::RoofFan => 12,
        AttributeType::Unknown(u) => u,
    }
}
