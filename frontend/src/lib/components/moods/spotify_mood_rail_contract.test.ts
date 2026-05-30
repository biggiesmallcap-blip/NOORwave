import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, test } from 'vitest';
import { SPOTIFY_MOOD_CATEGORIES } from './spotify-moods-data';

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, 'SpotifyMoodRail.svelte'), 'utf8');

describe('Spotify mood rail contracts', () => {
	test('renders Spotify mood cards from cached cover metadata and stable fallbacks', () => {
		expect(source).toContain('ArtworkImage');
		expect(source).toContain('getCachedSpotifyChartMetaMap');
		expect(source).toContain("fallbackText={p.title.slice(0, 2).toUpperCase()}");
	});

	test('does not fetch full Spotify playlists while rendering mood cards', () => {
		expect(source).not.toContain("import { api } from '$lib/api/client';");
		expect(source).not.toContain('getSpotifyPlaylist');
		expect(source).not.toContain('setTimeout(() =>');
	});

	test('preserves mood origin when opening Spotify playlists', () => {
		expect(source).toContain("new URLSearchParams({ from: 'moods', mood: category.slug })");
		expect(source).toContain('href={playlistHref(p.id)}');
		expect(source).toContain('goto(playlistHref(id))');
	});

	test('does not link retired Spotify editorial playlist IDs', () => {
		const retiredIds = new Set(['37i9dQZF1DWZ7eJRBxJpAa', '37i9dQZF1DXdPec7aLusmQ']);
		const playlistIds = SPOTIFY_MOOD_CATEGORIES.flatMap((category) =>
			category.playlists.map((playlist) => playlist.id),
		);

		expect(playlistIds.filter((id) => retiredIds.has(id))).toEqual([]);
	});
});
