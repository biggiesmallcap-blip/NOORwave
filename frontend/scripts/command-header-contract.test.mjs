import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const read = (rel) => readFileSync(resolve(import.meta.dirname, rel), 'utf8');
const header = read('../src/lib/components/ui/CommandHeader.svelte');
const videos = read('../src/routes/videos/+layout.svelte');

// STYLING.md "Command header": one header for search-led pages. Back (when
// given) and the field share the first row, the field centred at
// --measure-command; scope tabs, then the toolbar, sit under it.
describe('CommandHeader', () => {
	test('back sits beside the field, tabs and toolbar follow in order', () => {
		const back = header.indexOf('class="back-link"');
		const field = header.indexOf('{@render field()}');
		const tabs = header.indexOf('{@render tabs()}');
		const toolbar = header.indexOf('{@render toolbar()}');
		expect(back).toBeGreaterThan(-1);
		expect(back).toBeLessThan(field);
		expect(field).toBeLessThan(tabs);
		expect(tabs).toBeLessThan(toolbar);
	});

	test('the field is centred at the command measure', () => {
		expect(header).toContain('grid-template-columns: 1fr minmax(0, var(--measure-command)) 1fr;');
		expect(header).toMatch(/\.search-slot\s*\{[^}]*grid-column: 2;/);
	});

	test('the toolbar row has a fixed height so tabs never shift the content', () => {
		expect(header).toMatch(/\.toolbar\s*\{[^}]*min-height: 36px;/);
	});
});

describe('Videos mounts the command header', () => {
	test('the layout no longer carries its own header markup', () => {
		expect(videos).toContain("import CommandHeader from '$lib/components/ui/CommandHeader.svelte';");
		expect(videos).not.toContain('<header');
		expect(videos).not.toContain('class="search-row"');
	});

	test('the gap under the header is the shared header gap', () => {
		expect(videos).toMatch(/\.video-section\s*\{[^}]*gap: var\(--header-gap\);/);
	});
});
