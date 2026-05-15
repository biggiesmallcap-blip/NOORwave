import { describe, expect, test } from 'vitest';
import { createRemoteSearchGate, normalizeRemoteSearchQuery, shouldRunRemoteSearch } from './search';

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

describe('remote search gate', () => {
	test('a response is current until a newer search begins', () => {
		const gate = createRemoteSearchGate();
		const token = gate.begin();
		expect(gate.isCurrent(token)).toBe(true);
		const newer = gate.begin();
		expect(gate.isCurrent(token)).toBe(false);
		expect(gate.isCurrent(newer)).toBe(true);
	});

	test('clearing or shortening the query invalidates an in-flight request', () => {
		const gate = createRemoteSearchGate();
		// User types a valid query — a request goes out.
		const inFlight = gate.begin();
		// User deletes back to an invalid query before the response lands.
		gate.invalidate();
		// The late response must not be applied.
		expect(gate.isCurrent(inFlight)).toBe(false);
	});
});
