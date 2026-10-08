import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const read = (rel) => readFileSync(resolve(import.meta.dirname, rel), 'utf8');
const chip = read('../src/lib/components/ui/FilterChip.svelte');
const duplicates = read('../src/routes/duplicates/+page.svelte');

// STYLING.md "FilterChip": "is this filter on?". A toggle button; on is the
// soft accent fill with accent-strong text and the accent hairline.
describe('FilterChip', () => {
	test('is a toggle button', () => {
		expect(chip).toContain('type="button"');
		expect(chip).toContain('aria-pressed={pressed}');
	});

	test('on is accent-soft fill, accent-strong text, accent-line border', () => {
		const on = chip.match(/\.filter-chip\[aria-pressed='true'\]\s*\{([^}]*)\}/)?.[1] ?? '';
		expect(on).toContain('background: var(--accent-soft);');
		expect(on).toContain('border-color: var(--accent-line);');
		expect(on).toContain('color: var(--accent-strong);');
	});

	test('off is an outline pill that fills on hover', () => {
		const off = chip.match(/\.filter-chip\s*\{([^}]*)\}/)?.[1] ?? '';
		expect(off).toContain('border: 1px solid var(--border-subtle);');
		expect(off).toContain('border-radius: 999px;');
		// Hover only fills an off chip, so an on chip keeps its accent.
		expect(chip).toMatch(/\.filter-chip\[aria-pressed='false'\]:hover:not\(:disabled\)\s*\{[^}]*background: var\(--bg-hover\);/);
	});
});

describe('Duplicates relationship filters use FilterChip', () => {
	test('the chips mount the shared control; the local recipe is gone', () => {
		expect(duplicates).toContain('<FilterChip pressed={active} onclick={() => toggleRelationship(rel)}>');
		expect(duplicates).not.toContain('class="filter-chip"');
		expect(duplicates).not.toMatch(/\.filter-chip\s*\{/);
	});
});
