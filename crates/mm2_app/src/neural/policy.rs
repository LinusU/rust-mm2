//! The driving policy: a small multilayer perceptron evaluated on the
//! CPU, a few thousand multiply-adds per decision.
//!
//! Weights come from a `.mm2p` file the trainer exports
//! (`training/export.py`): a little-endian header and one dense layer
//! after another. Hidden layers use `tanh`; the output layer's `tanh`
//! bounds every action to `-1..=1`. The file records the observation
//! and action sizes it was trained for, and [`Policy::parse`] refuses a
//! file whose sizes do not match the running build's — a stale weight
//! file must fail to load, never drive on misaligned inputs.

use std::fmt;

/// File magic.
const MAGIC: &[u8; 4] = b"MM2P";
/// Format version this build reads.
const VERSION: u32 = 1;
/// Upper bound on any one layer's width — a corrupt header must not
/// turn into a multi-gigabyte allocation.
const MAX_WIDTH: usize = 4096;
/// Upper bound on the number of layers.
const MAX_LAYERS: usize = 16;

/// Why a policy file was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyError {
    /// Not a policy file.
    BadMagic,
    /// A format version this build does not read.
    Version(u32),
    /// The file ends before its declared contents do.
    Truncated,
    /// A layer shape that cannot be right.
    BadShape(String),
    /// Trained for a different observation or action layout.
    Mismatch {
        what: &'static str,
        file: usize,
        build: usize,
    },
    /// A weight that is not a finite number.
    NonFinite,
}

impl fmt::Display for PolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadMagic => write!(f, "not a policy file (bad magic)"),
            Self::Version(v) => write!(f, "policy format version {v} is not supported"),
            Self::Truncated => write!(f, "policy file is truncated"),
            Self::BadShape(s) => write!(f, "policy layer shape is invalid: {s}"),
            Self::Mismatch { what, file, build } => write!(
                f,
                "policy was trained for {what} size {file}, this build uses {build}"
            ),
            Self::NonFinite => write!(f, "policy holds a non-finite weight"),
        }
    }
}

impl std::error::Error for PolicyError {}

#[derive(Debug, Clone, PartialEq)]
struct Layer {
    inputs: usize,
    outputs: usize,
    /// Row-major, `outputs × inputs`.
    weights: Vec<f32>,
    bias: Vec<f32>,
}

/// A loaded policy network.
#[derive(Debug, Clone, PartialEq)]
pub struct Policy {
    layers: Vec<Layer>,
    /// Scratch widths: the widest layer, so evaluation never allocates.
    width: usize,
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn u32(&mut self) -> Result<u32, PolicyError> {
        let (head, rest) = self
            .0
            .split_first_chunk::<4>()
            .ok_or(PolicyError::Truncated)?;
        self.0 = rest;
        Ok(u32::from_le_bytes(*head))
    }

    fn f32s(&mut self, n: usize) -> Result<Vec<f32>, PolicyError> {
        let bytes = n.checked_mul(4).ok_or(PolicyError::Truncated)?;
        if self.0.len() < bytes {
            return Err(PolicyError::Truncated);
        }
        let (head, rest) = self.0.split_at(bytes);
        self.0 = rest;
        let v: Vec<f32> = head
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes(*c))
            .collect();
        if v.iter().any(|x| !x.is_finite()) {
            return Err(PolicyError::NonFinite);
        }
        Ok(v)
    }
}

impl Policy {
    /// Parse a `.mm2p` file, checking it against the observation and
    /// action sizes the caller will feed and read.
    pub fn parse(bytes: &[u8], obs_dim: usize, act_dim: usize) -> Result<Self, PolicyError> {
        let (magic, rest) = bytes
            .split_first_chunk::<4>()
            .ok_or(PolicyError::Truncated)?;
        if magic != MAGIC {
            return Err(PolicyError::BadMagic);
        }
        let mut r = Reader(rest);
        let version = r.u32()?;
        if version != VERSION {
            return Err(PolicyError::Version(version));
        }
        let file_obs = r.u32()? as usize;
        let file_act = r.u32()? as usize;
        if file_obs != obs_dim {
            return Err(PolicyError::Mismatch {
                what: "observation",
                file: file_obs,
                build: obs_dim,
            });
        }
        if file_act != act_dim {
            return Err(PolicyError::Mismatch {
                what: "action",
                file: file_act,
                build: act_dim,
            });
        }
        let count = r.u32()? as usize;
        if count == 0 || count > MAX_LAYERS {
            return Err(PolicyError::BadShape(format!("{count} layers")));
        }
        let mut layers = Vec::with_capacity(count);
        let mut expect = obs_dim;
        for i in 0..count {
            let inputs = r.u32()? as usize;
            let outputs = r.u32()? as usize;
            if inputs != expect || outputs == 0 || outputs > MAX_WIDTH || inputs > MAX_WIDTH {
                return Err(PolicyError::BadShape(format!(
                    "layer {i}: {inputs}→{outputs}, expected {expect} inputs"
                )));
            }
            let weights = r.f32s(inputs * outputs)?;
            let bias = r.f32s(outputs)?;
            layers.push(Layer {
                inputs,
                outputs,
                weights,
                bias,
            });
            expect = outputs;
        }
        if expect != act_dim {
            return Err(PolicyError::BadShape(format!(
                "last layer has {expect} outputs, expected {act_dim}"
            )));
        }
        let width = layers.iter().map(|l| l.outputs).max().unwrap_or(0);
        Ok(Self { layers, width })
    }

    /// Serialize back into the `.mm2p` layout [`Policy::parse`] reads.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(&(self.obs_dim() as u32).to_le_bytes());
        out.extend_from_slice(&(self.act_dim() as u32).to_le_bytes());
        out.extend_from_slice(&(self.layers.len() as u32).to_le_bytes());
        for l in &self.layers {
            out.extend_from_slice(&(l.inputs as u32).to_le_bytes());
            out.extend_from_slice(&(l.outputs as u32).to_le_bytes());
            for w in l.weights.iter().chain(&l.bias) {
                out.extend_from_slice(&w.to_le_bytes());
            }
        }
        out
    }

    /// Observation size this policy reads.
    pub fn obs_dim(&self) -> usize {
        self.layers.first().map_or(0, |l| l.inputs)
    }

    /// Action size this policy writes.
    pub fn act_dim(&self) -> usize {
        self.layers.last().map_or(0, |l| l.outputs)
    }

    /// Number of weights and biases.
    pub fn parameters(&self) -> usize {
        self.layers
            .iter()
            .map(|l| l.weights.len() + l.bias.len())
            .sum()
    }

    /// Evaluate the network: `obs` in, bounded actions out. `scratch`
    /// is reused between calls so a frame's worth of cars allocates
    /// nothing.
    pub fn act(&self, obs: &[f32], out: &mut [f32], scratch: &mut Scratch) {
        debug_assert_eq!(obs.len(), self.obs_dim());
        debug_assert_eq!(out.len(), self.act_dim());
        scratch.a.resize(self.width, 0.0);
        scratch.b.resize(self.width, 0.0);
        let (mut src, mut dst) = (&mut scratch.a, &mut scratch.b);
        for (i, layer) in self.layers.iter().enumerate() {
            let input: &[f32] = if i == 0 { obs } else { &src[..layer.inputs] };
            for (o, (row, bias)) in layer
                .weights
                .chunks_exact(layer.inputs)
                .zip(&layer.bias)
                .enumerate()
            {
                let sum: f32 = row.iter().zip(input).map(|(w, x)| w * x).sum();
                dst[o] = (sum + bias).tanh();
            }
            std::mem::swap(&mut src, &mut dst);
        }
        out.copy_from_slice(&src[..out.len()]);
    }
}

/// Reusable evaluation buffers for [`Policy::act`].
#[derive(Debug, Default, Clone)]
pub struct Scratch {
    a: Vec<f32>,
    b: Vec<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny() -> Policy {
        Policy {
            layers: vec![
                Layer {
                    inputs: 2,
                    outputs: 3,
                    weights: vec![1.0, 0.0, 0.0, 1.0, 0.5, -0.5],
                    bias: vec![0.0, 0.1, 0.0],
                },
                Layer {
                    inputs: 3,
                    outputs: 1,
                    weights: vec![1.0, -1.0, 2.0],
                    bias: vec![0.25],
                },
            ],
            width: 3,
        }
    }

    #[test]
    fn evaluates_the_network_it_was_given() {
        let p = tiny();
        let mut out = [0.0];
        p.act(&[0.3, -0.2], &mut out, &mut Scratch::default());
        let h = [
            0.3f32.tanh(),
            (-0.2f32 + 0.1).tanh(),
            (0.15f32 + 0.1).tanh(),
        ];
        let want = (h[0] - h[1] + 2.0 * h[2] + 0.25).tanh();
        assert!((out[0] - want).abs() < 1e-6);
    }

    #[test]
    fn round_trips_through_its_file_layout() {
        let p = tiny();
        assert_eq!(Policy::parse(&p.to_bytes(), 2, 1).unwrap(), p);
        assert_eq!(p.parameters(), 13);
    }

    #[test]
    fn refuses_a_file_for_another_layout() {
        let bytes = tiny().to_bytes();
        assert!(matches!(
            Policy::parse(&bytes, 3, 1),
            Err(PolicyError::Mismatch {
                what: "observation",
                ..
            })
        ));
        assert!(matches!(
            Policy::parse(&bytes, 2, 2),
            Err(PolicyError::Mismatch { what: "action", .. })
        ));
    }

    #[test]
    fn refuses_damaged_files() {
        let bytes = tiny().to_bytes();
        assert_eq!(
            Policy::parse(&bytes[..bytes.len() - 1], 2, 1),
            Err(PolicyError::Truncated)
        );
        assert_eq!(Policy::parse(b"nope", 2, 1), Err(PolicyError::BadMagic));
        let mut nan = bytes.clone();
        let at = nan.len() - 4;
        nan[at..].copy_from_slice(&f32::NAN.to_le_bytes());
        assert_eq!(Policy::parse(&nan, 2, 1), Err(PolicyError::NonFinite));
    }
}
