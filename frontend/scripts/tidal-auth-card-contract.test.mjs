import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const source = readFileSync(resolve(import.meta.dirname, '../src/routes/settings/+page.svelte'), 'utf8');

describe('TIDAL auth card', () => {
	test('does not print the raw TIDAL authorize URL in settings', () => {
		expect(source).not.toContain('<a class="verify-link" href={verifyUrl} target="_blank">{verifyUrl}</a>');
		expect(source).toContain('Open TIDAL sign-in');
		expect(source).toContain('href={verifyUrl}');
	});
});
