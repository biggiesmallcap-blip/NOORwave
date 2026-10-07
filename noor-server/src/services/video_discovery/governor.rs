//! The crawler's call budget: brisk while nothing plays, gentle during
//! playback, never a fixed-interval pattern.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, Instant};

use chrono::NaiveDate;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Idle,
    Active,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Active => "active",
        }
    }
}

pub const IDLE_SPACING: Duration = Duration::from_secs(2);
pub const ACTIVE_SPACING: Duration = Duration::from_secs(12);
pub const IDLE_PER_HOUR: usize = 500;
pub const ACTIVE_PER_HOUR: usize = 150;
pub const DAILY_CAP: usize = 6000;
const HOUR: Duration = Duration::from_secs(3600);
const VIDEO_ACTIVE_WINDOW_SECS: i64 = 600;

static LAST_VIDEO_ACTIVITY: AtomicI64 = AtomicI64::new(0);

/// Called when a video stream is requested or a watch is recorded.
pub fn note_video_activity() {
    LAST_VIDEO_ACTIVITY.store(chrono::Utc::now().timestamp(), Ordering::Relaxed);
}

pub fn mode_for(audio_active: bool, last_video_epoch: i64, now_epoch: i64) -> Mode {
    if audio_active || now_epoch - last_video_epoch < VIDEO_ACTIVE_WINDOW_SECS {
        Mode::Active
    } else {
        Mode::Idle
    }
}

pub fn current_mode(audio_active: bool) -> Mode {
    mode_for(
        audio_active,
        LAST_VIDEO_ACTIVITY.load(Ordering::Relaxed),
        chrono::Utc::now().timestamp(),
    )
}

/// `unit` in [0, 1] maps to 50%-150% of the base spacing.
pub fn jittered(base: Duration, unit: f64) -> Duration {
    base.mul_f64(0.5 + unit.clamp(0.0, 1.0))
}

#[derive(Debug, Default)]
pub struct Governor {
    calls: VecDeque<Instant>,
    day: Option<NaiveDate>,
    day_calls: usize,
    next_allowed: Option<Instant>,
}

impl Governor {
    pub fn new() -> Self {
        Self::default()
    }

    fn roll(&mut self, now: Instant, today: NaiveDate) {
        if self.day != Some(today) {
            self.day = Some(today);
            self.day_calls = 0;
        }
        while self
            .calls
            .front()
            .is_some_and(|at| now.saturating_duration_since(*at) >= HOUR)
        {
            self.calls.pop_front();
        }
    }

    /// How long to wait before `calls` more requests may go out; `None` means
    /// now. Urgent work skips the spacing but never the caps.
    pub fn wait(
        &mut self,
        now: Instant,
        today: NaiveDate,
        mode: Mode,
        calls: usize,
        urgent: bool,
    ) -> Option<Duration> {
        self.roll(now, today);
        if self.day_calls + calls > DAILY_CAP {
            return Some(Duration::from_secs(600));
        }
        let per_hour = match mode {
            Mode::Idle => IDLE_PER_HOUR,
            Mode::Active => ACTIVE_PER_HOUR,
        };
        if self.calls.len() + calls > per_hour {
            let oldest = self.calls.front().copied().unwrap_or(now);
            return Some(
                (oldest + HOUR)
                    .saturating_duration_since(now)
                    .max(Duration::from_secs(1)),
            );
        }
        if !urgent
            && let Some(next) = self.next_allowed
            && next > now
        {
            return Some(next - now);
        }
        None
    }

    pub fn record(
        &mut self,
        now: Instant,
        today: NaiveDate,
        mode: Mode,
        calls: usize,
        jitter_unit: f64,
    ) {
        self.roll(now, today);
        for _ in 0..calls {
            self.calls.push_back(now);
        }
        self.day_calls += calls;
        let spacing = match mode {
            Mode::Idle => IDLE_SPACING,
            Mode::Active => ACTIVE_SPACING,
        };
        self.next_allowed = Some(now + jittered(spacing * calls.max(1) as u32, jitter_unit));
    }

    pub fn calls_last_hour(&self) -> usize {
        self.calls.len()
    }

    pub fn calls_today(&self) -> usize {
        self.day_calls
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 7).unwrap()
    }

    #[test]
    fn spacing_depends_on_mode_and_jitter() {
        let start = Instant::now();
        let mut idle = Governor::new();
        assert_eq!(idle.wait(start, day(), Mode::Idle, 1, false), None);
        idle.record(start, day(), Mode::Idle, 1, 0.5);
        assert_eq!(
            idle.wait(start, day(), Mode::Idle, 1, false),
            Some(Duration::from_secs(2))
        );
        assert_eq!(
            idle.wait(start + Duration::from_secs(2), day(), Mode::Idle, 1, false),
            None
        );

        let mut active = Governor::new();
        active.record(start, day(), Mode::Active, 1, 0.0);
        assert_eq!(
            active.wait(start, day(), Mode::Active, 1, false),
            Some(Duration::from_secs(6))
        );
        assert_eq!(
            jittered(Duration::from_secs(12), 1.0),
            Duration::from_secs(18)
        );
    }

    #[test]
    fn hourly_and_daily_caps_hold_even_for_urgent_work() {
        let start = Instant::now();
        let mut governor = Governor::new();
        governor.record(start, day(), Mode::Active, ACTIVE_PER_HOUR, 0.0);
        assert!(governor.wait(start, day(), Mode::Active, 1, true).is_some());
        assert_eq!(
            governor.wait(start + HOUR, day(), Mode::Active, 1, true),
            None
        );

        let mut daily = Governor::new();
        daily.day = Some(day());
        daily.day_calls = DAILY_CAP;
        assert!(daily.wait(start, day(), Mode::Idle, 1, true).is_some());
        let tomorrow = day().succ_opt().unwrap();
        assert_eq!(daily.wait(start, tomorrow, Mode::Idle, 1, true), None);
    }

    #[test]
    fn urgent_work_skips_spacing() {
        let start = Instant::now();
        let mut governor = Governor::new();
        governor.record(start, day(), Mode::Idle, 1, 0.5);
        assert_eq!(governor.wait(start, day(), Mode::Idle, 1, true), None);
    }

    #[test]
    fn audio_or_recent_video_means_active() {
        assert_eq!(mode_for(true, 0, 10_000), Mode::Active);
        assert_eq!(mode_for(false, 9_500, 10_000), Mode::Active);
        assert_eq!(mode_for(false, 9_000, 10_000), Mode::Idle);
    }
}
