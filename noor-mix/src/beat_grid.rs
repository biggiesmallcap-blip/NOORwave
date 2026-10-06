//! A fitted period describes measured markers, not confidence in unmeasured audio.

#[derive(Debug, Clone, Copy)]
pub struct BeatGridFit {
    pub period_seconds: f64,
    pub origin_seconds: f64,
    pub max_error_seconds: f64,
    pub measured_start_seconds: f64,
    pub measured_end_seconds: f64,
    pub inlier_count: usize,
}

pub fn fit_beat_grid(markers: &[f32]) -> Option<BeatGridFit> {
    // Bound work even for imported grids. The latest measured portion is more
    // relevant to a tail projection than an arbitrarily old opening.
    const MAX_MARKERS: usize = 16_384;
    const MAX_SLOPE_SPAN: usize = 32;
    let mut times = Vec::with_capacity(markers.len().min(MAX_MARKERS));
    for seconds in markers
        .iter()
        .skip(markers.len().saturating_sub(MAX_MARKERS))
    {
        let seconds = f64::from(*seconds);
        if seconds.is_finite()
            && seconds >= 0.0
            && times.last().is_none_or(|previous| seconds > *previous)
        {
            times.push(seconds);
        }
    }
    let seed = median(times.windows(2).map(|pair| pair[1] - pair[0]).collect())?;
    if seed < 0.02 {
        return None;
    }

    // Infer missing beat counts instead of equating marker count with beat
    // count. Near duplicates have zero ticks and disappear. Large/unrelated
    // gaps split runs: their unknown phase must not enter a whole-span mean.
    let mut best = Vec::new();
    let mut run = vec![(0.0, *times.first()?)];
    for seconds in times.iter().copied().skip(1) {
        let (previous_tick, previous_time) = *run.last()?;
        let delta = seconds - previous_time;
        if delta < seed * 0.25 {
            continue;
        }
        let ticks = (delta / seed).round().max(1.0);
        if ticks > 8.0 || (delta - ticks * seed).abs() > seed * 0.25 {
            if run.len() >= best.len() {
                best = run;
            }
            run = vec![(0.0, seconds)];
        } else {
            run.push((previous_tick + ticks, seconds));
        }
    }
    if run.len() >= best.len() {
        best = run;
    }
    if best.len() < 2 {
        return None;
    }

    // Multi-beat slopes average detector-frame quantization without letting
    // an isolated missing beat lower the tempo. At most one bounded slope per
    // marker is needed; a robust median supplies the initial period.
    let slopes = (0..best.len() - 1)
        .map(|index| {
            let end = (index + MAX_SLOPE_SPAN).min(best.len() - 1);
            (best[end].1 - best[index].1) / (best[end].0 - best[index].0)
        })
        .collect();
    let mut period = median(slopes)?;
    let mut origin = median(
        best.iter()
            .map(|(tick, seconds)| seconds - tick * period)
            .collect(),
    )?;
    let tolerance = (seed * 0.12).clamp(0.02, 0.12);
    // Reject timing outliers before refining, rather than deriving the rate
    // from the first/last point of a noisy or interrupted grid.
    best.retain(|(tick, seconds)| (seconds - origin - tick * period).abs() <= tolerance);
    if best.len() < 2 {
        return None;
    }
    for _ in 0..2 {
        (period, origin) = least_squares(&best)?;
        best.retain(|(tick, seconds)| (seconds - origin - tick * period).abs() <= tolerance);
        if best.len() < 2 {
            return None;
        }
    }
    if !period.is_finite() || period < 0.02 || !origin.is_finite() {
        return None;
    }
    let max_error = best
        .iter()
        .map(|(tick, seconds)| (seconds - origin - tick * period).abs())
        .fold(0.0_f64, f64::max);
    Some(BeatGridFit {
        period_seconds: period,
        origin_seconds: origin,
        max_error_seconds: max_error,
        measured_start_seconds: best.first()?.1,
        measured_end_seconds: best.last()?.1,
        inlier_count: best.len(),
    })
}

fn median(mut values: Vec<f64>) -> Option<f64> {
    values.retain(|value| value.is_finite());
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    Some(if values.len() % 2 == 0 {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    })
}

fn least_squares(points: &[(f64, f64)]) -> Option<(f64, f64)> {
    let count = points.len() as f64;
    let mean_tick = points.iter().map(|point| point.0).sum::<f64>() / count;
    let mean_time = points.iter().map(|point| point.1).sum::<f64>() / count;
    let variance = points
        .iter()
        .map(|(tick, _)| (tick - mean_tick).powi(2))
        .sum::<f64>();
    if variance <= 0.0 {
        return None;
    }
    let covariance = points
        .iter()
        .map(|(tick, seconds)| (tick - mean_tick) * (seconds - mean_time))
        .sum::<f64>();
    let period = covariance / variance;
    Some((period, mean_time - period * mean_tick))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quantized(bpm: f64, seconds: f64, phase: f64) -> Vec<f32> {
        (0..(seconds * bpm / 60.0) as usize)
            .map(|index| ((phase + index as f64 * 60.0 / bpm) * 100.0).round() as f32 / 100.0)
            .collect()
    }

    #[test]
    fn multi_beat_fit_prevents_quantized_121_122_rate_drift() {
        let a = fit_beat_grid(&quantized(121.0, 90.0, 0.13)).unwrap();
        let b = fit_beat_grid(&quantized(122.0, 90.0, 0.13)).unwrap();
        let rate = b.period_seconds / a.period_seconds;
        let correct_rate = 121.0 / 122.0;
        let drift = 24.0 * ((122.0 * rate / 121.0) - 1.0);
        assert!(
            drift.abs() < 0.01,
            "rate={rate}, ideal={correct_rate}, drift={drift}s"
        );
        assert!((60.0 / a.period_seconds - 121.0).abs() < 0.03);
        assert!((60.0 / b.period_seconds - 122.0).abs() < 0.03);
    }

    #[test]
    fn fractional_period_projects_without_cumulative_millisecond_rounding() {
        let fit = fit_beat_grid(&quantized(174.0, 90.0, 0.13)).unwrap();
        let index = ((270.0 - fit.origin_seconds) / fit.period_seconds).floor();
        let projected = fit.origin_seconds + index * fit.period_seconds;
        let actual = 0.13 + index * 60.0 / 174.0;
        assert!(
            (projected - actual).abs() < 0.02,
            "phase error={}",
            projected - actual
        );
    }

    #[test]
    fn fractional_bar_period_preserves_phase_after_three_hundred_bars() {
        let period = 1.90476495_f64;
        let markers = (0..48)
            .map(|bar| (0.37 + bar as f64 * period) as f32)
            .collect::<Vec<_>>();
        let fit = fit_beat_grid(&markers).unwrap();
        let actual = 0.37 + 300.0 * period;
        let projected = fit.origin_seconds + 300.0 * fit.period_seconds;
        assert!(
            (projected - actual).abs() < 0.001,
            "projected={projected}, actual={actual}"
        );
    }

    #[test]
    fn isolated_timing_outlier_does_not_tilt_the_period_or_origin() {
        let mut markers = (0..80)
            .map(|beat| 0.13 + beat as f32 * 0.5)
            .collect::<Vec<_>>();
        markers[38] += 0.1;
        let fit = fit_beat_grid(&markers).unwrap();
        assert!((fit.period_seconds - 0.5).abs() < 0.0001);
        assert!((fit.origin_seconds - 0.13).abs() < 0.001);
        assert!(fit.inlier_count < markers.len());
    }

    #[test]
    fn missed_beats_near_duplicates_and_local_jitter_remain_robust() {
        for markers in [
            vec![0.0, 0.5, 1.0, 2.0, 2.5, 3.0, 3.5],
            vec![0.0, 0.5, 1.0, 1.001, 1.5, 2.0, 2.5, 3.0],
            vec![0.0, 0.49, 1.0, 1.51, 2.0, 2.5],
        ] {
            let fit = fit_beat_grid(&markers).unwrap();
            assert!((60.0 / fit.period_seconds - 120.0).abs() < 0.5, "{fit:?}");
        }
        for markers in [
            vec![],
            vec![1.0],
            vec![2.0, 2.0],
            vec![5.0, 1.0],
            vec![0.0, f32::NAN],
        ] {
            assert!(fit_beat_grid(&markers).is_none());
        }
    }
}
