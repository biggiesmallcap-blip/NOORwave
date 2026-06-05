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

describe('artist page layout contracts', () => {
	test('keeps the artist portrait top-aligned as the biography expands', () => {
		const hero = cssBlock('.hero');
		expect(hero).toContain('align-items: flex-start');

		const body = cssBlock('.hero-body');
		expect(body).toContain('align-items: flex-start');

		const portraitWrap = cssBlock('.hero-portrait-wrap');
		expect(portraitWrap).toContain('align-self: flex-start');
	});

	test('keeps expanded biography readable without pushing the page too far down', () => {
		expect(source).toContain('class="hero-bio-panel" class:expanded={bioExpanded}');

		const expandedBio = cssBlock('.hero-bio-panel.expanded .hero-bio');
		expect(expandedBio).toContain('max-height: clamp(');
		expect(expandedBio).toContain('overflow-y: auto');

		const bio = cssBlock('.hero-bio');
		expect(bio).toContain('white-space: pre-line');
	});

	test('shows Spotify world plays beside local plays only through TrackRow', () => {
		expect(source).toContain('worldPlayCount={streamCount ?? null}');
		expect(source).not.toContain('class="stream-badge"');
	});

	test('hero play falls back to TIDAL top tracks when the local artist has no tracks', () => {
		expect(source).toContain('playTidalTracksNow');
		expect(source).toContain('async function ensureTidalTopTracksForPlayback()');
		expect(source).toContain('if (tracks.length > 0)');
		expect(source).toContain('const playable = topTracks.map(artistTrackPlayable)');
		expect(source).toContain('await playTidalTracksNow(playable, artist?.name ?? \'artist\')');
		expect(source).toContain('await playArtist(artistId)');
	});

	test('uses the shared TIDAL discography playable mapper with the artist fallback id', () => {
		expect(source).toContain("import { tidalDiscographyTrackToPlayable } from '$lib/utils/track';");
		expect(source).toContain('function artistTrackPlayable(track: TidalDiscographyTrack)');
		expect(source).toContain("tidalDiscographyTrackToPlayable(track, { artistTidalId: artist?.tidal_id ?? null })");
		expect(source).toContain('{@const playable = artistTrackPlayable(track)}');
		expect(source).not.toContain('type TidalPlayable');
		expect(source).not.toContain('function tidalDiscographyTrackToPlayable(t: TidalDiscographyTrack)');
	});

	test('keeps route copy and comments ASCII-safe', () => {
		expect(source).not.toMatch(/\u2014/);
		expect(source).not.toMatch(/[\u2500-\u257F]/);
	});
});
