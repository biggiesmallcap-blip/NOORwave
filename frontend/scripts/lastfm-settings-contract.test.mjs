import { readFileSync } from 'node:fs';
import { describe, expect, test } from 'vitest';

describe('Last.fm settings actions', () => {
	test('keeps separate retry-untagged and refresh-all actions', () => {
		const source = readFileSync('src/routes/settings/+page.svelte', 'utf8');

		expect(source).toContain("startLastfmEnrichment('retry_untagged')");
		expect(source).toContain("startLastfmEnrichment('refresh')");
		expect(source).toContain('Retry untagged');
		expect(source).toContain('Recheck all tags');
	});
});
