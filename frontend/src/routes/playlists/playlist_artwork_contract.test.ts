import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const page = readFileSync(join(here, '+page.svelte'), 'utf8');
const cache = readFileSync(join(here, '../../lib/stores/playlist_artwork_cache.ts'), 'utf8');

describe('playlist artwork contracts', () => {
	test('playlist cover mosaics route track artwork through ArtworkImage', () => {
		expect(cache).toContain('pickArtworkUrls(tracks: Array<{ artwork_url: string | null }>)');
		expect(page).toContain("import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';");
		expect(page).toContain('<ArtworkImage');
		expect(page).toContain('className="playlist-cover-art"');
		expect(page).toContain('src={url}');
		expect(page).toContain('src={mosaic[0]}');
		expect(page).toContain('size={320}');
		expect(page).toContain('fallbackText="PL"');
		expect(page).toContain('decorative={true}');
		expect(page).toContain(':global(.playlist-cover-art)');
		expect(page).not.toContain('<img src={url}');
		expect(page).not.toContain('<img class="cover-solo"');
	});

	test('cleans up playlist route async work on destroy', () => {
		expect(page).toContain("import { onDestroy, onMount } from 'svelte';");
		expect(page).toContain('let playlistLoadSeq = 0;');
		expect(page).toContain('let destroyed = false;');
		expect(page).toContain('onDestroy(() => {');
		expect(page).toContain('artistFetchAbort?.abort();');
		expect(page).toContain('previewAbort?.abort();');
		expect(page).toContain('clearTimeout(previewDebounce);');
		expect(page).toContain('mosaicObserver?.disconnect();');
		expect(page).toContain('function isCurrentPlaylistLoad(seq: number): boolean');
		expect(page).toContain('return !destroyed && seq === playlistLoadSeq;');
		expect(page).toContain('if (!isCurrentPlaylistLoad(seq)) return;');
		// ensurePlaylistTracks awaits a fetch and then writes component state, so
		// both the success and the failure path have to bail after unmount.
		expect(page).toContain('if (destroyed) return [];');
	});

	test('rows link to the playlist detail route instead of expanding in place', () => {
		// The accordion was replaced by /playlists/[id]; leaving its state behind
		// would mean two sources of truth for a playlist's track list.
		expect(page).toContain('href={`/playlists/${playlist.id}`}');
		expect(page).not.toContain('expandedPlaylistIds');
		expect(page).not.toContain('aria-expanded');
	});

	test('the list is borderless rather than a stack of glass panels', () => {
		// STYLING.md: browse lists are flat. Glass is for panels and overlays,
		// and the drawer below is the only thing on this page that qualifies.
		expect(page).toContain('class="playlist-row"');
		expect(page).not.toContain('class="playlist-card glass-panel"');
		expect(page).toContain('.playlist-row:hover');
		expect(page).toContain('background: var(--bg-hover);');
	});

	test('sort and filter persist across sessions and default to last updated', () => {
		// A SvelteKit snapshot only restores on back/forward, so these lived in
		// localStorage as well; the guards against a QuotaExceeded boot crash are
		// inside createPersistedStore.
		expect(page).toContain("import { createPersistedStore, oneOf } from '$lib/stores/persisted';");
		expect(page).toContain("createPersistedStore<PlaylistSort>('playlists.sort', 'recent_update'");
		expect(page).toContain("createPersistedStore<PlaylistFilter>('playlists.filter', 'all'");
	});
});
