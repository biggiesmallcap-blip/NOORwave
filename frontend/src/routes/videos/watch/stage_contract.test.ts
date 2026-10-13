import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, test } from 'vitest';

const here = dirname(fileURLToPath(import.meta.url));
const watch = readFileSync(join(here, '+page.svelte'), 'utf8');
const row = readFileSync(join(here, '../../../lib/video/archive/StageRow.svelte'), 'utf8');

describe('watch page stage contract', () => {
	test('builds the four rows from the Atlas and Air endpoints', () => {
		expect(watch).toContain('void loadStage(');
		expect(watch).toContain('title="This concert"');
		expect(watch).toContain('title={`Same scene: ${scene.name}`}');
		expect(watch).toContain('title="One step out"');
		expect(watch).toContain('title="Other cuts of this song"');
	});

	test('a stale video never writes rows', () => {
		expect(watch).toContain('() => seq === stageRequest');
	});

	test('the related grid is the fallback when the exploring rows come back empty', () => {
		expect(watch).toContain('let useFallback = $derived(stageSettled && needsFallback(stageRows));');
		expect(watch).toContain('if (!fallback || !item || (!item.artist_id && !item.artist_name)) return;');
		expect(watch).toContain('{#if current && useFallback && (relatedLoading || feedLoading || relatedVideos.length > 0)}');
		expect(watch).toContain('if (!useFallback || !current || relatedLoading');
	});

	test('concerts play whole and in order; scenes deep link to the Archive', () => {
		expect(watch).toContain('onclick={() => void playConcert(concert)}');
		expect(watch).toContain('playConcert(concert, video.tidal_id)');
		expect(watch).toContain('href={`/videos/archive?scene=${scene.id}`}');
	});

	test('leaves the slot the Atlas mini map mounts into', () => {
		expect(watch).toContain('<div class="atlas-minimap-slot"></div>');
	});

	test('row cards carry the shared video menu', () => {
		expect(row).toContain('<VideoCard video={item.video}');
	});
});
