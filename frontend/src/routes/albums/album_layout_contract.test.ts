import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, '[id]', '+page.svelte'), 'utf8');

function cssBlock(selector: string): string {
	const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
	const match = source.match(new RegExp(`${escaped}\\s*\\{(?<body>[^}]*)\\}`));
	if (!match?.groups?.body) {
		throw new Error(`Missing CSS block for ${selector}`);
	}
	return match.groups.body;
}

describe('album page layout contracts', () => {
	test('keeps the artwork backdrop subtle behind album and artist metadata', () => {
		const backdrop = cssBlock('.hero-backdrop');
		expect(backdrop).toContain('saturate(1.08)');
		expect(backdrop).toContain('brightness(0.72)');
		expect(backdrop).toContain('opacity: 0.32');
		expect(backdrop).not.toContain('saturate(1.6)');
		expect(backdrop).not.toContain('opacity: 0.7');

		const veil = cssBlock('.hero-veil');
		expect(veil).toContain('rgba(11, 11, 15, 0.62)');
		expect(veil).toContain('rgba(11, 11, 15, 0.78)');
		expect(veil).not.toContain('rgba(0,0,0,0.08)');
	});

	test('loads album Spotify stats and passes album track world plays to TrackRow', () => {
		expect(source).toContain('cachedApi.getAlbumSpotifyStats');
		expect(source).not.toContain('api.getArtistSpotifyStats');
		expect(source).toContain('playcountByIsrc');
		expect(source).toContain('worldPlayCount={track.isrc ? playcountByIsrc.get(track.isrc) : null}');
	});
});
