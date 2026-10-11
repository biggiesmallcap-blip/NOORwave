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
    output_scratch: Vec<Vec<f32>>,
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
        let output_scratch = rubato::Resampler::output_buffer_allocate(&inner, true);
        Ok(Self {
            inner,
            chunk_size_in: Self::CHUNK_SIZE_IN,
            channels,
            in_rate,
            out_rate,
            residual: (0..channels)
                .map(|_| Vec::with_capacity(Self::CHUNK_SIZE_IN))
                .collect(),
            output_scratch,
        })
    }

    pub(crate) fn process(&mut self, interleaved: &[f32]) -> Vec<f32> {
        if interleaved.is_empty() {
            return Vec::new();
        }
        let chunks =
            (self.residual[0].len() + interleaved.len() / self.channels) / self.chunk_size_in;
        // This wrapper keeps the ratio fixed. The next-call estimate bounds
        // every chunk, avoiding growth of the returned interleaved packet.
        let mut out = Vec::with_capacity(
            chunks * rubato::Resampler::output_frames_next(&self.inner) * self.channels,
        );
        for frame in interleaved.chunks_exact(self.channels) {
            for (ch, &s) in frame.iter().enumerate() {
                self.residual[ch].push(s);
            }
            if self.residual[0].len() < self.chunk_size_in {
                continue;
            }
            let result = rubato::Resampler::process_into_buffer(
                &mut self.inner,
                &self.residual,
                &mut self.output_scratch,
                None,
            );
            for channel in &mut self.residual {
                channel.clear();
            }
            match result {
                Ok((_, frames)) => Self::interleave_into(&self.output_scratch, frames, &mut out),
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
        let result = rubato::Resampler::process_into_buffer(
            &mut self.inner,
            &self.residual,
            &mut self.output_scratch,
            None,
        );
        for channel in &mut self.residual {
            channel.clear();
        }
        let frames = match result {
            Ok((_, frames)) => frames,
            Err(e) => {
                warn!("rubato flush error: {e}");
                return Vec::new();
            }
        };
        let real_out_frames =
            ((real_in as f64) * self.out_rate as f64 / self.in_rate as f64).round() as usize;
        let real_out_frames = real_out_frames.min(frames);
        let mut out: Vec<f32> = Vec::with_capacity(real_out_frames * self.channels);
        Self::interleave_into(&self.output_scratch, real_out_frames, &mut out);
        out
    }

    fn interleave_into(chunk_out: &[Vec<f32>], frames: usize, out: &mut Vec<f32>) {
        if chunk_out.is_empty() {
            return;
        }
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

    // Compare against the former allocating path using rubato's independent
    // convenience API, including its original padding and trimming policy.
    fn allocating_reference(r: &mut StreamResampler, input: &[f32], flush: bool) -> Vec<f32> {
        for frame in input.chunks_exact(r.channels) {
            for (channel, sample) in frame.iter().enumerate() {
                r.residual[channel].push(*sample);
            }
        }
        let mut out = Vec::new();
        while r.residual[0].len() >= r.chunk_size_in || (flush && !r.residual[0].is_empty()) {
            let real_in = r.residual[0].len().min(r.chunk_size_in);
            for channel in &mut r.residual {
                if channel.len() < r.chunk_size_in {
                    channel.resize(r.chunk_size_in, 0.0);
                }
            }
            let input: Vec<Vec<f32>> = r
                .residual
                .iter_mut()
                .map(|channel| channel.drain(..r.chunk_size_in).collect())
                .collect();
            let chunk = rubato::Resampler::process(&mut r.inner, &input, None).unwrap();
            let frames = if real_in < r.chunk_size_in {
                ((real_in as f64 * r.out_rate as f64 / r.in_rate as f64).round() as usize)
                    .min(chunk[0].len())
            } else {
                chunk[0].len()
            };
            for frame in 0..frames {
                for channel in &chunk {
                    out.push(channel[frame]);
                }
            }
        }
        out
    }

    #[test]
    fn stream_resampler_matches_allocating_reference_across_packets_and_flush() {
        for (in_rate, out_rate) in [
            (44_100, 48_000),
            (48_000, 44_100),
            (96_000, 44_100),
            (44_100, 96_000),
        ] {
            for channels in [1, 2, 6] {
                let mut r = StreamResampler::new(in_rate, out_rate, channels).unwrap();
                let mut reference = StreamResampler::new(in_rate, out_rate, channels).unwrap();
                let capacities: Vec<_> = r.residual.iter().map(Vec::capacity).collect();
                let mut frame_offset = 0;
                for frames in [1, 1023, 1024, 1025, 8193, 7] {
                    let input: Vec<_> = (0..frames)
                        .flat_map(|frame| {
                            (0..channels).map(move |channel| {
                                let position = frame_offset + frame;
                                if position == 25 {
                                    0.75 - channel as f32 * 0.05
                                } else {
                                    ((position as f32 * 0.013).sin() * 0.4) + channel as f32 * 0.02
                                }
                            })
                        })
                        .collect();
                    frame_offset += frames;
                    let actual = r.process(&input);
                    let expected = allocating_reference(&mut reference, &input, false);
                    assert_eq!(
                        actual, expected,
                        "{in_rate}->{out_rate}, {channels} ch, {frames} frames"
                    );
                    assert!(
                        r.residual
                            .iter()
                            .all(|channel| channel.len() < r.chunk_size_in)
                    );
                    assert_eq!(
                        r.residual.iter().map(Vec::capacity).collect::<Vec<_>>(),
                        capacities,
                        "input scratch must stay bounded for large packets"
                    );
                }
                assert_eq!(r.flush(), allocating_reference(&mut reference, &[], true));
                assert!(r.flush().is_empty());
                assert!(r.process(&[]).is_empty());
            }
        }
    }

    #[test]
    fn stream_resampler_allocates_only_returned_packet_and_flush_buffers() {
        for (in_rate, out_rate, channels) in [
            (44_100, 48_000, 2),
            (96_000, 44_100, 1),
            (48_000, 96_000, 6),
        ] {
            let mut resampler = StreamResampler::new(in_rate, out_rate, channels).unwrap();
            let input = vec![0.25; (StreamResampler::CHUNK_SIZE_IN * 8 + 13) * channels];
            let (output, counts) =
                crate::playback::runtime::shared::test_measure_allocations(|| {
                    resampler.process(&input)
                });
            assert!(!output.is_empty());
            assert_eq!(
                counts,
                (1, 0),
                "packet scratch allocation for {in_rate}->{out_rate}, {channels} channels"
            );
            let (tail, counts) =
                crate::playback::runtime::shared::test_measure_allocations(|| resampler.flush());
            assert!(!tail.is_empty());
            assert_eq!(
                counts,
                (1, 0),
                "flush scratch allocation for {in_rate}->{out_rate}, {channels} channels"
            );
            assert!(resampler.flush().is_empty());
        }
    }

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
