import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const page = readFileSync(resolve(import.meta.dirname, '../src/routes/library/+page.svelte'), 'utf8');

describe('Library route motion', () => {
	test('Library does not apply page-level translate animation on entry', () => {
		expect(page).toContain('class="page-shell library"');
		expect(page).not.toContain('class="page-shell library animate-in"');
	});
});
