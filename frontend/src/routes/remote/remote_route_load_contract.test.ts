import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

function source(path: string): string {
	return readFileSync(resolve(__dirname, path), 'utf8');
}

describe('remote route load contracts', () => {
	const guardedPages = [
		['local artist', source('artists/[id]/+page.svelte')],
		['local album', source('albums/[id]/+page.svelte')],
		['playlist', source('playlists/[id]/+page.svelte')],
		['TIDAL album', source('tidal/albums/[id]/+page.svelte')],
	] as const;

	test.each(guardedPages)('%s ignores stale route-load responses', (_name, page) => {
		expect(page).toContain('let loadSeq = 0;');
		expect(page).toContain('async function load(id: number)');
		expect(page).toContain('const seq = ++loadSeq;');
		expect(page).toContain('if (seq !== loadSeq) return;');
		expect(page).toContain('if (seq === loadSeq) loading = false;');
		expect(page).toContain('const id =');
		expect(page).toContain('void load(id);');
		expect(page).not.toContain('void load();');
	});

	test('local artist discography cannot overwrite a newer artist route', () => {
		const page = source('artists/[id]/+page.svelte');
		expect(page).toContain('api.getArtist(id)');
		expect(page).toContain('api.getArtistTracks(id)');
		expect(page).toContain('api.getArtistDiscography(id)');
		expect(page).toContain('if (!detailLoaded || seq !== loadSeq) return;');
		expect(page).toContain('if (seq === loadSeq) {');
		expect(page).toContain('tidalPictureUrl = null;');
	});
});
