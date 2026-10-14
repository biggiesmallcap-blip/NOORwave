import { describe, expect, test } from 'vitest';
import { dropView, keepView, lastView } from './view_cache';

describe('video view cache', () => {
	test('keeps the last answer per page and forgets on request', () => {
		expect(lastView('logbook')).toBeNull();
		keepView('logbook', { count: 3 });
		expect(lastView<{ count: number }>('logbook')?.count).toBe(3);
		keepView('logbook', { count: 4 });
		expect(lastView<{ count: number }>('logbook')?.count).toBe(4);
		dropView('logbook');
		expect(lastView('logbook')).toBeNull();
	});
});
