import { afterEach, describe, expect, test, vi } from 'vitest';
import { createFlipper, dialOrder, digitVerdict, flipTarget } from './flip';
import { fixtureChannels } from './fixtures';

const channels = fixtureChannels.channels;
const numbers = channels.map((channel) => channel.no);

describe('flipping', () => {
	test('the dial is unlocked channels in number order', () => {
		expect(dialOrder(channels).map((channel) => channel.no)).toEqual([1, 6, 12, 14]);
	});

	test('Up and Down step along the dial, skip locked channels and wrap', () => {
		expect(flipTarget(channels, 1, 1)?.no).toBe(6);
		// 7 Rewind is locked: 6 goes straight to 12.
		expect(flipTarget(channels, 6, 1)?.no).toBe(12);
		expect(flipTarget(channels, 14, 1)?.no).toBe(1);
		expect(flipTarget(channels, 1, -1)?.no).toBe(14);
		expect(flipTarget(channels, 12, -1)?.no).toBe(6);
		// From a number that is not on the dial: where it would sit.
		expect(flipTarget(channels, 7, 1)?.no).toBe(12);
		expect(flipTarget(channels, null, -1)?.no).toBe(14);
		expect(flipTarget([], 3, 1)).toBeNull();
	});

	test('typed numbers commit as soon as only one channel fits', () => {
		// "1" could still become 12 or 14.
		expect(digitVerdict('1', numbers)).toBe('wait');
		expect(digitVerdict('12', numbers)).toBe('commit');
		expect(digitVerdict('6', numbers)).toBe('commit');
		expect(digitVerdict('06', numbers)).toBe('commit');
		expect(digitVerdict('9', numbers)).toBe('invalid');
		expect(digitVerdict('13', numbers)).toBe('invalid');
		// Locked channels are still numbers on the dial (the stage says so).
		expect(digitVerdict('21', numbers)).toBe('commit');
		expect(digitVerdict('2', numbers)).toBe('wait');
	});
});

describe('debounced flips', () => {
	afterEach(() => {
		vi.useRealTimers();
	});

	test('a held rocker previews every channel and tunes only the last', () => {
		vi.useFakeTimers();
		const previews: number[] = [];
		const tuned: number[] = [];
		const flipper = createFlipper({
			channels: () => channels,
			currentNo: () => 1,
			preview: (channel) => previews.push(channel.no),
			tune: (channel) => tuned.push(channel.no),
			settleMs: 300,
		});
		flipper.press(1);
		vi.advanceTimersByTime(120);
		flipper.press(1);
		vi.advanceTimersByTime(120);
		flipper.press(1);
		expect(previews).toEqual([6, 12, 14]);
		expect(tuned).toEqual([]);
		expect(flipper.pending?.no).toBe(14);
		vi.advanceTimersByTime(300);
		expect(tuned).toEqual([14]);
		expect(flipper.pending).toBeNull();
	});

	test('after a tune the next press starts from what is on air', () => {
		vi.useFakeTimers();
		let onAir = 12;
		const tuned: number[] = [];
		const flipper = createFlipper({
			channels: () => channels,
			currentNo: () => onAir,
			preview: () => {},
			tune: (channel) => {
				tuned.push(channel.no);
				onAir = channel.no;
			},
			settleMs: 300,
		});
		flipper.press(-1);
		vi.advanceTimersByTime(300);
		flipper.press(-1);
		vi.advanceTimersByTime(300);
		expect(tuned).toEqual([6, 1]);
	});

	test('cancel drops a pending flip', () => {
		vi.useFakeTimers();
		const tune = vi.fn();
		const flipper = createFlipper({ channels: () => channels, currentNo: () => 1, preview: () => {}, tune, settleMs: 300 });
		flipper.press(1);
		flipper.cancel();
		vi.advanceTimersByTime(1000);
		expect(tune).not.toHaveBeenCalled();
	});
});
