import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, test } from 'vitest';

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, 'SpotifyMoodRail.svelte'), 'utf8');

describe('Spotify mood rail contracts', () => {
	test('does not prefetch full Spotify playlists while rendering mood cards', () => {
		expect(source).not.toContain('getSpotifyPlaylist');
		expect(source).not.toContain("import { api } from '$lib/api/client';");
	});
});
