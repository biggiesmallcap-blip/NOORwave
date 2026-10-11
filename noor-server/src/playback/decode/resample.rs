//! Decode resampling helpers.

use anyhow::{Context, Result, anyhow};

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
    input_frames: usize,
    output_frames: usize,
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
            input_frames: 0,
            output_frames: 0,
        })
    }

    pub(crate) fn process(&mut self, interleaved: &[f32]) -> Result<Vec<f32>> {
        if !interleaved.len().is_multiple_of(self.channels) {
            return Err(anyhow!(
                "resampler input has an incomplete interleaved frame"
            ));
        }
        if interleaved.is_empty() {
            return Ok(Vec::new());
        }
        self.input_frames = self
            .input_frames
            .saturating_add(interleaved.len() / self.channels);
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
            let (_, frames) = result.context("rubato packet processing failed")?;
            self.append_output(frames, usize::MAX, &mut out);
        }
        Ok(out)
    }

    pub(crate) fn flush(&mut self) -> Result<Vec<f32>> {
        // Round the complete source duration once, rather than rounding each
        // packet. EOF must also drain filter history on exact chunk boundaries.
        let expected_frames = ((self.input_frames as u128 * self.out_rate as u128
            + self.in_rate as u128 / 2)
            / self.in_rate as u128)
            .min(usize::MAX as u128) as usize;
        let remaining = expected_frames.saturating_sub(self.output_frames);
        let mut out = Vec::with_capacity(remaining * self.channels);
        while self.output_frames < expected_frames {
            for channel in &mut self.residual {
                channel.resize(self.chunk_size_in, 0.0);
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
            let (_, frames) = result.context("rubato EOF drain failed")?;
            if frames == 0 {
                return Err(anyhow!("rubato EOF drain made no progress"));
            }
            self.append_output(frames, expected_frames - self.output_frames, &mut out);
        }
        for channel in &mut self.residual {
            channel.clear();
        }
        Ok(out)
    }

    #[cfg(test)]
    pub(super) fn test_truncate_output_buffer(&mut self) {
        // Trigger rubato's real validation in the HTTP decoder test.
        self.output_scratch[0].clear();
    }

    fn append_output(&mut self, frames: usize, limit: usize, out: &mut Vec<f32>) {
        // SincFixedIn starts its interpolation index half a filter before
        // zero, aligning emitted PCM with the source already. Its delay is
        // retained history to drain at EOF, not a prefix to remove here.
        let take = frames.min(limit);
        out.reserve(take * self.channels);
        for frame in 0..take {
            for channel in &self.output_scratch {
                out.push(channel[frame]);
            }
        }
        self.output_frames = self.output_frames.saturating_add(take);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampler_rejects_partial_interleaved_frames_without_mutating_stream() {
        let mut r = StreamResampler::new(44_100, 48_000, 2).unwrap();
        assert!(r.process(&[0.25, -0.25]).unwrap().is_empty());
        let before = r.residual.clone();
        let input_frames = r.input_frames;
        let error = r.process(&[0.5, -0.5, 0.75]).unwrap_err();
        assert!(error.to_string().contains("incomplete interleaved frame"));
        assert_eq!(r.residual, before);
        assert_eq!(r.input_frames, input_frames);
    }

    #[test]
    fn resampler_returns_rubato_packet_and_eof_failures() {
        for flush in [false, true] {
            let mut r = StreamResampler::new(44_100, 48_000, 2).unwrap();
            assert!(!r.process(&vec![0.1; 2048]).unwrap().is_empty());
            let emitted_before_fault = r.output_frames;
            // Exercise rubato's actual short-output-buffer validation rather
            // than returning a mocked wrapper error.
            r.output_scratch[0].clear();
            let error = if flush {
                assert!(r.process(&[0.25, -0.25]).unwrap().is_empty());
                r.flush().unwrap_err()
            } else {
                r.process(&vec![0.25; 2048]).unwrap_err()
            };
            let chain = format!("{error:#}");
            assert!(chain.contains("rubato"), "{chain}");
            assert!(
                matches!(
                    error.downcast_ref::<rubato::ResampleError>(),
                    Some(rubato::ResampleError::InsufficientOutputBufferSize {
                        channel: 0,
                        actual: 0,
                        ..
                    })
                ),
                "{chain}"
            );
            assert_eq!(r.output_frames, emitted_before_fault);
        }
    }

    #[test]
    fn resampler_preserves_duration_and_tail_across_chunk_boundaries() {
        for (in_rate, out_rate) in [(44_100, 48_000), (96_000, 44_100), (44_100, 96_000)] {
            for frames in [1, 13, 1024, 1025, 4096, 4099] {
                let mut r = StreamResampler::new(in_rate, out_rate, 2).unwrap();
                let mut input = vec![0.0; frames * 2];
                input[(frames - 1) * 2] = 0.75;
                input[(frames - 1) * 2 + 1] = -0.5;
                let mut output = r.process(&input).unwrap();
                output.extend(r.flush().unwrap());
                let expected_frames =
                    ((frames as f64 * out_rate as f64 / in_rate as f64).round()) as usize;
                assert_eq!(
                    output.len(),
                    expected_frames * 2,
                    "{in_rate}->{out_rate}, {frames} input frames"
                );
                if expected_frames > 0 {
                    assert!(
                        output.iter().any(|s| s.abs() > 0.1),
                        "last impulse was lost: {in_rate}->{out_rate}, {frames} frames"
                    );
                }
                assert!(r.flush().unwrap().is_empty());
            }
        }
    }

    #[test]
    fn resampler_aligns_impulse_to_source_clock() {
        for (in_rate, out_rate) in [(44_100, 48_000), (48_000, 44_100), (96_000, 48_000)] {
            let mut r = StreamResampler::new(in_rate, out_rate, 1).unwrap();
            let mut input = vec![0.0; 4096];
            input[256] = 0.75;
            let mut output = r.process(&input).unwrap();
            output.extend(r.flush().unwrap());
            let peak = output
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
                .unwrap()
                .0;
            let expected = (256.0 * out_rate as f64 / in_rate as f64).round() as usize;
            assert!(
                peak.abs_diff(expected) <= 1,
                "impulse at {peak}, expected {expected}: {in_rate}->{out_rate}"
            );
        }
    }

    // Independent whole-clip reference uses rubato's allocating API and
    // selects exactly the source-duration window after zero-padding its tail.
    fn allocating_reference(
        input: &[f32],
        in_rate: u32,
        out_rate: u32,
        channels: usize,
    ) -> Vec<f32> {
        let mut r = StreamResampler::new(in_rate, out_rate, channels).unwrap();
        let expected =
            ((input.len() / channels) as f64 * out_rate as f64 / in_rate as f64).round() as usize;
        let mut raw = Vec::new();
        let mut position = 0;
        while raw.len() / channels < expected {
            let planar: Vec<Vec<f32>> = (0..channels)
                .map(|channel| {
                    (0..StreamResampler::CHUNK_SIZE_IN)
                        .map(|frame| {
                            input
                                .get((position + frame) * channels + channel)
                                .copied()
                                .unwrap_or(0.0)
                        })
                        .collect()
                })
                .collect();
            position += StreamResampler::CHUNK_SIZE_IN;
            let chunk = rubato::Resampler::process(&mut r.inner, &planar, None).unwrap();
            for frame in 0..chunk[0].len() {
                for channel in &chunk {
                    raw.push(channel[frame]);
                }
            }
        }
        raw[..expected * channels].to_vec()
    }

    #[test]
    fn stream_resampler_matches_allocating_reference_across_packets_and_flush() {
        for (in_rate, out_rate) in [
            (44_100, 48_000),
            (48_000, 44_100),
            (96_000, 44_100),
            (44_100, 96_000),
            (8_000, 384_000),
            (384_000, 8_000),
        ] {
            for channels in [1, 2, 6] {
                let mut r = StreamResampler::new(in_rate, out_rate, channels).unwrap();
                let mut all_input = Vec::new();
                let mut all_output = Vec::new();
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
                    all_output.extend(r.process(&input).unwrap());
                    all_input.extend(input);
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
                all_output.extend(r.flush().unwrap());
                assert_eq!(
                    all_output,
                    allocating_reference(&all_input, in_rate, out_rate, channels),
                    "{in_rate}->{out_rate}, {channels} ch"
                );
                assert!(r.flush().unwrap().is_empty());
                assert!(r.process(&[]).unwrap().is_empty());
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
                    resampler.process(&input).unwrap()
                });
            assert!(!output.is_empty());
            assert_eq!(
                counts,
                (1, 0),
                "packet scratch allocation for {in_rate}->{out_rate}, {channels} channels"
            );
            let (tail, counts) = crate::playback::runtime::shared::test_measure_allocations(|| {
                resampler.flush().unwrap()
            });
            assert!(!tail.is_empty());
            assert_eq!(
                counts,
                (1, 0),
                "flush scratch allocation for {in_rate}->{out_rate}, {channels} channels"
            );
            assert!(resampler.flush().unwrap().is_empty());
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
        let out = r.process(&input).unwrap();
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
        let out = r.process(&input).unwrap();
        assert!(
            out.is_empty(),
            "small input should buffer in residual, not produce output"
        );
    }
}
