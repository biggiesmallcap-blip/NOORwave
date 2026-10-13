//! The daily clock: a channel day is four 6-hour blocks of local time, each
//! seeded by (channel, day, block). Times are seconds since local midnight of
//! the day they belong to; a day's last program may run past 86400, and the
//! next day's schedule still starts at 0.

use chrono::{Local, NaiveDate, Timelike};

use super::programmer::Program;
use crate::services::video_sets::build_seed;

pub const DAY_S: i64 = 86_400;
pub const BLOCK_S: i64 = 6 * 3_600;
pub const BLOCKS: usize = 4;
const DAY_FORMAT: &str = "%Y-%m-%d";

/// Stable across runs and releases (FNV-1a), unlike std's hasher.
pub fn seed(channel: &str, day: &str, block: usize) -> u64 {
    build_seed(channel, &format!("{day}/{block}"))
}

/// Today's local date and the seconds since local midnight.
pub fn local_now() -> (String, i64) {
    let now = Local::now();
    (
        now.date_naive().format(DAY_FORMAT).to_string(),
        i64::from(now.time().num_seconds_from_midnight()),
    )
}

pub fn parse_day(day: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(day, DAY_FORMAT).ok()
}

pub fn shift_day(day: &str, days: i64) -> Option<String> {
    let date = parse_day(day)?;
    let shifted = date.checked_add_signed(chrono::Duration::days(days))?;
    Some(shifted.format(DAY_FORMAT).to_string())
}

pub fn days_since_epoch(day: &str) -> Option<i64> {
    let date = parse_day(day)?;
    let epoch = NaiveDate::from_ymd_opt(1970, 1, 1)?;
    Some((date - epoch).num_days())
}

/// The program airing at `t` and how far into it. Programs are sorted by
/// start and back to back; `None` before the first or after the last ends.
pub fn locate(programs: &[Program], t: i64) -> Option<(usize, i64)> {
    let after = programs.partition_point(|p| p.start_s <= t);
    let index = after.checked_sub(1)?;
    let program = &programs[index];
    (t < program.end_s()).then_some((index, t - program.start_s))
}

/// Today's programs followed by tomorrow's, shifted onto today's clock. Only
/// tomorrow's programs still airing when today's tail ends are kept, so the
/// timeline stays in step with the wall clock across midnight.
pub fn join(today: Vec<Program>, tomorrow: Vec<Program>) -> Vec<Program> {
    let tail_end = today.last().map_or(DAY_S, Program::end_s).max(DAY_S);
    let mut out = today;
    out.extend(tomorrow.into_iter().filter_map(|mut program| {
        program.start_s += DAY_S;
        (program.end_s() > tail_end).then_some(program)
    }));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn program(slot: i64, start_s: i64, duration_s: i64) -> Program {
        Program {
            slot,
            start_s,
            duration_s,
            video_id: 100 + slot,
            why: String::new(),
        }
    }

    /// Back to back from `start` with the given lengths.
    fn run(start: i64, lengths: &[i64]) -> Vec<Program> {
        let mut cursor = start;
        lengths
            .iter()
            .enumerate()
            .map(|(slot, length)| {
                let p = program(slot as i64, cursor, *length);
                cursor += length;
                p
            })
            .collect()
    }

    #[test]
    fn seeds_differ_by_channel_day_and_block_and_never_drift() {
        let a = seed("wild", "2026-10-14", 0);
        assert_eq!(a, seed("wild", "2026-10-14", 0));
        assert_ne!(a, seed("wild", "2026-10-14", 1));
        assert_ne!(a, seed("wild", "2026-10-15", 0));
        assert_ne!(a, seed("your", "2026-10-14", 0));
    }

    #[test]
    fn four_blocks_of_six_hours_make_a_day() {
        assert_eq!(BLOCK_S * BLOCKS as i64, DAY_S);
    }

    #[test]
    fn a_program_running_over_a_block_boundary_is_found_with_its_offset() {
        // The last program of block 0 starts at 21500 and runs 300 s, so
        // block 1 begins at 21800, not 21600.
        let programs = vec![
            program(0, 21_200, 300),
            program(1, 21_500, 300),
            program(2, 21_800, 200),
        ];
        assert_eq!(locate(&programs, BLOCK_S), Some((1, 100)));
        assert_eq!(locate(&programs, 21_799), Some((1, 299)));
        assert_eq!(locate(&programs, 21_800), Some((2, 0)));
        assert_eq!(locate(&programs, 21_199), None);
        assert_eq!(locate(&programs, 22_000), None);
    }

    #[test]
    fn midnight_hands_over_to_tomorrow_in_step_with_the_clock() {
        // Today's tail starts at 86200 and runs to 86500 (100 s past midnight).
        let today = vec![program(0, 85_900, 300), program(1, 86_200, 300)];
        // Tomorrow starts at 0 regardless: 0-240 still airs at 100 s past
        // midnight, 240-480 follows.
        let tomorrow = run(0, &[240, 240, 240]);
        let timeline = join(today, tomorrow);
        assert_eq!(locate(&timeline, 86_390), Some((1, 190)));
        let next = &timeline[2];
        assert_eq!((next.start_s, next.duration_s), (DAY_S, 240));
        assert_eq!(timeline.len(), 5);
        assert_eq!(locate(&timeline, DAY_S + 300), Some((3, 60)));
        // A tail that ends exactly at midnight keeps tomorrow whole.
        let exact = join(vec![program(0, 86_100, 300)], run(0, &[240, 240]));
        assert_eq!(exact.len(), 3);
        assert_eq!(exact[1].start_s, DAY_S);
        // A long tail (a concert) skips what tomorrow aired meanwhile.
        let long = join(vec![program(0, 86_000, 700)], run(0, &[240, 240, 240]));
        assert_eq!(long.len(), 3, "0-240 ended before the concert did");
        assert_eq!(long[1].start_s, DAY_S + 240);
    }

    #[test]
    fn days_shift_across_months_and_years() {
        assert_eq!(shift_day("2026-12-31", 1).as_deref(), Some("2027-01-01"));
        assert_eq!(shift_day("2026-03-01", -1).as_deref(), Some("2026-02-28"));
        assert_eq!(shift_day("nope", 1), None);
        assert_eq!(days_since_epoch("1970-01-02"), Some(1));
    }
}
