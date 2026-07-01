import { readFileSync } from 'node:fs';
import { describe, expect, test } from 'vitest';

const source = readFileSync('src/routes/settings/+page.svelte', 'utf8').replace(/\r\n/g, '\n');

describe('settings category contract', () => {
	test('keeps Discovery engine inside the Audio settings category', () => {
		expect(source).not.toContain("| 'discovery'");
		expect(source).not.toContain("id: 'discovery'");
		expect(source).not.toContain("cat.id === 'discovery'");
		expect(source).not.toContain("activeCategory === 'discovery'");
		expect(source).toContain("{#if activeCategory === 'audio'}\n\t\t\t<section data-setting-id=\"discovery-engine\" class=\"glass-panel section-panel\">\n\t\t\t\t<SectionHeader eyebrow=\"Learning\" title=\"Discovery engine\"");
	});
});
