import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const read = (rel) => readFileSync(resolve(import.meta.dirname, rel), 'utf8');
const home = read('../src/routes/+page.svelte');
const mixes = read('../src/lib/components/home/YourMixesShelf.svelte');
const jump = read('../src/lib/components/home/JumpBackInShelf.svelte');

describe('Home order (audit Round 2)', () => {
	test('sections appear in the agreed order', () => {
		const order = [
			'<JumpBackInShelf',
			'<YourMixesShelf kind="music"',
			'<HomeRecommendationsShelf',
			'pagePath="new-releases"',
			'<PersonalRadioShelf',
			'<HomeMoodsRail',
			'<DiscoverShelves',
			'<YourMixesShelf kind="video"',
			'pagePath="hires"',
			'title="Weekly articles"',
			'title="Latest news"',
		];
		const at = order.map((needle) => home.indexOf(needle));
		for (const index of at) expect(index).toBeGreaterThan(-1);
		expect([...at].sort((a, b) => a - b)).toEqual(at);
	});

	test('the duplicate search field is gone', () => {
		expect(home).not.toContain('<SearchField');
		expect(home).not.toContain('homeSearchKeydown');
	});
});

describe('mix tiles print their name once', () => {
	test('the artwork carries the name; the caption shows only the subtitle', () => {
		expect(mixes).not.toContain('<h3 class="title">{mix.title}</h3>');
		expect(mixes).toContain("kind?: 'music' | 'video'");
	});
});

describe('Jump back in', () => {
	test('reuses the recent listens query and the shared track menu', () => {
		expect(jump).toContain('cachedApi.getRecentListens(50)');
		expect(jump).toContain('buildTrackMenu(');
		expect(jump).toContain('playTrackNow(entry.track_id)');
	});
});
