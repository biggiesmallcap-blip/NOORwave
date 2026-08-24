import { describe, expect, test } from 'vitest';
import { ARTIST_ENRICHMENT_DELAY_MS } from './artist_loading';

describe('artist loading policy', () => {
	test('gives core profile data a short head start before rich shelves', () => {
		expect(ARTIST_ENRICHMENT_DELAY_MS).toBeGreaterThanOrEqual(200);
		expect(ARTIST_ENRICHMENT_DELAY_MS).toBeLessThanOrEqual(500);
	});
});
