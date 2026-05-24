pub fn camelot_distance(a: &str, b: &str) -> Option<u8> {
    let (a_number, a_mode) = parse_camelot(a)?;
    let (b_number, b_mode) = parse_camelot(b)?;
    let number_distance = {
        let raw = a_number.abs_diff(b_number);
        raw.min(12 - raw)
    };
    if a_mode == b_mode {
        Some(number_distance)
    } else if a_number == b_number {
        Some(7)
    } else {
        Some((number_distance + 7).min(12))
    }
}

pub fn bpm_delta_pct(a: f32, b: f32) -> f32 {
    if !a.is_finite() || !b.is_finite() || a <= 0.0 || b <= 0.0 {
        return f32::INFINITY;
    }
    ((a - b).abs() / a.min(b)) * 100.0
}

pub fn nearest_tempo_family_bpm(reference: f32, bpm: f32) -> f32 {
    if !reference.is_finite() || !bpm.is_finite() || reference <= 0.0 || bpm <= 0.0 {
        return bpm;
    }

    [bpm, bpm * 2.0, bpm / 2.0]
        .into_iter()
        .filter(|candidate| candidate.is_finite() && *candidate > 0.0)
        .min_by(|left, right| {
            bpm_delta_pct(reference, *left).total_cmp(&bpm_delta_pct(reference, *right))
        })
        .unwrap_or(bpm)
}

pub fn energy_delta(a: Option<f32>, b: Option<f32>) -> Option<f32> {
    Some((a? - b?).abs())
}

pub fn vocal_clash_score(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(left, right)| left.max(0.0) * right.max(0.0))
        .fold(0.0, f32::max)
}

fn parse_camelot(value: &str) -> Option<(u8, char)> {
    let trimmed = value.trim();
    if trimmed.len() < 2 {
        return None;
    }
    let (number, mode) = trimmed.split_at(trimmed.len() - 1);
    let number = number.parse::<u8>().ok()?;
    let mode = mode.chars().next()?.to_ascii_uppercase();
    (1..=12).contains(&number).then_some((number, mode))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camelot_distance_same_key_is_zero() {
        assert_eq!(camelot_distance("8A", "8A"), Some(0));
    }

    #[test]
    fn camelot_distance_adjacent_number_is_one() {
        assert_eq!(camelot_distance("8A", "9A"), Some(1));
    }

    #[test]
    fn camelot_distance_relative_major_minor_is_seven() {
        assert_eq!(camelot_distance("8A", "8B"), Some(7));
    }

    #[test]
    fn bpm_delta_pct_is_symmetric() {
        assert_eq!(bpm_delta_pct(120.0, 126.0), bpm_delta_pct(126.0, 120.0));
    }

    #[test]
    fn nearest_tempo_family_bpm_accepts_half_time_grid() {
        assert_eq!(nearest_tempo_family_bpm(124.0, 62.0), 124.0);
        assert_eq!(nearest_tempo_family_bpm(62.0, 124.0), 62.0);
    }

    #[test]
    fn vocal_clash_score_uses_max_product() {
        assert_eq!(vocal_clash_score(&[0.2, 0.5], &[0.5, 0.25]), 0.125);
    }
}
