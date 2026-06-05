import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, test } from 'vitest';

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, '[id]', '+page.svelte'), 'utf8');

describe('TIDAL discover detail route contract', () => {
	test('guards discover shelf loads against stale route responses', () => {
		expect(source).toContain('let loadSeq = 0;');
		expect(source).toContain('loadSeq += 1;');
		expect(source).toContain("error = 'Missing discover shelf.';");
		expect(source).toContain('const seq = ++loadSeq;');
		expect(source).toContain('const res = await api.getTidalDiscoverModule(id, 50);');
		expect(source).toContain('if (seq !== loadSeq) return;');
		expect(source).toContain('if (seq === loadSeq) loading = false;');
		expect(source).toContain('void load(id);');
	});

	test('keeps shelf item actions wired through shared TIDAL helpers', () => {
		expect(source).toContain('void playTidalTrackNow(tidalHomeItemToPlayable(item));');
		expect(source).toContain('void playTidalPlaylist(item.id);');
		expect(source).toContain('openContextMenu(event, buildTidalTrackMenu(tidalHomeItemToPlayable(item)), item.title);');
		expect(source).toContain('openContextMenu(event, buildAlbumMenu({');
		expect(source).toContain('openContextMenu(event, buildArtistMenu({');
		expect(source).not.toContain('$:');
	});
});
