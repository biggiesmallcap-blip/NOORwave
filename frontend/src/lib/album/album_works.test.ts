import { describe, expect, test } from 'vitest';
import { groupWorks } from './album_works';

describe('groupWorks', () => {
	test('consecutive movements of one work form a group and show only the movement', () => {
		const layout = groupWorks([
			'Suite No. 1 in G Major, BWV 1007: I. Prelude',
			'Suite No. 1 in G Major, BWV 1007: II. Allemande',
			'Suite No. 2 in D Minor, BWV 1008: I. Prelude',
			'Suite No. 2 in D Minor, BWV 1008: II. Allemande',
		]);
		expect(layout.groups).toEqual([
			{ work: 'Suite No. 1 in G Major, BWV 1007', start: 0, end: 1 },
			{ work: 'Suite No. 2 in D Minor, BWV 1008', start: 2, end: 3 },
		]);
		expect(layout.displayTitles).toEqual(['I. Prelude', 'II. Allemande', 'I. Prelude', 'II. Allemande']);
	});

	test('a leading composer prefix stays in the work, the movement is split at the last colon', () => {
		const layout = groupWorks([
			'J.S. Bach: Suite No. 1, BWV 1007: I. Prelude',
			'J.S. Bach: Suite No. 1, BWV 1007: II. Allemande',
		]);
		expect(layout.groups[0].work).toBe('J.S. Bach: Suite No. 1, BWV 1007');
		expect(layout.displayTitles).toEqual(['I. Prelude', 'II. Allemande']);
	});

	test('ordinary albums stay flat, including a lone colon title', () => {
		const titles = ['Creep', 'Remix: Club Edit', 'Karma Police'];
		expect(groupWorks(titles)).toEqual({ groups: [], displayTitles: titles });
	});
});
