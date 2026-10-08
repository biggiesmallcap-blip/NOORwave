import { describe, expect, test } from 'vitest';
import { existsSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const path = (rel) => resolve(import.meta.dirname, rel);
const read = (rel) => readFileSync(path(rel), 'utf8');

// Design audit, October 2026: Automix and DJ are one Mix page. The header
// owns both switches; one Diagnostics disclosure holds both engines' details.
describe('Mix page', () => {
	test('one route hosts Automix and DJ', () => {
		const mix = read('../src/routes/mix/+page.svelte');
		expect(mix).toContain('<AutomixPanel');
		expect(mix).toContain('<DjCockpit />');
		expect(mix).toContain('<DjCockpit part="controls" />');
		expect(mix).toContain('aria-label="Automix"');
		expect(mix).toContain('aria-label="DJ transitions"');
	});

	test('the old routes redirect permanently', () => {
		for (const route of ['automix', 'dj']) {
			expect(existsSync(path(`../src/routes/${route}/+page.svelte`))).toBe(false);
			expect(read(`../src/routes/${route}/+page.ts`)).toContain("redirect(308, '/mix')");
		}
	});

	test('the sidebar has one Mix item', () => {
		const registry = JSON.parse(read('../src/lib/routes/registry-data.json'));
		const nav = JSON.parse(read('../src/lib/routes/navigation-data.json'));
		expect(registry.mix.path).toBe('/mix');
		expect(registry.automix).toBeUndefined();
		expect(registry.dj).toBeUndefined();
		expect(nav.navigationZones.find((zone) => zone.label === 'Tools').routeIds).toContain('mix');
	});

	test('one Diagnostics disclosure holds both engines', () => {
		const mix = read('../src/routes/mix/+page.svelte');
		const panel = read('../src/lib/components/mix/AutomixPanel.svelte');
		const cockpit = read('../src/lib/components/dj-cockpit/DjCockpit.svelte');
		expect(panel.match(/<summary>Diagnostics<\/summary>/g)?.length).toBe(1);
		expect(panel).toContain('{@render diagnostics?.()}');
		expect(mix).toContain('<DjDiagnostics />');
		expect(cockpit).not.toContain('<details class="disclosure">');
	});

	test('transition style sits with mix intent and speed', () => {
		const mixIntent = read('../src/lib/components/dj-cockpit/MixIntentControl.svelte');
		expect(mixIntent).toContain('<Dropdown label="Transition style"');
		expect(mixIntent).toContain('<Segmented label="Mix intent"');
	});
});
