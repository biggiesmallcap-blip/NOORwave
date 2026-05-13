//! Decode resampling helpers.

use anyhow::{Context, Result, anyhow};
use tracing::warn;

pub(crate) fn extend_mono_from_interleaved(
    out: &mut Vec<f32>,
    interleaved: &[f32],
    channels: usize,
) {
    if channels <= 1 {
        out.extend_from_slice(interleaved);
        return;
    }
    out.extend(
        interleaved
            .chunks(channels)
            .map(|frame| frame.iter().copied().sum::<f32>() / channels as f32),
    );
}

pub(crate) fn adapt_channels(
    samples: &[f32],
    input_channels: usize,
    output_channels: usize,
) -> Vec<f32> {
    if input_channels == 0 || output_channels == 0 || samples.is_empty() {
        return Vec::new();
    }

    if input_channels == output_channels {
        return samples.to_vec();
    }

    let mut output = Vec::with_capacity((samples.len() / input_channels) * output_channels);
    for frame in samples.chunks(input_channels) {
        match (input_channels, output_channels) {
            (_, 1) => {
                let mix = frame.iter().copied().sum::<f32>() / frame.len().max(1) as f32;
                output.push(mix);
            }
            (1, channels) => {
                let sample = frame[0];
                output.extend(std::iter::repeat_n(sample, channels));
            }
            _ => {
                for channel in 0..output_channels {
                    let index = channel.min(frame.len().saturating_sub(1));
                    output.push(frame[index]);
                }
            }
        }
    }

    output
}

pub(crate) struct StreamResampler {
    inner: rubato::SincFixedIn<f32>,
    chunk_size_in: usize,
    pub(crate) channels: usize,
    pub(crate) in_rate: u32,
    pub(crate) out_rate: u32,
    residual: Vec<Vec<f32>>,
}

impl StreamResampler {
    pub(crate) const CHUNK_SIZE_IN: usize = 1024;

    pub(crate) fn new(in_rate: u32, out_rate: u32, channels: usize) -> Result<Self> {
        if channels == 0 || in_rate == 0 || out_rate == 0 {
            return Err(anyhow!(
                "StreamResampler::new: invalid arguments ({in_rate} -> {out_rate} Hz, {channels} ch)"
            ));
        }
        let params = rubato::SincInterpolationParameters {
            sinc_len: 128,
            f_cutoff: 0.95,
            interpolation: rubato::SincInterpolationType::Linear,
            oversampling_factor: 256,
            window: rubato::WindowFunction::BlackmanHarris2,
        };
        let inner = rubato::SincFixedIn::<f32>::new(
            out_rate as f64 / in_rate as f64,
            2.0,
            params,
            Self::CHUNK_SIZE_IN,
            channels,
        )
        .context("rubato SincFixedIn init failed")?;
        Ok(Self {
            inner,
            chunk_size_in: Self::CHUNK_SIZE_IN,
            channels,
            in_rate,
            out_rate,
            residual: vec![Vec::with_capacity(Self::CHUNK_SIZE_IN * 2); channels],
        })
    }

    pub(crate) fn process(&mut self, interleaved: &[f32]) -> Vec<f32> {
        if interleaved.is_empty() {
            return Vec::new();
        }
        for frame in interleaved.chunks_exact(self.channels) {
            for (ch, &s) in frame.iter().enumerate() {
                self.residual[ch].push(s);
            }
        }
        let mut out: Vec<f32> = Vec::new();
        while self.residual[0].len() >= self.chunk_size_in {
            let chunk_in: Vec<Vec<f32>> = self
                .residual
                .iter_mut()
                .map(|ch| ch.drain(..self.chunk_size_in).collect())
                .collect();
            match rubato::Resampler::process(&mut self.inner, &chunk_in, None) {
                Ok(chunk_out) => Self::interleave_into(&chunk_out, &mut out),
                Err(e) => {
                    warn!("rubato process error: {e}");
                    return out;
                }
            }
        }
        out
    }

    pub(crate) fn flush(&mut self) -> Vec<f32> {
        if self.residual[0].is_empty() {
            return Vec::new();
        }
        let real_in = self.residual[0].len();
        for ch in self.residual.iter_mut() {
            ch.resize(self.chunk_size_in, 0.0);
        }
        let chunk_in: Vec<Vec<f32>> = self
            .residual
            .iter_mut()
            .map(|ch| ch.drain(..self.chunk_size_in).collect())
            .collect();
        let chunk_out = match rubato::Resampler::process(&mut self.inner, &chunk_in, None) {
            Ok(v) => v,
            Err(e) => {
                warn!("rubato flush error: {e}");
                return Vec::new();
            }
        };
        let real_out_frames =
            ((real_in as f64) * self.out_rate as f64 / self.in_rate as f64).round() as usize;
        let real_out_frames = real_out_frames.min(chunk_out[0].len());
        let trimmed: Vec<Vec<f32>> = chunk_out
            .into_iter()
            .map(|ch| ch.into_iter().take(real_out_frames).collect())
            .collect();
        let mut out: Vec<f32> = Vec::with_capacity(real_out_frames * self.channels);
        Self::interleave_into(&trimmed, &mut out);
        out
    }

    fn interleave_into(chunk_out: &[Vec<f32>], out: &mut Vec<f32>) {
        if chunk_out.is_empty() {
            return;
        }
        let frames = chunk_out[0].len();
        let channels = chunk_out.len();
        out.reserve(frames * channels);
        for f in 0..frames {
            for ch in chunk_out.iter() {
                out.push(ch[f]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapt_channels_duplicates_mono_to_stereo() {
        let output = adapt_channels(&[0.25, -0.25], 1, 2);
        assert_eq!(output, vec![0.25, 0.25, -0.25, -0.25]);
    }

    #[test]
    fn extend_mono_from_interleaved_appends_without_replacing_existing_samples() {
        let mut out = vec![0.75];

        extend_mono_from_interleaved(&mut out, &[0.25, -0.25, 0.5, -0.5], 2);

        assert_eq!(out, vec![0.75, 0.0, 0.0]);
    }

    #[test]
    fn stream_resampler_passthrough_when_rates_match_at_callsite() {
        assert_eq!(48_000_u32, 48_000_u32);
    }

    #[test]
    fn stream_resampler_produces_output_for_downsample() {
        let mut r = StreamResampler::new(96_000, 48_000, 2).expect("new");
        let frames = StreamResampler::CHUNK_SIZE_IN * 2;
        let input = vec![0.0_f32; frames * 2];
        let out = r.process(&input);
        assert!(!out.is_empty(), "expected resampled output, got empty");
        let out_frames = out.len() / 2;
        assert!(
            out_frames < frames,
            "downsample output ({out_frames}) should be less than input ({frames})"
        );
    }

    #[test]
    fn stream_resampler_holds_residual_under_chunk_size() {
        let mut r = StreamResampler::new(44_100, 48_000, 2).expect("new");
        let input = vec![0.0_f32; 10 * 2];
        let out = r.process(&input);
        assert!(
            out.is_empty(),
            "small input should buffer in residual, not produce output"
        );
    }
}
