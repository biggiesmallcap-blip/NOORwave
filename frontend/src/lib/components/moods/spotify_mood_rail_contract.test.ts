import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, test } from 'vitest';
import { SPOTIFY_MOOD_CATEGORIES } from './spotify-moods-data';

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, 'SpotifyMoodRail.svelte'), 'utf8');

describe('Spotify mood rail contracts', () => {
	test('does not prefetch full Spotify playlists while rendering mood cards', () => {
		expect(source).not.toContain('getSpotifyPlaylist');
		expect(source).not.toContain("import { api } from '$lib/api/client';");
	});

	test('does not link retired Spotify editorial playlist IDs', () => {
		const retiredIds = new Set(['37i9dQZF1DWZ7eJRBxJpAa', '37i9dQZF1DXdPec7aLusmQ']);
		const playlistIds = SPOTIFY_MOOD_CATEGORIES.flatMap((category) =>
			category.playlists.map((playlist) => playlist.id),
		);

		expect(playlistIds.filter((id) => retiredIds.has(id))).toEqual([]);
	});
});
