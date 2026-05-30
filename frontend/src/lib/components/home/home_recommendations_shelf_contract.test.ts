import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, test } from 'vitest';

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, 'HomeRecommendationsShelf.svelte'), 'utf8');
const homePage = readFileSync(join(here, '../../../routes/+page.svelte'), 'utf8');
const client = readFileSync(join(here, '../../api/client.ts'), 'utf8');

describe('home recommendations shelf contract', () => {
	test('loads provider shelves independently after Home renders', () => {
		expect(homePage).toContain('HomeRecommendationsShelf');
		expect(source).toContain('onMount');
		expect(source).toContain('api.getLastfmStatus()');
		expect(source).toContain('api.getListenBrainzStatus()');
		expect(source).toContain('api.getHomeRecommendations()');
		expect(client).toContain('/api/home/recommendations');
	});

	test('has loading, empty, and error states for provider data', () => {
		expect(source).toContain("type State = 'hidden' | 'loading' | 'ready' | 'empty' | 'error'");
		expect(source).toContain("viewState === 'hidden'");
		expect(source).toContain("viewState === 'loading'");
		expect(source).toContain("viewState === 'empty'");
		expect(source).toContain("viewState === 'error'");
		expect(source).toContain('Connected profiles have no playable recommendations yet.');
		expect(source).toContain('Retry');
	});

	test('renders only playable local rows through the standard player path', () => {
		expect(source).toContain('playTrackNow');
		expect(source).toContain('item.local_track_id');
		expect(source).toContain('upscaleTidalArtwork(url, 320)');
		expect(source).toContain('onerror');
	});
});
