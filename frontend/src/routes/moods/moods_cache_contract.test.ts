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

	test('mood drill-down uses abortable loads and caches empty module responses', () => {
		expect(moodDetailPage).toContain('let activeMoodController: AbortController | null = null;');
		expect(moodDetailPage).toContain('api.getTidalMoodPage(s, controller.signal)');
		expect(moodDetailPage).toContain('if (!isCurrentMoodRequest(s, generation, controller.signal)) return;');
		expect(moodDetailPage).toContain('putCachedMoodPage(s, modules);');
		expect(moodDetailPage).toContain("viewState = cached.length > 0 ? 'ready' : 'empty';");
	});
});
