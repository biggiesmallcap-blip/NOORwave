import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, test } from 'vitest';

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, '[id]', '+page.svelte'), 'utf8');

describe('external Spotify playlist page contract', () => {
	test('labels the page as an external Spotify playlist', () => {
		expect(source).toContain('eyebrow="External playlist"');
		expect(source).toContain('<SpotifySourceBadge />');
		expect(source).not.toContain('ephemeral"');
	});

	test('saves visibly, then offers Update once a saved copy exists', () => {
		expect(source).toContain("{busy === 'saving' ? 'Saving...' : 'Save playlist'}");
		expect(source).toContain('savedPlaylist = res.savedPlaylist;');
		expect(source).toContain("{busy === 'updating' ? 'Updating...' : 'Update from Spotify'}");
		// Update re-reads Spotify, not the server's cached copy.
		expect(source).toContain('api.getSpotifyPlaylist(id, undefined, { refresh: true })');
		// Save waits for background matching so pending tracks are not skipped.
		expect(source).toContain('await settlePending(seq);');
	});

	test('rescans unmatched tracks against TIDAL, ignoring cached misses', () => {
		expect(source).toContain('api.resolveTidalBulk(ids, true)');
		expect(source).toContain("label: 'Rescan unmatched tracks'");
	});
});
