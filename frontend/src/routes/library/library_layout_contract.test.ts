import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, '+page.svelte'), 'utf8');

function cssBlock(selector: string): string {
	const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
	const match = source.match(new RegExp(`${escaped}\\s*\\{(?<body>[^}]*)\\}`));
	if (!match?.groups?.body) {
		throw new Error(`Missing CSS block for ${selector}`);
	}
	return match.groups.body;
}

describe('library layout contracts', () => {
	test('primary category pills stay centered under the search input', () => {
		expect(source).toContain('class="filter-pill-group filter-pill-group--primary"');
		expect(source).toContain('class="filter-pill-actions"');
		expect(source).toContain('class="library-search-meta"');
		const primaryStart = source.indexOf('class="filter-pill-group filter-pill-group--primary"');
		const actionsStart = source.indexOf('class="filter-pill-actions"');
		const randomStart = source.indexOf('title="Random play"');
		const metaStart = source.indexOf('class="library-search-meta"');
		const statusStart = source.indexOf('class="library-status"');
		expect(primaryStart).toBeGreaterThan(-1);
		expect(actionsStart).toBeGreaterThan(primaryStart);
		expect(randomStart).toBeGreaterThan(primaryStart);
		expect(randomStart).toBeLessThan(actionsStart);
		expect(metaStart).toBeGreaterThan(actionsStart);
		expect(statusStart).toBeGreaterThan(metaStart);

		const row = cssBlock('.filter-pills');
		expect(row).toContain('max-width: 720px');
		expect(row).toContain('margin: 0 auto');
		expect(row).toContain('grid-template-columns: minmax(0, 1fr) auto minmax(0, 1fr)');

		const primary = cssBlock('.filter-pill-group--primary');
		expect(primary).toContain('grid-column: 2');
		expect(primary).toContain('justify-content: center');

		const meta = cssBlock('.library-search-meta');
		expect(meta).toContain('min-height');
		expect(meta).toContain('justify-content: center');
	});
});
