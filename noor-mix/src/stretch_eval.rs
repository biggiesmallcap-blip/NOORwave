#[derive(Debug, Clone, PartialEq)]
pub struct StretchEvaluationReport {
    pub sample_rate: u32,
    pub channels: u16,
    pub tempo_ratio: f32,
    pub input_frames: usize,
    pub output_frames: usize,
    pub expected_output_frames: usize,
    pub length_error_frames: i64,
    pub finite: bool,
    pub non_finite_samples: usize,
    pub peak: f32,
    pub rms_change_db: f32,
    pub phase_marker_count: usize,
    pub phase_marker_matched: usize,
    pub max_phase_drift_ms: f32,
}

impl StretchEvaluationReport {
    pub fn passes_objective_gate(&self, tempo_delta_pct: f32) -> bool {
        if !self.finite || self.peak > 0.98 || self.length_error_frames.abs() > 1 {
            return false;
        }
        if self.phase_marker_count != self.phase_marker_matched {
            return false;
        }
        let max_phase_drift_ms = if tempo_delta_pct <= 5.001 {
            10.0
        } else if tempo_delta_pct <= 8.001 {
            20.0
        } else {
            return false;
        };
        self.max_phase_drift_ms <= max_phase_drift_ms
    }
}

pub fn evaluate_stretch_render(
    input: &[f32],
    output: &[f32],
    sample_rate: u32,
    channels: u16,
    tempo_ratio: f32,
    source_phase_markers: &[usize],
) -> StretchEvaluationReport {
    let channels_usize = usize::from(channels.max(1));
    let input_frames = input.len() / channels_usize;
    let output_frames = output.len() / channels_usize;
    let expected_output_frames = if tempo_ratio.is_finite() && tempo_ratio > 0.0 {
        (input_frames as f32 / tempo_ratio).round().max(0.0) as usize
    } else {
        0
    };
    let finite = output.iter().all(|sample| sample.is_finite());
    let non_finite_samples = output.iter().filter(|sample| !sample.is_finite()).count();
    let peak = output
        .iter()
        .filter(|sample| sample.is_finite())
        .fold(0.0_f32, |acc, sample| acc.max(sample.abs()));
    let rms_change_db = rms_change_db(input, output);
    let (phase_marker_matched, max_phase_drift_ms) = phase_drift_ms(
        output,
        sample_rate,
        channels_usize,
        tempo_ratio,
        source_phase_markers,
    );

    StretchEvaluationReport {
        sample_rate,
        channels: channels.max(1),
        tempo_ratio,
        input_frames,
        output_frames,
        expected_output_frames,
        length_error_frames: output_frames as i64 - expected_output_frames as i64,
        finite,
        non_finite_samples,
        peak,
        rms_change_db,
        phase_marker_count: source_phase_markers.len(),
        phase_marker_matched,
        max_phase_drift_ms,
    }
}

#[cfg(feature = "signalsmith-eval")]
pub fn render_signalsmith_stretch(
    input: &[f32],
    sample_rate: u32,
    channels: u16,
    tempo_ratio: f32,
) -> Option<Vec<f32>> {
    if !tempo_ratio.is_finite() || tempo_ratio <= 0.0 {
        return None;
    }
    let channels = channels.max(1);
    let channels_usize = usize::from(channels);
    if input.len() % channels_usize != 0 {
        return None;
    }
    let input_frames = input.len() / channels_usize;
    let output_frames = (input_frames as f32 / tempo_ratio).round().max(1.0) as usize;
    let output_samples = output_frames.checked_mul(channels_usize)?;
    let mut output = vec![0.0_f32; output_samples];
    let mut stretch =
        signalsmith_stretch::Stretch::preset_default(u32::from(channels), sample_rate.max(1));
    stretch.exact(input, &mut output).then_some(output)
}

fn phase_drift_ms(
    output: &[f32],
    sample_rate: u32,
    channels: usize,
    tempo_ratio: f32,
    source_phase_markers: &[usize],
) -> (usize, f32) {
    if source_phase_markers.is_empty() || !tempo_ratio.is_finite() || tempo_ratio <= 0.0 {
        return (0, 0.0);
    }
    let search_radius_frames = (sample_rate as usize / 20).max(1);
    let mut matched = 0_usize;
    let mut max_drift_ms = 0.0_f32;
    for source_frame in source_phase_markers {
        let expected = (*source_frame as f32 / tempo_ratio).round().max(0.0) as usize;
        let Some(found) = nearest_transient_frame(output, channels, expected, search_radius_frames)
        else {
            continue;
        };
        matched += 1;
        let drift_frames = found.abs_diff(expected);
        let drift_ms = drift_frames as f32 * 1_000.0 / sample_rate.max(1) as f32;
        max_drift_ms = max_drift_ms.max(drift_ms);
    }
    if matched == source_phase_markers.len() {
        (matched, max_drift_ms)
    } else {
        (matched, f32::INFINITY)
    }
}

fn nearest_transient_frame(
    output: &[f32],
    channels: usize,
    expected: usize,
    search_radius_frames: usize,
) -> Option<usize> {
    let frame_count = output.len() / channels.max(1);
    if frame_count == 0 {
        return None;
    }
    let start = expected.saturating_sub(search_radius_frames);
    let end = expected
        .saturating_add(search_radius_frames)
        .saturating_add(1)
        .min(frame_count);
    let mut best_frame = None;
    let mut best_amp = 0.0_f32;
    for frame in start..end {
        let base = frame * channels;
        let amp = output[base..base + channels]
            .iter()
            .filter(|sample| sample.is_finite())
            .map(|sample| sample.abs())
            .sum::<f32>()
            / channels as f32;
        if amp > best_amp {
            best_amp = amp;
            best_frame = Some(frame);
        }
    }
    (best_amp > 1e-4).then_some(best_frame?)
}

fn rms_change_db(input: &[f32], output: &[f32]) -> f32 {
    let input_rms = rms(input).max(1e-9);
    let output_rms = rms(output).max(1e-9);
    20.0 * (output_rms / input_rms).log10()
}

fn rms(samples: &[f32]) -> f32 {
    let mut sum_sq = 0.0_f32;
    let mut count = 0_usize;
    for sample in samples.iter().copied().filter(|sample| sample.is_finite()) {
        sum_sq += sample * sample;
        count += 1;
    }
    if count == 0 {
        return 0.0;
    }
    (sum_sq / count as f32).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn click_track(
        sample_rate: u32,
        channels: u16,
        seconds: usize,
        bpm: f32,
    ) -> (Vec<f32>, Vec<usize>) {
        let channels_usize = usize::from(channels.max(1));
        let frames = sample_rate as usize * seconds;
        let mut samples = vec![0.0; frames * channels_usize];
        let interval_frames = (60.0 * sample_rate as f32 / bpm).round().max(1.0) as usize;
        let markers = (0..frames)
            .step_by(interval_frames)
            .inspect(|frame| {
                for channel in 0..channels_usize {
                    samples[*frame * channels_usize + channel] = 0.9;
                }
            })
            .collect::<Vec<_>>();
        (samples, markers)
    }

    fn render_click_markers(
        source_markers: &[usize],
        source_frames: usize,
        sample_rate: u32,
        channels: u16,
        tempo_ratio: f32,
        drift_frames: isize,
    ) -> Vec<f32> {
        let channels_usize = usize::from(channels.max(1));
        let output_frames = (source_frames as f32 / tempo_ratio).round().max(1.0) as usize;
        let mut output = vec![0.0; output_frames * channels_usize];
        for source_frame in source_markers {
            let frame = (*source_frame as f32 / tempo_ratio).round() as isize + drift_frames;
            if frame < 0 || frame as usize >= output_frames {
                continue;
            }
            for channel in 0..channels_usize {
                output[frame as usize * channels_usize + channel] = 0.9;
            }
        }
        let noise_period = (sample_rate / 10).max(1) as usize;
        for frame in (0..output_frames).step_by(noise_period) {
            for channel in 0..channels_usize {
                output[frame * channels_usize + channel] += 0.01;
            }
        }
        output
    }

    fn print_eval_row(
        renderer: &str,
        seconds: usize,
        tempo_ratio: f32,
        render_ms: u128,
        report: &StretchEvaluationReport,
    ) {
        let delta_pct = (tempo_ratio - 1.0).abs() * 100.0;
        println!(
            "smart_stretch_eval renderer={renderer} window_secs={seconds} tempo_ratio={tempo_ratio:.3} render_ms={render_ms} finite={} length_error_frames={} peak={:.3} rms_change_db={:.2} phase_markers={}/{} max_phase_drift_ms={:.3} passed={}",
            report.finite,
            report.length_error_frames,
            report.peak,
            report.rms_change_db,
            report.phase_marker_matched,
            report.phase_marker_count,
            report.max_phase_drift_ms,
            report.passes_objective_gate(delta_pct)
        );
    }

    #[test]
    fn stretch_eval_tracks_length_and_phase_for_ideal_clicks() {
        let sample_rate = 48_000;
        let channels = 2;
        let tempo_ratio = 1.05;
        let (input, markers) = click_track(sample_rate, channels, 30, 120.0);
        let output = render_click_markers(
            &markers,
            input.len() / usize::from(channels),
            sample_rate,
            channels,
            tempo_ratio,
            0,
        );

        let report = evaluate_stretch_render(
            &input,
            &output,
            sample_rate,
            channels,
            tempo_ratio,
            &markers,
        );

        assert!(report.finite);
        assert_eq!(report.length_error_frames, 0);
        assert!(report.max_phase_drift_ms < 0.1, "{report:?}");
        assert!(report.passes_objective_gate(5.0), "{report:?}");
    }

    #[test]
    fn stretch_eval_rejects_excess_phase_drift() {
        let sample_rate = 48_000;
        let channels = 2;
        let tempo_ratio = 1.05;
        let (input, markers) = click_track(sample_rate, channels, 30, 120.0);
        let output = render_click_markers(
            &markers,
            input.len() / usize::from(channels),
            sample_rate,
            channels,
            tempo_ratio,
            800,
        );

        let report = evaluate_stretch_render(
            &input,
            &output,
            sample_rate,
            channels,
            tempo_ratio,
            &markers,
        );

        assert!(report.max_phase_drift_ms > 10.0, "{report:?}");
        assert!(!report.passes_objective_gate(5.0));
    }

    #[test]
    fn stretch_eval_counts_non_finite_samples() {
        let input = vec![0.0; 16];
        let output = vec![0.0, f32::NAN, 0.2, 0.3];

        let report = evaluate_stretch_render(&input, &output, 48_000, 1, 1.0, &[]);

        assert!(!report.finite);
        assert_eq!(report.non_finite_samples, 1);
        assert!(!report.passes_objective_gate(3.0));
    }

    #[test]
    #[ignore]
    fn smart_stretch_evaluation_baseline_benchmark() {
        let sample_rate = 48_000;
        let channels = 2;
        for seconds in [30_usize, 90, 180] {
            let (input, markers) = click_track(sample_rate, channels, seconds, 120.0);
            for tempo_ratio in [0.88_f32, 0.92, 0.95, 0.97, 1.03, 1.05, 1.08, 1.12] {
                let started = Instant::now();
                let output = render_click_markers(
                    &markers,
                    input.len() / usize::from(channels),
                    sample_rate,
                    channels,
                    tempo_ratio,
                    0,
                );
                let render_ms = started.elapsed().as_millis();
                let report = evaluate_stretch_render(
                    &input,
                    &output,
                    sample_rate,
                    channels,
                    tempo_ratio,
                    &markers,
                );
                print_eval_row(
                    "synthetic_baseline",
                    seconds,
                    tempo_ratio,
                    render_ms,
                    &report,
                );

                #[cfg(feature = "signalsmith-eval")]
                {
                    let started = Instant::now();
                    let output =
                        render_signalsmith_stretch(&input, sample_rate, channels, tempo_ratio)
                            .expect("signalsmith render");
                    let render_ms = started.elapsed().as_millis();
                    let report = evaluate_stretch_render(
                        &input,
                        &output,
                        sample_rate,
                        channels,
                        tempo_ratio,
                        &markers,
                    );
                    print_eval_row("signalsmith", seconds, tempo_ratio, render_ms, &report);
                }
            }
        }
    }

    #[cfg(feature = "signalsmith-eval")]
    #[test]
    fn signalsmith_renderer_uses_frame_based_output_size() {
        let sample_rate = 48_000;
        let channels = 2;
        let tempo_ratio = 1.05;
        let (input, markers) = click_track(sample_rate, channels, 30, 120.0);

        let output =
            render_signalsmith_stretch(&input, sample_rate, channels, tempo_ratio).expect("render");
        let report = evaluate_stretch_render(
            &input,
            &output,
            sample_rate,
            channels,
            tempo_ratio,
            &markers,
        );

        assert!(report.finite, "{report:?}");
        assert!(report.length_error_frames.abs() <= 1, "{report:?}");
        assert!(report.peak <= 0.98, "{report:?}");
    }
}
