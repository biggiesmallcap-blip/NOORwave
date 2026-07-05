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

	test('like/skip post feedback then rerank with pre-shaping base scores', () => {
		const store = readFileSync(STORE, 'utf8');
		expect(store).toContain('export function likeNode');
		expect(store).toContain('export function skipNode');
		const feedbackIdx = store.indexOf('/api/discovery/feedback');
		const rerankIdx = store.indexOf('/api/discovery/rerank');
		expect(feedbackIdx).toBeGreaterThan(-1);
		expect(rerankIdx).toBeGreaterThan(feedbackIdx);
		// base_score must be the pre-shaping raw score so shaping runs once.
		expect(store).toContain('base_score: n.rawScore ?? n.score');
		// Merge touches rerankScore/why only - canvas layout stays put.
		expect(store).toContain('rerankScore: row.score');
	});

	test('ranked list panel queues through the pending-queue pipeline', () => {
		const store = readFileSync(STORE, 'utf8');
		expect(store).toContain('export async function queueSpaceTracks');
		expect(store).toContain('/api/discovery/space/queue');

		const list = readFileSync(
			'src/lib/components/DiscoverSpace/DiscoverRankedList.svelte',
			'utf8'
		);
		expect(list).toContain('ArtworkImage');
		expect(list).toContain('size={320}');
		expect(list).toContain('Play all');
		expect(list).toContain('Queue all');
		expect(list).toContain("key_bpm: 'Key+BPM'");
		expect(list).toContain('likeNode');
		expect(list).toContain('skipNode');

		const page = readFileSync(PAGE, 'utf8');
		expect(page).toContain('<DiscoverRankedList');
	});

	test('why summary surfaces in hover card and side panel', () => {
		const hover = readFileSync(
			'src/lib/components/DiscoverSpace/DiscoverHoverCard.svelte',
			'utf8'
		);
		expect(hover).toContain('node.why');
		const panel = readFileSync(
			'src/lib/components/DiscoverSpace/DiscoverSidePanel.svelte',
			'utf8'
		);
		expect(panel).toContain('node.why');
	});

	test('branching keeps a persisted walk-back path', () => {
		const store = readFileSync(STORE, 'utf8');
		expect(store).toContain('export function branchHere');
		expect(store).toContain('export function walkBack');
		expect(store).toContain("'discoverspace.branch.v1'");
		// Hydration restores the tree position via the locked seed.
		const hydrate = store.slice(
			store.indexOf('export function hydrateDiscoverControls'),
			store.indexOf('function reloadActiveSpace')
		);
		expect(hydrate).toContain('branchPath');
		expect(hydrate).toContain('lockedSeedId');

		const page = readFileSync(PAGE, 'utf8');
		expect(page).toContain('<DiscoverBreadcrumb');
		const panel = readFileSync(
			'src/lib/components/DiscoverSpace/DiscoverSidePanel.svelte',
			'utf8'
		);
		expect(panel).toContain('branchHere');
	});

	test('declutter: no window coupling, no dead prop, one blend fetch trigger', () => {
		const canvas = readFileSync(
			'src/lib/components/DiscoverSpace/DiscoverSpace.svelte',
			'utf8'
		);
		expect(canvas).not.toContain('__discoverSpaceHyperspaceSearch');
		expect(canvas).not.toContain('onNewNodes');
		// Recenter effect fires on node-set changes only.
		expect(canvas).toContain('lastNodeSetKey');

		const page = readFileSync(PAGE, 'utf8');
		expect(page).not.toContain('__discoverSpaceHyperspaceSearch');
		expect(page).toContain('bind:this={spaceCanvas}');
		expect(page).not.toContain('DiscoverLegend');

		const store = readFileSync(STORE, 'utf8');
		// The store owns the blend fetch; page handlers must not double-trigger.
		const addBlendBody = store.slice(
			store.indexOf('export function addBlendSeed'),
			store.indexOf('export function removeBlendSeed')
		);
		expect(addBlendBody).toContain('loadBlendSpace');
		const pageAddHandler = page.slice(
			page.indexOf('function handleAddToBlend'),
			page.indexOf('function handleRemoveBlendSeed')
		);
		expect(pageAddHandler).not.toContain('loadBlendSpace');
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
