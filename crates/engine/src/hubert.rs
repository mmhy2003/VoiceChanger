//! ContentVec/HuBERT content features and their upsampling to the 100 Hz F0 frame rate.

use std::path::Path;

use ort::session::Session;
use ort::value::Tensor;

use crate::error::{EngineError, Result};
use crate::runtime::{self, ActiveProvider, Provider};

/// Features per content frame (ContentVec layer 12).
pub const DIM: usize = 768;
/// Zeros added on each side of the audio before encoding.
const EDGE_PAD: usize = 40;
/// Shortest input (one 25 ms receptive field) the encoder accepts.
const MIN_SAMPLES: usize = 400;

/// Repeats each `DIM`-value row of `feats` twice: 50 Hz frames become 100 Hz frames.
pub fn upsample2(feats: &[f32], frames: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(2 * frames * DIM);
    for row in feats.as_chunks::<DIM>().0.iter().take(frames) {
        out.extend_from_slice(row);
        out.extend_from_slice(row);
    }
    out
}

/// A ContentVec session.
pub struct ContentEncoder {
    session: Session,
    rank: usize,
    active: ActiveProvider,
}

impl ContentEncoder {
    /// Opens the ContentVec model at `path` on `provider`, falling back to the CPU.
    pub fn load(path: &Path, provider: Provider) -> Result<Self> {
        let (session, active) = runtime::open_session(path, provider)?;
        let rank = session.inputs()[0]
            .dtype()
            .tensor_shape()
            .map_or(0, |shape| shape.len());
        if !matches!(rank, 2 | 3) {
            return Err(EngineError::UnsupportedModel {
                reason: format!("content encoder input has rank {rank}, expected 2 or 3"),
            });
        }
        Ok(Self {
            session,
            rank,
            active,
        })
    }

    pub fn active_provider(&self) -> ActiveProvider {
        self.active
    }

    /// Content features of 16 kHz audio, row-major `[T][DIM]`, and `T` (about one frame per 320 samples).
    pub fn encode(&mut self, audio_16k: &[f32]) -> Result<(Vec<f32>, usize)> {
        let n = (audio_16k.len() + 2 * EDGE_PAD).max(MIN_SAMPLES);
        let mut data = vec![0.0f32; n];
        data[EDGE_PAD..EDGE_PAD + audio_16k.len()].copy_from_slice(audio_16k);
        let input = if self.rank == 3 {
            Tensor::from_array(([1usize, 1, n], data))
        } else {
            Tensor::from_array(([1usize, n], data))
        }
        .map_err(inference)?;
        let outputs = self.session.run(ort::inputs![input]).map_err(inference)?;
        let (shape, feats) = outputs[0].try_extract_tensor::<f32>().map_err(inference)?;
        let d = shape.last().copied().unwrap_or(0);
        if d != DIM as i64 {
            return Err(EngineError::UnsupportedModel {
                reason: format!("content encoder gives {d} features, expected {DIM}"),
            });
        }
        Ok((feats.to_vec(), feats.len() / DIM))
    }
}

fn inference(e: ort::Error) -> EngineError {
    EngineError::Inference(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::{self, HUBERT, ORT_DLL};
    use crate::testutil::assets_or_skip;

    fn encoder() -> ContentEncoder {
        let enc = ContentEncoder::load(&assets::asset(HUBERT).unwrap(), Provider::Cpu).unwrap();
        assert_eq!(enc.active_provider(), Provider::Cpu);
        enc
    }

    #[test]
    fn upsample2_repeats_rows() {
        let r0 = vec![1.0; DIM];
        let r1 = vec![2.0; DIM];
        let up = upsample2(&[r0.clone(), r1.clone()].concat(), 2);
        assert_eq!(up, [r0.clone(), r0, r1.clone(), r1].concat());
    }

    #[test]
    fn upsample2_empty() {
        assert!(upsample2(&[], 0).is_empty());
    }

    #[test]
    fn hubert_one_second_is_50_frames() {
        if !assets_or_skip(&[ORT_DLL, HUBERT]) {
            return;
        }
        let audio: Vec<f32> = (0..16000)
            .map(|n| 0.1 * (2.0 * std::f32::consts::PI * 220.0 * n as f32 / 16000.0).sin())
            .collect();
        let (feats, t) = encoder().encode(&audio).unwrap();
        assert_eq!(t, 50);
        assert_eq!(feats.len(), 50 * DIM);
        assert!(feats.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn hubert_short_input_padded() {
        if !assets_or_skip(&[ORT_DLL, HUBERT]) {
            return;
        }
        let (feats, t) = encoder().encode(&[0.01; 100]).unwrap();
        assert!(t >= 1);
        assert_eq!(feats.len(), t * DIM);
    }
}
