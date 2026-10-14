import { describe, expect, test } from 'vitest';
import { channelNumber, forYouLabel, formatClock, formatRuntime, liveSlot, progressOf, secondsSinceMidnight, tuneStart, unlockLabel, unlockProgress } from './clock';
import { FIXTURE_NOW_S, fixtureChannels } from './fixtures';

const [bay, your, kpop] = fixtureChannels.channels;

describe('tune-in offset', () => {
	test('joins the scheduled video at its offset', () => {
		expect(tuneStart(95.5, 240)).toEqual({ skip: false, startAt: 95.5 });
		expect(tuneStart(0, 240)).toEqual({ skip: false, startAt: 0 });
	});

	test('within its first 20 s it starts from the top', () => {
		expect(tuneStart(19.9, 240)).toEqual({ skip: false, startAt: 0 });
		expect(tuneStart(20, 240)).toEqual({ skip: false, startAt: 20 });
	});

	test('past 75% of the runtime skips to the next item', () => {
		expect(tuneStart(180, 240)).toEqual({ skip: true, startAt: 0 });
		expect(tuneStart(230, 240)).toEqual({ skip: true, startAt: 0 });
		// Just under the line still joins.
		expect(tuneStart(179.9, 240).skip).toBe(false);
	});

	test('from the start always begins the video on now at 0:00', () => {
		expect(tuneStart(95.5, 240, 'start')).toEqual({ skip: false, startAt: 0 });
		expect(tuneStart(230, 240, 'start')).toEqual({ skip: false, startAt: 0 });
	});

	test('without a runtime it joins wherever the schedule is; junk offsets start at 0', () => {
		expect(tuneStart(500, null)).toEqual({ skip: false, startAt: 500 });
		expect(tuneStart(500, 0)).toEqual({ skip: false, startAt: 500 });
		expect(tuneStart(-4, 240)).toEqual({ skip: false, startAt: 0 });
		expect(tuneStart(Number.NaN, 240)).toEqual({ skip: false, startAt: 0 });
	});
});

describe('live progress', () => {
	test('moves through now in real time from offset_s', () => {
		const at0 = liveSlot(bay, 0);
		expect(at0.program?.video.title).toBe('Feelin Myself');
		expect(at0.offset).toBe(60);
		expect(at0.progress).toBeCloseTo(60 / 210);
		const later = liveSlot(bay, 45);
		expect(later.offset).toBe(105);
		expect(later.progress).toBeCloseTo(0.5);
		expect(later.next?.video.title).toBe('Tell Me When to Go');
		expect(later.stale).toBe(false);
	});

	test('when now runs out, next takes over and asks for a refetch', () => {
		// Your Channel: 200 s into a 242 s video; 50 s later it is 8 s into next.
		const slot = liveSlot(your, 50);
		expect(slot.program?.video.title).toBe('Wake Up (Live at Coachella)');
		expect(slot.offset).toBeCloseTo(8);
		expect(slot.next).toBeNull();
		expect(slot.stale).toBe(true);
	});

	test('with nothing after it, the last picture holds full', () => {
		const slot = liveSlot(kpop, 1000);
		expect(slot.progress).toBe(1);
		expect(slot.stale).toBe(true);
	});

	test('a channel with nothing on is empty, not stale', () => {
		expect(liveSlot({ now: null, next: null }, 30)).toEqual({ program: null, offset: 0, progress: 0, next: null, then: null, stale: false });
	});

	test('when now runs out, then moves up to next', () => {
		const then = your.next ? { ...your.next, slot: 99, start_s: your.next.start_s + your.next.duration_s } : null;
		const slot = liveSlot({ ...your, then }, 50);
		expect(slot.next?.slot).toBe(99);
		expect(slot.then).toBeNull();
		expect(liveSlot({ ...your, then }, 0).then?.slot).toBe(99);
	});

	test('a locked scene says what it still lacks, hours or artists', () => {
		expect(unlockLabel({ hours: 12.7, need_hours: 30 })).toBe('12 of 30 hours');
		const artistsShort = { hours: 40, need_hours: 30, artists: 28, need_artists: 40 };
		expect(unlockLabel(artistsShort)).toBe('28 of 40 artists');
		expect(unlockProgress(artistsShort)).toBeCloseTo(0.7);
		const hoursShort = { hours: 6, need_hours: 30, artists: 35, need_artists: 40 };
		expect(unlockLabel(hoursShort)).toBe('6 of 30 hours');
		expect(unlockProgress(hoursShort)).toBeCloseTo(0.2);
		expect(unlockLabel(null)).toBe('');
	});

	test('the strip is named for the time of day', () => {
		expect(forYouLabel(8 * 3600)).toBe('For you this morning');
		expect(forYouLabel(13 * 3600 + 23 * 60)).toBe('For you this afternoon');
		expect(forYouLabel(21 * 3600)).toBe('For you tonight');
		expect(forYouLabel(2 * 3600)).toBe('For you tonight');
		expect(forYouLabel(86_400 + 9 * 3600)).toBe('For you this morning');
	});

	test('progress clamps and survives a zero runtime', () => {
		expect(progressOf(-5, 100)).toBe(0);
		expect(progressOf(150, 100)).toBe(1);
		expect(progressOf(10, 0)).toBe(0);
	});

	test('unlock progress is hours over hours needed', () => {
		expect(unlockProgress({ hours: 12, need_hours: 30 })).toBeCloseTo(0.4);
		expect(unlockProgress({ hours: 40, need_hours: 30 })).toBe(1);
		expect(unlockProgress(null)).toBe(0);
	});
});

describe('formatting', () => {
	test('guide times and runtimes', () => {
		expect(formatClock(FIXTURE_NOW_S + 150)).toBe('21:02');
		expect(formatClock(86_400 + 60)).toBe('00:01');
		expect(formatRuntime(252)).toBe('4:12');
		expect(formatRuntime(3730)).toBe('1:02:10');
		expect(formatRuntime(null)).toBe('');
		expect(channelNumber(3)).toBe('03');
		expect(channelNumber(112)).toBe('112');
	});

	test('seconds since local midnight', () => {
		expect(secondsSinceMidnight(new Date(2026, 9, 14, 21, 30, 15))).toBe(21 * 3600 + 30 * 60 + 15);
	});
});
