//! Research aid for UNK-42: how crosswalk rectangles sit against the
//! BAI sidewalk curves. Run against `retail/`; not shipped.

use mm2_assets::{InstallMount, Vfs, mount_install};
use mm2_formats::psdl::{AttributeType, Psdl};
use mm2_game::nav::LaneKind;
use mm2_game::props::carriageways;

fn d3(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// One crosswalk's measurements (m): its length, then each end's
/// distance to the nearest sidewalk curve end and curve vertex.
type Row = (f32, f32, f32, f32, f32);

fn main() {
    let dir = std::env::args().nth(1).expect("install dir");
    let city = std::env::args().nth(2).unwrap_or_else(|| "london".into());
    let mut vfs = Vfs::new();
    mount_install(&mut vfs, dir.as_ref(), &InstallMount::default()).unwrap();
    let (bytes, _) = vfs.read_path(&format!("city/{city}.psdl")).unwrap();
    let psdl = Psdl::parse(&bytes).unwrap();
    let build = mm2_content::load_nav_graph(&vfs, &city).unwrap();
    let mut ends = Vec::new();
    let mut pts = Vec::new();
    for l in build.graph.lanes() {
        if l.id.kind != LaneKind::Sidewalk {
            continue;
        }
        let v = l.vertices();
        ends.push(v[0]);
        ends.push(v[v.len() - 1]);
        pts.extend(v.iter().copied());
    }
    let cw: Vec<_> = carriageways(&psdl)
        .into_iter()
        .filter(|c| c.kind == AttributeType::Crosswalk)
        .collect();
    println!(
        "{city}: {} crosswalks, {} sidewalk ends",
        cw.len(),
        ends.len()
    );
    let mut rows = Vec::new();
    for c in &cw {
        let r = &c.ring; // p0 p1 p3 p2
        let a = [
            (r[0][0] + r[1][0]) / 2.,
            (r[0][1] + r[1][1]) / 2.,
            (r[0][2] + r[1][2]) / 2.,
        ];
        let b = [
            (r[2][0] + r[3][0]) / 2.,
            (r[2][1] + r[3][1]) / 2.,
            (r[2][2] + r[3][2]) / 2.,
        ];
        let near =
            |p: [f32; 3], set: &[[f32; 3]]| set.iter().map(|q| d3(p, *q)).fold(f32::MAX, f32::min);
        rows.push((
            d3(a, b),
            near(a, &ends),
            near(b, &ends),
            near(a, &pts),
            near(b, &pts),
        ));
    }
    let hist = |name: &str, f: &dyn Fn(&Row) -> f32| {
        let mut v: Vec<f32> = rows.iter().map(f).collect();
        v.sort_by(|x, y| x.total_cmp(y));
        let q = |p: f32| v[((v.len() - 1) as f32 * p) as usize];
        println!(
            "{name}: min {:.1} p25 {:.1} med {:.1} p75 {:.1} max {:.1}",
            q(0.),
            q(0.25),
            q(0.5),
            q(0.75),
            q(1.)
        );
    };
    hist("crosswalk length", &|r| r.0);
    hist("end A -> nearest sidewalk END", &|r| r.1);
    hist("end B -> nearest sidewalk END", &|r| r.2);
    hist("end A -> nearest sidewalk VERTEX", &|r| r.3);
    hist("end B -> nearest sidewalk VERTEX", &|r| r.4);
    for t in [2.0, 4.0, 6.0, 8.0] {
        let n = rows.iter().filter(|r| r.3 <= t && r.4 <= t).count();
        let m = rows.iter().filter(|r| r.1 <= t && r.2 <= t).count();
        println!("both ends within {t} m: vertex {n}, curve-end {m}");
    }
}
