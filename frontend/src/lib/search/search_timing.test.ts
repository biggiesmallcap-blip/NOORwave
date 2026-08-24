import { describe, expect, test } from 'vitest';

import { PRIMARY_SEARCH_DEBOUNCE_MS, SECONDARY_PROVIDER_DELAY_MS } from './search_timing';

describe('interactive search timing', () => {
	test('secondary providers require a longer quiet period than primary search', () => {
		expect(SECONDARY_PROVIDER_DELAY_MS).toBeGreaterThanOrEqual(PRIMARY_SEARCH_DEBOUNCE_MS * 3);
	});
});
