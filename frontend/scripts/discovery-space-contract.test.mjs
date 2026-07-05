import { readFileSync } from 'node:fs';
import { describe, expect, test } from 'vitest';

const STORE = 'src/lib/components/DiscoverSpace/discover_space_store.ts';
const TYPES = 'src/lib/components/DiscoverSpace/discover_space_types.ts';
const ADAPTER = 'src/lib/components/DiscoverSpace/discover_space_adapter.ts';
const PAGE = 'src/routes/discoverspace/+page.svelte';
const FILTER_BAR = 'src/lib/components/DiscoverSpace/DiscoverFilterBar.svelte';

describe('discovery space controls contract', () => {
	test('store sends coherence, session_id, and filters on space and blend requests', () => {
		const store = readFileSync(STORE, 'utf8');
		expect(store).toContain('function controlRequestFields()');
		expect(store).toContain('coherence: s.coherence');
		expect(store).toContain("session_id: s.sessionId || undefined");
		expect(store).toContain('filters: isFilterNoop(s.filters) ? undefined : s.filters');
		// Both request builders spread the same control fields, so the WS-driven
		// reload (handleDiscoverySpaceRefreshed -> loadSpace) inherits the user's
		// controls instead of silently resetting them.
		const spreadCount = store.split('...controlRequestFields()').length - 1;
		expect(spreadCount).toBeGreaterThanOrEqual(2);
		expect(store).toContain('export function handleDiscoverySpaceRefreshed');
	});

	test('controls persist in sessionStorage and hydrate before the first load', () => {
		const store = readFileSync(STORE, 'utf8');
		expect(store).toContain("'discoverspace.controls.v1'");
		expect(store).toContain("'discoverspace.session.v1'");
		expect(store).toContain('export function hydrateDiscoverControls');

		const page = readFileSync(PAGE, 'utf8');
		const mountBody = page.slice(page.indexOf('onMount(() => {'));
		expect(mountBody.indexOf('hydrateDiscoverControls()')).toBeGreaterThan(-1);
		expect(mountBody.indexOf('hydrateDiscoverControls()')).toBeLessThan(
			mountBody.indexOf('loadSpace(')
		);
	});

	test('coherence slider debounces reloads and filters reload immediately', () => {
		const store = readFileSync(STORE, 'utf8');
		expect(store).toContain('export function setCoherence');
		expect(store).toContain('coherenceReloadTimer = setTimeout(');
		expect(store).toContain('export function setFilters');
		expect(store).toContain('reloadActiveSpace()');
	});

	test('node types and adapter carry why-related and shaped score fields', () => {
		const types = readFileSync(TYPES, 'utf8');
		expect(types).toContain('why?: string;');
		expect(types).toContain('whySignals?: string[];');
		expect(types).toContain('shapedScore?: number;');
		expect(types).toContain('export interface DiscoverFilters');
		expect(types).toContain('export function isFilterNoop');

		const adapter = readFileSync(ADAPTER, 'utf8');
		expect(adapter).toContain('shapedScore: api.shaped_score');
		expect(adapter).toContain('whySignals: api.why_signals');
	});

	test('filter bar exposes every backend filter and warns about key-only semantics', () => {
		const bar = readFileSync(FILTER_BAR, 'utf8');
		for (const field of [
			'bpm_min',
			'bpm_max',
			'energy_min',
			'energy_max',
			'key_compatible_only',
			'year_min',
			'year_max',
			'exclude_in_library',
			'exclude_heard_session',
		]) {
			expect(bar).toContain(field);
		}
		expect(bar).toContain('hides unanalyzed and external tracks');
		expect(bar).toContain('setCoherence');
		const page = readFileSync(PAGE, 'utf8');
		expect(page).toContain('<DiscoverFilterBar />');
	});
});
