import { describe, expect, test } from 'vitest';
import { initials } from './text';

describe('initials', () => {
	test('builds up to two initials from words', () => {
		expect(initials('Daft Punk')).toBe('DP');
		expect(initials('burial')).toBe('B');
		expect(initials('  a tribe called quest  ')).toBe('AT');
	});

	test('falls back when no initials can be found', () => {
		expect(initials('')).toBe('?');
		expect(initials('   ')).toBe('?');
	});
});
