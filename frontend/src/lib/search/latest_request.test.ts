import { describe, expect, test } from 'vitest';

import { createLatestRequestGate } from './latest_request';

describe('latest request gate', () => {
	test('starting a newer search aborts and invalidates the older search', () => {
		const gate = createLatestRequestGate();
		const older = gate.begin();
		const newer = gate.begin();

		expect(older.signal.aborted).toBe(true);
		expect(gate.isCurrent(older.token)).toBe(false);
		expect(newer.signal.aborted).toBe(false);
		expect(gate.isCurrent(newer.token)).toBe(true);
	});
});
