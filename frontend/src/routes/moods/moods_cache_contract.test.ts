import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, test } from 'vitest';

const here = dirname(fileURLToPath(import.meta.url));
const moodsPage = readFileSync(join(here, '+page.svelte'), 'utf8');
const moodDetailPage = readFileSync(join(here, '[slug]', '+page.svelte'), 'utf8');
const homeMoodsRail = readFileSync(
	join(here, '../../lib/components/home/HomeMoodsRail.svelte'),
	'utf8',
);
const spotifyMoodRail = readFileSync(
	join(here, '../../lib/components/moods/SpotifyMoodRail.svelte'),
	'utf8',
);

describe('moods cache and artwork contracts', () => {
	test('landing and home rail render TIDAL artwork through ArtworkImage', () => {
		for (const source of [moodsPage, homeMoodsRail]) {
			expect(source).toContain("import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';");
			expect(source).toContain('<ArtworkImage');
			expect(source).toContain('size={320}');
			expect(source).toContain('fallbackText="~"');
		}
	});

	test('landing and home rail cache provisional categories and share refresh throttling', () => {
		for (const source of [moodsPage, homeMoodsRail]) {
			expect(source).toContain('putCachedMoodCategories');
			expect(source).toContain('claimMoodThumbnailRefresh');
			expect(source).not.toContain('putCompleteMoodCategories');
			expect(source).not.toContain('MAX_THUMBNAIL_REFRESH_ATTEMPTS');
		}
	});

	test('landing and home rail guard mood loads against stale responses', () => {
		for (const source of [moodsPage, homeMoodsRail]) {
			expect(source).toContain('let loadSeq = 0;');
			expect(source).toContain('loadSeq += 1;');
			expect(source).toContain('const seq = ++loadSeq;');
			expect(source).toContain('if (seq !== loadSeq) return;');
			expect(source).toContain('if (seq === loadSeq)');
		}
	});

	test('mood context menus are app-owned', () => {
		for (const source of [moodsPage, homeMoodsRail, spotifyMoodRail]) {
			expect(source).toContain('e.preventDefault(); e.stopPropagation(); openContextMenu');
		}
	});

	test('mood drill-down uses abortable loads and caches empty module responses', () => {
		expect(moodDetailPage).toContain('let activeMoodController: AbortController | null = null;');
		expect(moodDetailPage).toContain('api.getTidalMoodPage(s, controller.signal)');
		expect(moodDetailPage).toContain('if (!isCurrentMoodRequest(s, generation, controller.signal)) return;');
		expect(moodDetailPage).toContain('putCachedMoodPage(s, modules);');
		expect(moodDetailPage).toContain("viewState = cached.length > 0 ? 'ready' : 'empty';");
	});
});
