import { describe, expect, test } from 'vitest';
import { dialFamilies } from './families';

type Row = { id: string; no: number; family: string | null };

const row = (id: string, no: number, family: string | null): Row => ({ id, no, family });

const channels: Row[] = [
	row('scene:3', 31, 'hiphop'),
	row('wild', 2, null),
	row('scene:1', 10, 'rock'),
	row('your', 1, null),
	row('scene:2', 21, 'country'),
	row('scene:4', 17, 'hiphop'),
	row('scene:5', 0, 'hiphop'),
	row('scene:6', 40, 'mystery'),
	row('scene:7', 14, 'latin'),
];

describe('dial families', () => {
	test('fixed channels lead, then the families in their fixed order, members by number', () => {
		const { main, families } = dialFamilies(channels);
		expect(main.map((c) => c.id)).toEqual(['your', 'wild']);
		expect(families.map((f) => f.key)).toEqual(['hiphop', 'latin', 'rock', 'country', 'other']);
		expect(families[0].channels.map((c) => c.no)).toEqual([17, 31, 0]);
		expect(families[0].label).toBe('Hip Hop and R&B');
		expect(families.at(-1)!.label).toBe('More scenes');
	});

	test('families with no channels are left out', () => {
		expect(dialFamilies([row('your', 1, null)]).families).toEqual([]);
	});
});
