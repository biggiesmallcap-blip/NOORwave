import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, '[id]', '+page.svelte'), 'utf8');
const discographySource = readFileSync(join(here, '[id]', 'discography', '[section]', '+page.svelte'), 'utf8');

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

	test('guards artist route loads against stale responses', () => {
		expect(source).toContain('let loadSeq = 0;');
		expect(source).toContain('async function load(id: number)');
		expect(source).toContain('const seq = ++loadSeq;');
		expect(source).toContain('cachedApi.getArtist(id)');
		expect(source).toContain('cachedApi.getArtistTracks(id)');
		expect(source).toContain('if (seq !== loadSeq) return;');
		expect(source).toContain('if (seq === loadSeq) loading = false;');
		expect(source).toContain('const id = artistId;');
		expect(source).toContain('tracks = [];');
		expect(source).toContain('void load(id);');
		expect(source).not.toContain('void load();');
	});

	test('guards artist enrichment loads against stale responses', () => {
		expect(source).toContain('let tidalLoadSeq = 0;');
		expect(source).toContain('async function loadDiscography(id: number)');
		expect(source).toContain('const seq = ++tidalLoadSeq;');
		expect(source).toContain('const res = await cachedApi.getArtistDiscography(id);');
		expect(source).toContain('if (seq !== tidalLoadSeq) return;');
		expect(source).toContain('if (seq === tidalLoadSeq) tidalLoading = false;');
		expect(source).toContain('let spotifyLoadSeq = 0;');
		expect(source).toContain('async function loadSpotifyStats(id: number)');
		expect(source).toContain('const stats = await cachedApi.getArtistSpotifyStats(id);');
		expect(source).toContain('if (seq === spotifyLoadSeq) spotifyStats = stats;');
		expect(source).toContain('void loadDiscography(id);');
		expect(source).toContain('void loadSpotifyStats(id);');
	});

	test('hero play falls back to TIDAL top tracks when the local artist has no tracks', () => {
		expect(source).toContain('playTidalTracksNow');
		expect(source).toContain('async function ensureTidalTopTracksForPlayback(id: number)');
		expect(source).toContain('if (tracks.length > 0)');
		expect(source).toContain('const requestedFor = artistId;');
		expect(source).toContain('const topTracks = await ensureTidalTopTracksForPlayback(requestedFor);');
		expect(source).toContain('if (artistId !== requestedFor) return;');
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

	test('orders top tracks through the TIDAL popularity list before local leftovers', () => {
		expect(source).toContain("type PopularTrackItem =");
		expect(source).toContain('for (const tidalTrack of tidalTopTracks)');
		expect(source).toContain('const localTrack = byTidalId.get(tidalTrack.tidal_id);');
		expect(source).toContain("ordered.push({ kind: 'local', track: localTrack });");
		expect(source).toContain("ordered.push({ kind: 'tidal', track: tidalTrack });");
		expect(source).toContain('ordered.push(...localRemainder.map((track) => ({ kind: \'local\' as const, track })))');
		expect(source).not.toContain('if (a.is_favorite !== b.is_favorite) return a.is_favorite ? -1 : 1;');
	});

	test('links artist shelves to searchable see-all routes', () => {
		expect(source).toContain('href={`/artists/${artistId}/discography/tracks`}');
		expect(source).toContain('href={`/artists/${artistId}/discography/albums`}');
		expect(source).toContain('href={`/artists/${artistId}/discography/singles`}');
		expect(source).toContain('href={`/artists/${artistId}/discography/compilations`}');
		expect(discographySource).toContain("type Section = 'tracks' | 'albums' | 'singles' | 'compilations';");
		expect(discographySource).toContain('cachedApi.getArtistDiscography(id)');
		expect(discographySource).toContain('type="search"');
		expect(discographySource).toContain('<TrackRow');
		expect(discographySource).toContain('<TidalTrackRow');
		expect(discographySource).toContain('openContextMenu(event, albumMenu(album), album.title);');
	});

	test('keeps artist media rail context menus app-owned', () => {
		expect(source).toContain('openContextMenu(e, buildTidalTrackMenu(playable), track.title);');
		expect(source).toContain('openContextMenu(e, similarArtistMenu(similar), similar.name);');
		expect(source).toContain('openContextMenu(e, fallbackAlbumMenu(album), album.title);');
		expect(source).toContain('e.preventDefault();');
		expect(source).toContain('e.stopPropagation();');
	});

	test('keeps route copy and comments ASCII-safe', () => {
		expect(source).not.toMatch(/\u2014/);
		expect(source).not.toMatch(/[\u2500-\u257F]/);
	});
});
