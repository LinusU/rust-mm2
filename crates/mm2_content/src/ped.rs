//! `anim/pedmodel_<name>.*` → [`PedArchetype`] producer (F19-A.5).
//!
//! One pedestrian archetype is a skeleton (`.skel`), a skinned mesh
//! (`.mod`), an authored state model (`.csv`), a paint-job shader table
//! (`.shaders`) and every animation clip the state model names
//! (`anim/<stem>.anim`). [`PedArchetype::load`] reads all of them
//! through the VFS, runs the `mm2_formats::ped` parsers and the
//! `mm2_game::ped` assemblers, and refuses an archetype that cannot be
//! driven end to end — a state naming a clip that does not resolve, or a
//! shader table that does not line up with the mesh's material groups,
//! is an error rather than a silent stand-in. The consumer (the app's
//! pedestrian visuals) holds the result read-only and shares it across
//! every actor of the archetype.

use std::collections::HashMap;
use std::fmt;

use mm2_assets::Vfs;
use mm2_formats::ped::{PedAnim, PedMod, PedSkel, PedStates};
use mm2_formats::pkg::{PkgShader, PkgShaders};
use mm2_game::ped::{PED_STATE_FPS, PedAnimError, PedAnimator, PedRig, PedSkin};

/// Why an archetype cannot be loaded.
#[derive(Debug)]
pub enum PedLoadError {
    /// A required member does not resolve through the VFS.
    Missing(String),
    /// A member resolved but could not be read or parsed.
    Parse {
        /// Logical path.
        path: String,
        /// The parser's message.
        error: String,
    },
    /// The skeleton does not form a single-rooted rig.
    Rig(String),
    /// The mesh does not assemble against the rig.
    Skin(String),
    /// The state model cannot start an animator.
    States(PedAnimError),
    /// A state names a clip that does not resolve to a parsed clip.
    MissingClip {
        /// The state row.
        state: String,
        /// The clip stem it names.
        clip: String,
    },
    /// The `.shaders` table is not `paint_jobs × material groups` wide.
    ShaderTable {
        /// Material groups in the mesh.
        materials: usize,
        /// Shaders per paint job in the table.
        per_paint_job: usize,
    },
}

impl fmt::Display for PedLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(p) => write!(f, "{p}: not found in the VFS"),
            Self::Parse { path, error } => write!(f, "{path}: {error}"),
            Self::Rig(e) => write!(f, "skeleton: {e}"),
            Self::Skin(e) => write!(f, "mesh: {e}"),
            Self::States(e) => write!(f, "state model: {e}"),
            Self::MissingClip { state, clip } => {
                write!(f, "state {state:?} names clip {clip:?}, which did not load")
            }
            Self::ShaderTable {
                materials,
                per_paint_job,
            } => write!(
                f,
                "shader table carries {per_paint_job} shaders per paint job against {materials} material groups"
            ),
        }
    }
}

impl std::error::Error for PedLoadError {}

/// A fully loaded pedestrian archetype.
#[derive(Debug)]
pub struct PedArchetype {
    /// File stem under `anim/`, e.g. `pedmodel_man`.
    pub stem: String,
    /// The flattened skeleton.
    pub rig: PedRig,
    /// The assembled skinned mesh.
    pub skin: PedSkin,
    /// The authored state model.
    pub states: PedStates,
    /// Paint-job shader table: material group `m` of paint job `p` is
    /// `shaders[p * shaders_per_paint_job + m]`.
    pub shaders: PkgShaders,
    /// Every clip the state model names, keyed by lower-cased stem.
    clips: HashMap<String, PedAnim>,
}

fn read_text(vfs: &Vfs, path: &str) -> Result<String, PedLoadError> {
    let bytes = read_bytes(vfs, path)?;
    String::from_utf8(bytes).map_err(|e| PedLoadError::Parse {
        path: path.to_string(),
        error: format!("not UTF-8 text: {e}"),
    })
}

fn read_bytes(vfs: &Vfs, path: &str) -> Result<Vec<u8>, PedLoadError> {
    if vfs.resolve(path).is_none() {
        return Err(PedLoadError::Missing(path.to_string()));
    }
    vfs.read_logical(path).map_err(|e| PedLoadError::Parse {
        path: path.to_string(),
        error: e.to_string(),
    })
}

impl PedArchetype {
    /// Load `anim/<stem>.{skel,mod,csv,shaders}` and every clip the
    /// state model references.
    pub fn load(vfs: &Vfs, stem: &str) -> Result<Self, PedLoadError> {
        let parse = |path: &str, error: String| PedLoadError::Parse {
            path: path.to_string(),
            error,
        };

        let skel_path = format!("anim/{stem}.skel");
        let skel = PedSkel::parse(&read_text(vfs, &skel_path)?)
            .map_err(|e| parse(&skel_path, e.to_string()))?;
        let rig = PedRig::from_skel(&skel).map_err(|e| PedLoadError::Rig(e.to_string()))?;

        let mod_path = format!("anim/{stem}.mod");
        let mesh = PedMod::parse(&read_text(vfs, &mod_path)?)
            .map_err(|e| parse(&mod_path, e.to_string()))?;
        let skin = PedSkin::from_mod(&mesh, &rig).map_err(|e| PedLoadError::Skin(e.to_string()))?;

        let csv_path = format!("anim/{stem}.csv");
        let states = PedStates::parse(&read_text(vfs, &csv_path)?)
            .map_err(|e| parse(&csv_path, e.to_string()))?;

        let shaders_path = format!("anim/{stem}.shaders");
        let shaders = PkgShaders::parse(&read_bytes(vfs, &shaders_path)?)
            .map_err(|e| parse(&shaders_path, e.to_string()))?;
        if shaders.paint_jobs == 0
            || shaders.shaders_per_paint_job as usize != skin.materials().len()
        {
            return Err(PedLoadError::ShaderTable {
                materials: skin.materials().len(),
                per_paint_job: shaders.shaders_per_paint_job as usize,
            });
        }

        let mut clips = HashMap::new();
        for s in &states.states {
            let key = s.anim.to_ascii_lowercase();
            if clips.contains_key(&key) {
                continue;
            }
            let missing = || PedLoadError::MissingClip {
                state: s.name.clone(),
                clip: s.anim.clone(),
            };
            let path = format!("anim/{key}.anim");
            if vfs.resolve(&path).is_none() {
                return Err(missing());
            }
            let bytes = vfs.read_logical(&path).map_err(|_| missing())?;
            let clip = PedAnim::parse(&bytes).map_err(|_| missing())?;
            clips.insert(key, clip);
        }

        let archetype = PedArchetype {
            stem: stem.to_string(),
            rig,
            skin,
            states,
            shaders,
            clips,
        };
        // A model with no starting state cannot animate at all.
        archetype
            .animator(archetype.start_state())
            .map_err(PedLoadError::States)?;
        Ok(archetype)
    }

    /// The clip a state row plays (`anim` stem, case-insensitive).
    pub fn clip(&self, stem: &str) -> Option<&PedAnim> {
        self.clips.get(&stem.to_ascii_lowercase())
    }

    /// Number of clips the state model pulled in.
    pub fn clip_count(&self) -> usize {
        self.clips.len()
    }

    /// Number of authored paint jobs (clothing colour sets).
    pub fn paint_jobs(&self) -> usize {
        self.shaders.paint_jobs as usize
    }

    /// The shader for material group `material` under `paint` — `None`
    /// when either index is outside the table.
    pub fn shader(&self, paint: usize, material: usize) -> Option<&PkgShader> {
        let per = self.shaders.shaders_per_paint_job as usize;
        if paint >= self.paint_jobs() || material >= per {
            return None;
        }
        self.shaders.shaders.get(paint * per + material)
    }

    /// The state an idle pedestrian starts in: authored `STAND` when the
    /// model has one, else its first row.
    pub fn start_state(&self) -> &str {
        self.states
            .states
            .iter()
            .find(|s| s.name == "STAND")
            .or(self.states.states.first())
            .map_or("STAND", |s| s.name.as_str())
    }

    /// A fresh animator over the state model at the designed playback
    /// rate ([`PED_STATE_FPS`]).
    pub fn animator(&self, start: &str) -> Result<PedAnimator, PedAnimError> {
        PedAnimator::new(&self.states, start, PED_STATE_FPS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mm2_assets::InstallMount;
    use std::path::Path;

    fn write(dir: &Path, logical: &str, bytes: &[u8]) {
        let p = dir.join(logical);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    fn vfs_of(dir: &Path) -> Vfs {
        let mut vfs = Vfs::new();
        mm2_assets::mount_install(&mut vfs, dir, &InstallMount::default()).unwrap();
        vfs
    }

    const SKEL: &str = "NumBones 3\nbone root {\n\toffset 0 1 0\n\tbone a {\n\t\toffset 0 0 0\n\t}\n\tbone b {\n\t\toffset 0 0 0\n\t}\n}\n";
    const CSV: &str = "# header\nSTAND,xstand,1,2,0,0,0,0,STAND\nWALK,xwalk,1,3,0,1,0,0,WALK\n";

    /// fpf = (bones + 1) * 3 = 12.
    fn clip(frames: u32) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&frames.to_le_bytes());
        b.extend_from_slice(&12u32.to_le_bytes());
        b.extend_from_slice(&0f32.to_le_bytes());
        b.push(1);
        for _ in 0..frames * 12 {
            b.extend_from_slice(&0f32.to_le_bytes());
        }
        b
    }

    /// `paints` paint jobs of `per` float shaders each.
    fn shaders(paints: u32, per: u32) -> Vec<u8> {
        let mut s = Vec::new();
        s.extend_from_slice(&paints.to_le_bytes());
        s.extend_from_slice(&per.to_le_bytes());
        for _ in 0..paints * per {
            s.push(0);
            for c in [0.5f32; 16] {
                s.extend_from_slice(&c.to_le_bytes());
            }
            s.extend_from_slice(&4f32.to_le_bytes());
        }
        s
    }

    /// A minimal packet-dialect `.mod`: 3 verts, 2 material groups, 3
    /// matrices (one per bone).
    const MOD: &str = "\
version: 1.09
verts: 3
normals: 3
colors: 1
tex1s: 1
tex2s: 0
tangents: 0
materials: 2
adjuncts: 3
primitives: 2
matrices: 3

v	0.0	0.0	0.0
v	0.0	-1.0	0.0
v	0.0	1.0	0.0
n	0.0	0.0	1.0
n	0.0	0.0	1.0
n	0.0	0.0	1.0
c	1.0	1.0	1.0	1.0
t1	0.5	0.5

mtl Test1:SKIN {
	packets:	1
	primitives:	1
	textures:	0
	illum: diffuse
	ambient:	0.4 0.3 0.2
	diffuse:	0.7 0.6 0.5
	specular:	0.8 0.7 0.6
}

mtl Test1:HAIR {
	packets:	1
	primitives:	1
	textures:	0
	illum: diffuse
	ambient:	0.1 0.0 0.0
	diffuse:	0.4 0.1 0.1
	specular:	0.6 0.4 0.4
}

packet 2 1 2 {
	adj	0	0	0	0	0	0
	adj	1	1	0	0	0	1
	tri	0	1	0
	mtx 0 1
}

packet 1 1 1 {
	adj	2	2	0	0	0	0
	tri	0	0	0
	mtx 2
}

mtxv 1 1 1
mtxn 1 1 1
";

    fn seed(dir: &Path) {
        write(dir, "anim/pedmodel_x.skel", SKEL.as_bytes());
        write(dir, "anim/pedmodel_x.csv", CSV.as_bytes());
        write(dir, "anim/pedmodel_x.mod", MOD.as_bytes());
        write(dir, "anim/pedmodel_x.shaders", &shaders(2, 2));
        write(dir, "anim/xstand.anim", &clip(2));
        write(dir, "anim/xwalk.anim", &clip(3));
    }

    #[test]
    fn a_complete_archetype_loads_with_its_clips_and_paints() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        seed(&dir);
        let a = PedArchetype::load(&vfs_of(&dir), "pedmodel_x").unwrap();
        assert_eq!(a.rig.bone_count(), 3);
        assert_eq!(a.clip_count(), 2);
        assert_eq!(a.paint_jobs(), 2);
        assert!(a.clip("XWALK").is_some(), "clip lookup is case-insensitive");
        assert!(a.shader(1, 1).is_some());
        assert!(a.shader(2, 0).is_none() && a.shader(0, 2).is_none());
        assert_eq!(a.start_state(), "STAND");
        assert!(a.animator("WALK").is_ok());
    }

    #[test]
    fn a_missing_member_is_named() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        seed(&dir);
        std::fs::remove_file(dir.join("anim/pedmodel_x.shaders")).unwrap();
        let e = PedArchetype::load(&vfs_of(&dir), "pedmodel_x").unwrap_err();
        assert!(
            matches!(&e, PedLoadError::Missing(p) if p == "anim/pedmodel_x.shaders"),
            "{e}"
        );
    }

    #[test]
    fn a_state_naming_an_absent_clip_refuses_the_archetype() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        seed(&dir);
        std::fs::remove_file(dir.join("anim/xwalk.anim")).unwrap();
        let e = PedArchetype::load(&vfs_of(&dir), "pedmodel_x").unwrap_err();
        assert!(
            matches!(&e, PedLoadError::MissingClip { state, clip } if state == "WALK" && clip == "xwalk"),
            "{e}"
        );
    }

    #[test]
    fn a_corrupt_clip_refuses_the_archetype() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        seed(&dir);
        write(&dir, "anim/xstand.anim", b"not a clip");
        let e = PedArchetype::load(&vfs_of(&dir), "pedmodel_x").unwrap_err();
        assert!(matches!(e, PedLoadError::MissingClip { .. }), "{e}");
    }

    #[test]
    fn a_shader_table_of_the_wrong_width_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        seed(&dir);
        write(&dir, "anim/pedmodel_x.shaders", &shaders(2, 3));
        let e = PedArchetype::load(&vfs_of(&dir), "pedmodel_x").unwrap_err();
        assert!(
            matches!(
                e,
                PedLoadError::ShaderTable {
                    materials: 2,
                    per_paint_job: 3
                }
            ),
            "{e}"
        );
    }

    #[test]
    fn a_mesh_that_does_not_fit_the_rig_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        seed(&dir);
        // A 2-bone rig cannot host a mesh whose partition names 3 bones.
        write(
            &dir,
            "anim/pedmodel_x.skel",
            b"NumBones 2\nbone root {\n\toffset 0 1 0\n\tbone a {\n\t\toffset 0 0 0\n\t}\n}\n",
        );
        let e = PedArchetype::load(&vfs_of(&dir), "pedmodel_x").unwrap_err();
        assert!(matches!(e, PedLoadError::Skin(_)), "{e}");
    }
}
