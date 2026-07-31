import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

// A playlist created from the queue used to stay invisible on /playlists until
// the cached read expired (5 minutes in memory, a day in localStorage), which
// is why a hard refresh looked like the fix. Two things have to hold for it to
// update live, and both are easy to drop silently: every mutation invalidates
// the playlist caches, and the cache is invalidated before any page reacts to
// the server event.

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, '../..');
const read = (relative: string) => readFileSync(join(root, relative), 'utf8');

const MUTATION_SITES = [
	'lib/stores/player.ts',
	'routes/genres/+page.svelte',
	'routes/library/+page.svelte',
	'routes/playlists/+page.svelte',
	'routes/playlists/[id]/+page.svelte',
	'lib/player/playlist_menu.ts',
];

describe('playlist live-update contracts', () => {
	test('every surface that mutates a playlist invalidates the playlist caches', () => {
		for (const file of MUTATION_SITES) {
			const source = read(file);
			expect(source, `${file} must import invalidatePlaylistCaches`).toContain(
				'invalidatePlaylistCaches',
			);
		}
	});

	test('invalidatePlaylistCaches is exported and covers every playlist key', () => {
		const source = read('lib/cache/ws_events.ts');
		expect(source).toContain('export function invalidatePlaylistCaches()');
		for (const key of [
			'getPlaylists',
			'getPlaylistTracks',
			'evaluateSmartPlaylist',
			'getPlaylistCoverSample',
		]) {
			expect(source).toContain(`'${key}'`);
		}
	});

	test('the server pushes playlists_changed and the cache layer handles it', () => {
		expect(read('lib/api/ws.ts')).toContain("{ type: 'playlists_changed' }");
		const events = read('lib/cache/ws_events.ts');
		expect(events).toContain("message.type === 'playlists_changed'");
	});

	test('caches are invalidated before subscribers see the message', () => {
		// wsMessages subscribers run synchronously inside update(). If the log were
		// published first, a page reacting by re-reading through cachedApi would
		// read back exactly the data the event says is stale.
		const source = read('lib/api/ws.ts');
		const invalidateAt = source.indexOf('applyCacheUpdateForWsMessage(data);');
		const publishAt = source.indexOf('wsMessages.update((msgs)');
		expect(invalidateAt).toBeGreaterThan(-1);
		expect(publishAt).toBeGreaterThan(-1);
		expect(invalidateAt).toBeLessThan(publishAt);
	});

	test('both playlist routes reload when the server says playlists changed', () => {
		for (const file of ['routes/playlists/+page.svelte', 'routes/playlists/[id]/+page.svelte']) {
			const source = read(file);
			expect(source, `${file} subscribes to wsMessages`).toContain('wsMessages.subscribe');
			expect(source, `${file} reacts to playlists_changed`).toContain("'playlists_changed'");
		}
	});
});
