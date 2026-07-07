use crate::program::{AutomationEvent, Curve, Param};

/// Easing fraction in [0, 1] with f(0) = 0 and f(1) = 1 for every curve, so
/// `interpolate` always starts at `from` and ends at `to`.
///
/// The equal-power pair is defined so that a fade-out event (from=1, to=0,
/// EqualPowerOut) evaluates to cos(t*pi/2) while a fade-in event (from=0,
/// to=1, EqualPowerIn) evaluates to sin(t*pi/2); their squares sum to 1 at
/// every point of the crossfade.
pub fn eval_curve(curve: Curve, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    match curve {
        Curve::Linear => t,
        Curve::EqualPowerIn => (t * std::f32::consts::FRAC_PI_2).sin(),
        Curve::EqualPowerOut => 1.0 - (t * std::f32::consts::FRAC_PI_2).cos(),
        Curve::Cosine => 0.5 - 0.5 * (std::f32::consts::PI * t).cos(),
    }
}

pub fn interpolate(from: f32, to: f32, curve: Curve, t: f32) -> f32 {
    from + (to - from) * eval_curve(curve, t)
}

/// Value of one automation event at an absolute frame position: None before
/// the event starts, held at `to` after it ends, interpolated inside.
pub fn event_value_at(event: &AutomationEvent, sample: u64) -> Option<f32> {
    if sample < event.start_sample {
        return None;
    }
    if sample >= event.end_sample {
        return Some(event.to);
    }
    let span = (event.end_sample - event.start_sample) as f32;
    let t = (sample - event.start_sample) as f32 / span;
    Some(interpolate(event.from, event.to, event.curve, t))
}

/// Resolve a parameter at an absolute frame position by folding every event
/// targeting it, in program order. Parameters default to 1.0 (unity gain /
/// unity rate) until their first event begins.
pub fn param_value_at(automation: &[AutomationEvent], param: Param, sample: u64) -> f32 {
    automation
        .iter()
        .filter(|event| event.param == param)
        .fold(1.0, |value, event| {
            event_value_at(event, sample).unwrap_or(value)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::DeckId;

    #[test]
    fn linear_has_expected_boundaries() {
        assert_eq!(eval_curve(Curve::Linear, 0.0), 0.0);
        assert_eq!(eval_curve(Curve::Linear, 1.0), 1.0);
    }

    #[test]
    fn every_curve_is_a_valid_easing() {
        for curve in [
            Curve::Linear,
            Curve::EqualPowerIn,
            Curve::EqualPowerOut,
            Curve::Cosine,
        ] {
            assert!(eval_curve(curve, 0.0).abs() < 1e-6, "{curve:?} at 0");
            assert!(
                (eval_curve(curve, 1.0) - 1.0).abs() < 1e-6,
                "{curve:?} at 1"
            );
        }
    }

    #[test]
    fn equal_power_fade_pair_conserves_energy() {
        for step in 0..=16 {
            let t = step as f32 / 16.0;
            let fade_out = interpolate(1.0, 0.0, Curve::EqualPowerOut, t);
            let fade_in = interpolate(0.0, 1.0, Curve::EqualPowerIn, t);
            let sum = fade_out * fade_out + fade_in * fade_in;
            assert!((sum - 1.0).abs() < 1e-5, "t={t}: sum was {sum}");
        }
    }

    #[test]
    fn cosine_is_smooth_and_centered() {
        assert!((eval_curve(Curve::Cosine, 0.5) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn interpolate_uses_curve_output() {
        assert!((interpolate(10.0, 20.0, Curve::Linear, 0.25) - 12.5).abs() < 1e-6);
    }

    #[test]
    fn param_value_at_defaults_to_unity_before_first_event() {
        let events = [AutomationEvent {
            param: Param::LowGain(DeckId::A),
            start_sample: 100,
            end_sample: 200,
            from: 1.0,
            to: 0.0,
            curve: Curve::Linear,
        }];
        assert_eq!(param_value_at(&events, Param::LowGain(DeckId::A), 50), 1.0);
        assert_eq!(param_value_at(&events, Param::LowGain(DeckId::A), 200), 0.0);
        assert_eq!(param_value_at(&events, Param::MidGain(DeckId::A), 150), 1.0);
    }

    #[test]
    fn param_value_at_holds_event_end_value_and_chains_events() {
        let events = [
            AutomationEvent {
                param: Param::LowGain(DeckId::B),
                start_sample: 0,
                end_sample: 100,
                from: 0.0,
                to: 0.0,
                curve: Curve::Linear,
            },
            AutomationEvent {
                param: Param::LowGain(DeckId::B),
                start_sample: 100,
                end_sample: 200,
                from: 0.0,
                to: 1.0,
                curve: Curve::Linear,
            },
        ];
        assert_eq!(param_value_at(&events, Param::LowGain(DeckId::B), 50), 0.0);
        assert_eq!(param_value_at(&events, Param::LowGain(DeckId::B), 100), 0.0);
        assert!((param_value_at(&events, Param::LowGain(DeckId::B), 150) - 0.5).abs() < 1e-6);
        assert_eq!(param_value_at(&events, Param::LowGain(DeckId::B), 300), 1.0);
    }
}
