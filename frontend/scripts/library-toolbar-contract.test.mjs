import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const page = readFileSync(resolve(import.meta.dirname, '../src/routes/library/+page.svelte'), 'utf8');

describe('Library command header and toolbar', () => {
	test('the shared header, tabs and toolbar replace the pill row', () => {
		expect(page).toContain('<CommandHeader');
		expect(page).toContain('<ScopeTabs tabs={libraryTabs} current={activeTab} label="Library views" onselect={(id) => switchTab(id as LibraryTab)} />');
		expect(page).not.toContain('class="filter-pills"');
		expect(page).not.toContain('class="filter-pill"');
	});

	test('albums use the shared sort dropdown, decade chips and layout switch', () => {
		expect(page).toContain('<Dropdown label="Sort albums"');
		expect(page).toContain('<Segmented label="Album layout"');
		expect(page).toMatch(/<FilterChip pressed=\{activeDecade === decade\}/);
		expect(page).not.toContain('class="decade-chip"');
	});
});
