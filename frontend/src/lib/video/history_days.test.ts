import { describe, expect, test } from 'vitest';
import { dayLabel, historyDays, playsLine } from './history_days';

describe('history in days', () => {
	const now = new Date(2026, 9, 14, 18, 0);

	test('days read like a diary', () => {
		expect(dayLabel(new Date(2026, 9, 14, 1, 0), now)).toBe('Today');
		expect(dayLabel(new Date(2026, 9, 13, 23, 0), now)).toBe('Yesterday');
		expect(dayLabel(new Date(2026, 9, 12, 9, 0), now)).toBe('Monday');
		expect(dayLabel(new Date(2026, 8, 30, 9, 0), now)).toBe('30 September');
		expect(dayLabel(new Date(2025, 11, 31, 9, 0), now)).toBe('31 December 2025');
	});

	test('entries group by local day, newest first, order kept', () => {
		const at = (d: Date) => d.toISOString();
		const entries = [
			{ id: 1, watched_at: at(new Date(2026, 9, 14, 17, 0)) },
			{ id: 2, watched_at: at(new Date(2026, 9, 14, 9, 0)) },
			{ id: 3, watched_at: at(new Date(2026, 9, 13, 20, 0)) },
			{ id: 4, watched_at: 'nonsense' },
		];
		const days = historyDays(entries, now);
		expect(days.map((d) => d.label)).toEqual(['Today', 'Yesterday', 'Earlier']);
		expect(days[0].entries.map((e) => e.id)).toEqual([1, 2]);
	});

	test('plays are said in words', () => {
		expect(playsLine(1)).toBe('');
		expect(playsLine(2)).toBe('watched twice');
		expect(playsLine(5)).toBe('watched 5 times');
	});
});
