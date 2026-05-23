use crate::program::Curve;

pub fn eval_curve(curve: Curve, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    match curve {
        Curve::Linear => t,
        Curve::EqualPowerIn => (t * std::f32::consts::FRAC_PI_2).sin(),
        Curve::EqualPowerOut => (t * std::f32::consts::FRAC_PI_2).cos(),
        Curve::Cosine => 0.5 - 0.5 * (std::f32::consts::PI * t).cos(),
    }
}

pub fn interpolate(from: f32, to: f32, curve: Curve, t: f32) -> f32 {
    from + (to - from) * eval_curve(curve, t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_has_expected_boundaries() {
        assert_eq!(eval_curve(Curve::Linear, 0.0), 0.0);
        assert_eq!(eval_curve(Curve::Linear, 1.0), 1.0);
    }

    #[test]
    fn equal_power_midpoint_squares_sum_to_one() {
        let a = eval_curve(Curve::EqualPowerOut, 0.5);
        let b = eval_curve(Curve::EqualPowerIn, 0.5);
        let sum = a * a + b * b;
        assert!((sum - 1.0).abs() < 1e-6, "sum was {sum}");
    }

    #[test]
    fn cosine_is_smooth_and_centered() {
        assert!((eval_curve(Curve::Cosine, 0.5) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn interpolate_uses_curve_output() {
        assert!((interpolate(10.0, 20.0, Curve::Linear, 0.25) - 12.5).abs() < 1e-6);
    }
}
