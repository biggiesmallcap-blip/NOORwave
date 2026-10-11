import { describe, expect, test } from 'vitest';
import type { ProviderRecommendationItem, ProviderRecommendationShelf } from '$lib/api/client';
import {
	PANEL_LIMIT,
	hasMoreThanShelf,
	mergeShelves,
	paintState,
	seededViewState,
} from './home_shelves';

const item = (title: string) => ({ title }) as ProviderRecommendationItem;

function shelf(
	entity: 'track' | 'artist' | 'album',
	status: ProviderRecommendationShelf['status'],
	titles: string[],
): ProviderRecommendationShelf {
	return {
		provider: 'lastfm',
		entity_type: entity,
		title: `Last.fm recommended ${entity}s`,
		status,
		items: titles.map(item),
	};
}

describe('mergeShelves', () => {
	test('an empty warming shelf never blanks a full one on screen', () => {
		const onScreen = [shelf('artist', 'ok', ['A', 'B'])];
		const merged = mergeShelves(onScreen, [shelf('artist', 'warming', [])]);
		expect(merged[0].items.map((i) => i.title)).toEqual(['A', 'B']);
	});
	test('a published shelf replaces the one on screen', () => {
		const onScreen = [shelf('artist', 'ok', ['A'])];
		const merged = mergeShelves(onScreen, [shelf('artist', 'ok', ['C', 'D'])]);
		expect(merged[0].items.map((i) => i.title)).toEqual(['C', 'D']);
	});
	test('an empty shelf that is not warming is the truth and wins', () => {
		const onScreen = [shelf('album', 'ok', ['A'])];
		const merged = mergeShelves(onScreen, [shelf('album', 'empty', [])]);
		expect(merged[0].items).toHaveLength(0);
	});
	test('the payload decides which shelves exist and their order', () => {
		const merged = mergeShelves(
			[shelf('track', 'ok', ['T'])],
			[shelf('album', 'warming', []), shelf('track', 'warming', [])],
		);
		expect(merged.map((s) => s.entity_type)).toEqual(['album', 'track']);
		expect(merged[1].items.map((i) => i.title)).toEqual(['T']);
	});
});

describe('view state', () => {
	test('items paint ready; warming with none shows loading; otherwise empty', () => {
		expect(paintState([shelf('track', 'ok', ['T'])])).toBe('ready');
		expect(paintState([shelf('track', 'warming', [])])).toBe('loading');
		expect(paintState([shelf('track', 'empty', [])])).toBe('empty');
	});
	test('boot paints cached shelves only when a provider can recommend', () => {
		const cached = [shelf('track', 'ok', ['T'])];
		expect(seededViewState(true, cached)).toBe('ready');
		expect(seededViewState(false, cached)).toBe('hidden');
		expect(seededViewState(true, [])).toBe('hidden');
	});
});

describe('hasMoreThanShelf', () => {
	test('only shelves longer than the rail link to View all', () => {
		const titles = (n: number) => Array.from({ length: n }, (_, i) => `t${i}`);
		expect(hasMoreThanShelf(shelf('track', 'ok', titles(PANEL_LIMIT)))).toBe(false);
		expect(hasMoreThanShelf(shelf('track', 'ok', titles(PANEL_LIMIT + 1)))).toBe(true);
	});
});
