import { describe, expect, test } from 'vitest';
import { FOLD_SHARE, dialFamilies } from './families';

type Row = { id: string; no: number; family: string | null; appetite: number | null };

const row = (id: string, no: number, family: string | null, appetite: number | null = null): Row => ({ id, no, family, appetite });

// Hip hop's three warmest: (0.9 + 0.6 + 0.3) / 3 = 0.6. Rock: 0.4 alone.
// Country: 0.05. The unknown family lands under "More scenes".
const channels: Row[] = [
	row('scene:3', 31, 'hiphop', 0.9),
	row('wild', 2, null),
	row('scene:1', 10, 'rock', 0.4),
	row('your', 1, null),
	row('scene:2', 21, 'country', 0.05),
	row('scene:4', 17, 'hiphop', 0.6),
	row('scene:7', 19, 'hiphop', 0.3),
	row('scene:8', 20, 'hiphop', 0.01),
	row('scene:5', 0, 'hiphop', null),
	row('scene:6', 40, 'mystery', 0.35),
];

const family = (key: string, onAir: string | null = null, choices = {}) =>
	dialFamilies(channels, onAir, choices).families.find((f) => f.key === key)!;

describe('dial families', () => {
	test('fixed channels lead, then families by warmth, members by number', () => {
		const { main, families } = dialFamilies(channels);
		expect(main.map((c) => c.id)).toEqual(['your', 'wild']);
		expect(families.map((f) => f.key)).toEqual(['hiphop', 'rock', 'other', 'country']);
		expect(families[0].channels.map((c) => c.no)).toEqual([17, 19, 20, 31, 0]);
		expect(families[0].label).toBe('Hip Hop and R&B');
		expect(families[0].appetite).toBeCloseTo(0.6);
		expect(families[2].label).toBe('More scenes');
	});

	test('families well below the warmest fold, unless on air or opened', () => {
		// 0.6 * 0.5 = 0.3: rock (0.4) and more (0.35) stay open, country folds.
		expect(FOLD_SHARE).toBe(0.5);
		expect(family('rock').folded).toBe(false);
		expect(family('other').folded).toBe(false);
		expect(family('country').folded).toBe(true);
		expect(family('country', 'scene:2').folded).toBe(false);
		expect(family('country', null, { country: false }).folded).toBe(false);
		expect(family('rock', null, { rock: true }).folded).toBe(true);
	});

	test('a dial with no history yet folds nothing, and ties keep the family order', () => {
		const { families } = dialFamilies([row('a', 10, 'world', null), row('b', 11, 'latin', null)]);
		expect(families.map((f) => f.key)).toEqual(['latin', 'world']);
		expect(families.every((f) => !f.folded)).toBe(true);
	});
});
