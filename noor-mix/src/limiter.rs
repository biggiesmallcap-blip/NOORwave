pub struct SafetyLimiter {
    ceiling: f32,
}

impl SafetyLimiter {
    pub fn new(ceiling: f32) -> Self {
        Self {
            ceiling: ceiling.abs(),
        }
    }

    pub fn process_in_place(&mut self, block: &mut [f32]) {
        let ceiling = if self.ceiling.is_finite() && self.ceiling > 0.0 {
            self.ceiling
        } else {
            1.0
        };
        for sample in block {
            *sample = sample.clamp(-ceiling, ceiling);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limiter_clamps_positive_transient() {
        let mut limiter = SafetyLimiter::new(0.8);
        let mut block = [0.2, 1.2];
        limiter.process_in_place(&mut block);
        assert_eq!(block, [0.2, 0.8]);
    }

    #[test]
    fn limiter_clamps_negative_transient() {
        let mut limiter = SafetyLimiter::new(0.8);
        let mut block = [-0.2, -1.2];
        limiter.process_in_place(&mut block);
        assert_eq!(block, [-0.2, -0.8]);
    }

    #[test]
    fn limiter_leaves_quiet_signal_unchanged() {
        let mut limiter = SafetyLimiter::new(0.8);
        let mut block = [-0.2, 0.0, 0.2];
        limiter.process_in_place(&mut block);
        assert_eq!(block, [-0.2, 0.0, 0.2]);
    }
}
