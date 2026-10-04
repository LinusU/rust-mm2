//! Positional object sounds — the retail `Aud3DAmbientObject` rules
//! for the `aud/ambient/<name>.csv` tables the moving world objects
//! carry (`drawbridge`, `ferry`, `subwaycar`).
//!
//! Recovered from the retail executable (`docs/research/movers.md`
//! § "Object audio"):
//!
//! - each sample row has an `active` flag — the table's column is the
//!   initial value, the owning object switches it at runtime;
//! - `sample type` 0 is a loop, played while active and the emitter's
//!   speed lies in the row's `min speed..=max speed` window; turning it
//!   inactive stops it at once;
//! - types 1 and 2 are one-shots on a timer: while active (and in the
//!   speed window) a sample whose timer has run out is fired unless it
//!   is still playing, and the timer is redrawn uniformly within the
//!   row's `oneshot time limit low..high` seconds — a `0,0` window
//!   (the drawbridge bell) re-fires the moment the last ring ends.
//!   Deactivation lets a playing one-shot finish;
//! - type 3 never fires on its own (it waits for an explicit trigger
//!   no retail object issues).
//!
//! The emitter is silent from the table's `Max distance` out. Inside
//! it, rows of types 0, 2 and 3 play at their authored volume times
//! [`ObjectAudioSpec::falloff`] — full volume within `Min distance`,
//! then falling linearly in *squared* distance to nothing at the max —
//! re-applied every frame. Type 1 rows are not positional: each firing
//! draws a gain in `0.75..1` and a random pan (the pan is not
//! reproduced) and ignores distance.

use mm2_formats::cardata::ObjectAudio;

/// What a sample row does when active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleKind {
    /// Type 0 — a speed-gated loop.
    Loop,
    /// Type 1 — a timed one-shot at a random gain.
    RandomOneShot,
    /// Type 2 — a timed one-shot.
    TimedOneShot,
    /// Type 3, or anything unrecognised — never self-triggers.
    Triggered,
}

impl SampleKind {
    /// Whether the row's volume follows the emitter's distance (every
    /// kind but the random one-shot, which draws its own gain and pan).
    pub fn is_positional(self) -> bool {
        self != Self::RandomOneShot
    }

    fn from_code(code: f32) -> Self {
        match code as i32 {
            0 => Self::Loop,
            1 => Self::RandomOneShot,
            2 => Self::TimedOneShot,
            _ => Self::Triggered,
        }
    }
}

/// One sample row.
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectSampleSpec {
    /// Wave stem.
    pub name: String,
    /// Authored volume.
    pub volume: f32,
    /// What it does.
    pub kind: SampleKind,
    /// One-shot re-fire window, seconds.
    pub interval: (f32, f32),
    /// The table's initial `active` value.
    pub initially_active: bool,
    /// Emitter-speed window.
    pub speed: (f32, f32),
}

/// A parsed `aud/ambient/<name>.csv` table.
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectAudioSpec {
    /// The table stem (`drawbridge`).
    pub name: String,
    /// Full volume within this many metres of the listener.
    pub min_distance: f32,
    /// Silent from this many metres out.
    pub max_distance: f32,
    /// The sample rows.
    pub samples: Vec<ObjectSampleSpec>,
}

impl ObjectAudioSpec {
    /// The logical path the original loads for `name`.
    pub fn logical(name: &str) -> String {
        format!("aud/ambient/{name}.csv")
    }

    /// Distil a parsed table.
    pub fn from_table(name: &str, table: &ObjectAudio) -> Self {
        Self {
            name: name.to_string(),
            min_distance: table.min_distance,
            max_distance: table.max_distance,
            samples: table
                .samples
                .iter()
                .map(|s| ObjectSampleSpec {
                    name: s.name.clone(),
                    volume: s.volume,
                    kind: SampleKind::from_code(s.kind),
                    interval: (s.oneshot_low, s.oneshot_high),
                    initially_active: s.active != 0.0,
                    speed: (s.min_speed, s.max_speed),
                })
                .collect(),
        }
    }

    /// Whether a listener `distance` metres away hears the emitter at
    /// all — the original stops it once the squared distance reaches
    /// the squared max.
    pub fn in_range(&self, distance: f32) -> bool {
        distance < self.max_distance
    }

    /// Volume factor for positional rows at `distance` metres: 1 within
    /// `Min distance`, then `1 − (d² − min²) / (max² − min²)` down to 0
    /// at the max (`0x511eb0`, `0x512260`).
    pub fn falloff(&self, distance: f32) -> f32 {
        let d2 = distance * distance;
        let min2 = self.min_distance * self.min_distance;
        let max2 = self.max_distance * self.max_distance;
        if d2 <= min2 {
            return 1.0;
        }
        if max2 <= min2 {
            return 0.0;
        }
        (1.0 - (d2 - min2) / (max2 - min2)).clamp(0.0, 1.0)
    }
}

/// What one emitter wants this tick.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ObjectAudioCues {
    /// Loop rows that should be sounding.
    pub loops: Vec<usize>,
    /// One-shot rows to fire now (with their gain factor), each only
    /// if no earlier firing of the row is still playing.
    pub fire: Vec<(usize, f32)>,
}

/// One emitter's runtime state.
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectAudioState {
    /// Per-row active flags.
    pub active: Vec<bool>,
    timers: Vec<f32>,
    rng: u32,
}

impl ObjectAudioState {
    /// Fresh state: rows take their authored initial `active`; `seed`
    /// feeds the re-fire draws (designed — the original's stream is
    /// shared with the whole game).
    pub fn new(spec: &ObjectAudioSpec, seed: u32) -> Self {
        Self {
            active: spec.samples.iter().map(|s| s.initially_active).collect(),
            timers: vec![0.0; spec.samples.len()],
            rng: seed | 1,
        }
    }

    /// Switch row `index` (or, with `None`, every row) on or off.
    pub fn set_active(&mut self, index: Option<usize>, on: bool) {
        match index {
            Some(i) => {
                if let Some(a) = self.active.get_mut(i) {
                    *a = on;
                }
            }
            None => self.active.iter_mut().for_each(|a| *a = on),
        }
    }

    fn draw(&mut self) -> f32 {
        // xorshift32 — any uniform source will do.
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng >> 8) as f32 / (1u32 << 24) as f32
    }

    /// Advance `dt` seconds with the emitter moving at `speed`.
    pub fn step(&mut self, spec: &ObjectAudioSpec, dt: f32, speed: f32) -> ObjectAudioCues {
        let mut cues = ObjectAudioCues::default();
        for (i, s) in spec.samples.iter().enumerate() {
            if !self.active.get(i).copied().unwrap_or(false) {
                continue;
            }
            let in_window = speed >= s.speed.0 && speed <= s.speed.1;
            match s.kind {
                SampleKind::Loop => {
                    if in_window {
                        cues.loops.push(i);
                    }
                }
                SampleKind::RandomOneShot | SampleKind::TimedOneShot => {
                    if !in_window {
                        continue;
                    }
                    if self.timers[i] <= 0.0 {
                        let gain = if s.kind == SampleKind::RandomOneShot {
                            0.75 + 0.25 * self.draw()
                        } else {
                            1.0
                        };
                        cues.fire.push((i, gain));
                        let (lo, hi) = s.interval;
                        self.timers[i] = if lo == hi {
                            hi
                        } else {
                            lo + (hi - lo) * self.draw()
                        };
                    }
                    self.timers[i] -= dt;
                }
                SampleKind::Triggered => {}
            }
        }
        cues
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> ObjectAudioSpec {
        let table = ObjectAudio::parse(
            b"Min distance,Max distance,3D priority,audible area,,,,,\n\
              0,225,12,0,,,,,\n\
              sample name,sample volume,sample type,oneshot time limit low,oneshot time limit high,active,min speed,max speed,doppler\n\
              ferryengine,0.98,0,0,0,1,0,999999,1\n\
              ferryhorn,1,2,5,10,1,0,999999,1\n\
              bridgebell,0.95,2,0,0,0,0,999999,1\n",
        )
        .unwrap();
        ObjectAudioSpec::from_table("ferry", &table)
    }

    #[test]
    fn table_rows_decode() {
        let s = spec();
        assert_eq!(s.min_distance, 0.0);
        assert_eq!(s.max_distance, 225.0);
        assert_eq!(s.samples[0].kind, SampleKind::Loop);
        assert_eq!(s.samples[1].kind, SampleKind::TimedOneShot);
        assert_eq!(s.samples[1].interval, (5.0, 10.0));
        assert!(!s.samples[2].initially_active);
    }

    #[test]
    fn active_loops_sound_and_horns_refire_in_their_window() {
        let s = spec();
        let mut st = ObjectAudioState::new(&s, 7);
        let mut horns = Vec::new();
        let mut t = 0.0;
        while t < 60.0 {
            let cues = st.step(&s, 0.1, 0.0);
            assert_eq!(cues.loops, vec![0]);
            for (i, _) in cues.fire {
                assert_eq!(i, 1, "the inactive bell never fires");
                horns.push(t);
            }
            t += 0.1;
        }
        assert_eq!(horns[0], 0.0);
        for w in horns.windows(2) {
            let gap = w[1] - w[0];
            assert!((4.9..=10.2).contains(&gap), "{gap}");
        }
    }

    #[test]
    fn a_zero_window_one_shot_refires_every_tick_until_switched_off() {
        let s = spec();
        let mut st = ObjectAudioState::new(&s, 7);
        st.set_active(Some(2), true);
        for _ in 0..3 {
            assert!(st.step(&s, 0.1, 0.0).fire.iter().any(|(i, _)| *i == 2));
        }
        st.set_active(None, false);
        let cues = st.step(&s, 0.1, 0.0);
        assert!(cues.loops.is_empty() && cues.fire.is_empty());
    }

    #[test]
    fn falloff_is_linear_in_squared_distance_between_min_and_max() {
        let mut s = spec();
        s.min_distance = 100.0;
        s.max_distance = 250.0;
        assert_eq!(s.falloff(0.0), 1.0);
        assert_eq!(s.falloff(100.0), 1.0);
        // Halfway in d² (100² + (250² − 100²) / 2 = 36250).
        assert!((s.falloff(36250f32.sqrt()) - 0.5).abs() < 1e-5);
        assert_eq!(s.falloff(250.0), 0.0);
        assert_eq!(s.falloff(400.0), 0.0);
        assert!(s.in_range(249.9) && !s.in_range(250.0));
        assert!(!SampleKind::RandomOneShot.is_positional());
        assert!(SampleKind::Loop.is_positional());
    }

    #[test]
    fn loops_respect_the_speed_window() {
        let mut s = spec();
        s.samples[0].speed = (1.0, 10.0);
        let mut st = ObjectAudioState::new(&s, 1);
        assert!(st.step(&s, 0.1, 0.0).loops.is_empty());
        assert_eq!(st.step(&s, 0.1, 5.0).loops, vec![0]);
    }
}
