import { describe, expect, test } from 'vitest';
import { normalizeRemoteSearchQuery, shouldRunRemoteSearch } from './search';

describe('remote search helpers', () => {
	test('normalizes whitespace', () => {
		expect(normalizeRemoteSearchQuery('  amy   shark  ')).toBe('amy shark');
	});

	test('requires two useful characters', () => {
		expect(shouldRunRemoteSearch('a')).toBe(false);
		expect(shouldRunRemoteSearch('  a  ')).toBe(false);
		expect(shouldRunRemoteSearch('am')).toBe(true);
	});
});
