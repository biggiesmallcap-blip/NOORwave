import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, test } from 'vitest';

const here = dirname(fileURLToPath(import.meta.url));
const watch = readFileSync(join(here, '+page.svelte'), 'utf8');
const row = readFileSync(join(here, '../../../lib/video/archive/StageRow.svelte'), 'utf8');

describe('watch page stage contract', () => {
	test('builds the concert, the cuts line and the exploring picks from the Atlas and Air endpoints', () => {
		expect(watch).toContain('void loadStage(');
		expect(watch).toContain('title="This concert"');
		expect(watch).toContain('aria-label="Other cuts of this song"');
		expect(watch).toContain('untrack(() => explorePicks(stageRows, item.tidal_id))');
	});

	test('a stale video never writes rows', () => {
		expect(watch).toContain('() => seq === stageRequest');
	});

	test('one endless grid: exploring picks first, related next, then the radio feed', () => {
		expect(watch).toContain('relatedVideos = picks;');
		expect(watch).toContain('relatedVideos = mergeFeed(picks, items, item.tidal_id);');
		expect(watch).toContain('if (!stageSettled || !current || relatedLoading');
		expect(watch).not.toContain('useFallback');
	});

	test('concerts play whole and in order; scenes deep link to the Archive', () => {
		expect(watch).toContain('onclick={() => void playConcert(concert)}');
		expect(watch).toContain('playConcert(concert, video.tidal_id)');
		expect(watch).toContain('href={`/videos/archive?scene=${scene.id}`}');
	});

	test('the Atlas mini map is the grid\'s first tile, only when the artist is on the map', () => {
		expect(watch).toContain('<AtlasMiniMap artistId={stageArtistId} {trail} caption={false} />');
		expect(watch).toContain('let lead = $derived(onMap ? 1 : 0);');
		expect(watch).toContain('atlas?.data.indexById.has(stageArtistId)');
		expect(watch).toContain('sessionTrail = [...sessionTrail.filter((seen) => seen !== id), id].slice(-10);');
	});

	test('row cards carry the shared video menu', () => {
		expect(row).toContain('<VideoCard video={item.video}');
	});
});
