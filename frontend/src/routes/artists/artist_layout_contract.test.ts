import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
// The artist view markup, styles, and load logic live in the shared
// ArtistDetail component now; the route's +page.svelte is a thin wrapper.
const source = readFileSync(join(here, 'ArtistDetail.svelte'), 'utf8');
const discographySource = readFileSync(join(here, 'ArtistDiscographySection.svelte'), 'utf8');
const discographyHelper = readFileSync(join(here, 'artist_discography.ts'), 'utf8');

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

	test('keeps the artist flow sourced from TIDAL and the local library only', () => {
		// The Spotify proxy layer (anonymous, flaky, was erroring in production
		// logs) is gated out of artist pages: no stats fetch, no world-plays
		// column, no stray badge markup.
		expect(source).not.toContain('SpotifyArtistStats');
		expect(source).not.toContain('getArtistSpotifyStats');
		expect(source).not.toContain('loadSpotifyStats');
		expect(source).not.toContain('worldPlayCount=');
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
		expect(source).toContain('const id = source.artistId;');
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
		expect(source).toContain('void loadDiscography(id);');
	});

	test('serves the TIDAL artist profile through the cache layer', () => {
		// cachedApi gives in-flight dedupe + stale-while-revalidate; the raw
		// api call refetched the full nine-call TIDAL fan-out on every visit.
		expect(source).toContain('await cachedApi.getTidalArtistProfile(tidalId)');
		expect(source).not.toContain('await api.getTidalArtistProfile(tidalId)');
	});

	test('renders TIDAL artist core data while full shelves continue loading', () => {
		expect(source).toContain("import { ARTIST_ENRICHMENT_DELAY_MS } from '$lib/artist/artist_loading';");
		expect(source).toContain('async function loadTidalCore(tidalId: number, seq: number)');
		expect(source).toContain('await cachedApi.getTidalArtistCore(tidalId)');
		expect(source).toContain('void loadTidalCore(tidalId, seq);');
		expect(source).toContain('void loadTidalProfile(tidalId, seq);');
		expect(source.indexOf('void loadTidalCore(tidalId, seq);')).toBeLessThan(
			source.indexOf('void loadTidalProfile(tidalId, seq);'),
		);
		expect(source).toContain('if (res.available) loading = false;');
		expect(source).toMatch(/setTimeout\(\(\) => \{[\s\S]*void loadTidalProfile\(tidalId, seq\);[\s\S]*\}, ARTIST_ENRICHMENT_DELAY_MS\);/);
	});

	test('gives video rail cards the app-owned context menu', () => {
		expect(source).toContain("import { buildVideoMenu } from '$lib/player/video_menu';");
		expect(source).toContain('openContextMenu(e, buildVideoMenu(video), video.title);');
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
		expect(source).toContain("tidalDiscographyTrackToPlayable(track, { artistTidalId: activeTidalArtistId })");
		expect(source).toContain('{@const playable = artistTrackPlayable(track)}');
		// Importing the shared TidalPlayable type is fine; only a local
		// re-declaration of it (or the mapper) is banned.
		expect(source).not.toContain('type TidalPlayable =');
		expect(source).not.toContain('function tidalDiscographyTrackToPlayable(t: TidalDiscographyTrack)');
	});

	test('orders top tracks through the TIDAL popularity list before local leftovers', () => {
		// The merge lives in the shared artist_discography helper now, so the
		// artist page and the see-all section pages can never drift apart.
		expect(discographyHelper).toContain('export type PopularTrackItem =');
		expect(discographyHelper).toContain('for (const tidalTrack of tidalTopTracks)');
		expect(discographyHelper).toContain('const localTrack = byTidalId.get(tidalTrack.tidal_id);');
		expect(discographyHelper).toContain("ordered.push({ kind: 'local', track: localTrack });");
		expect(discographyHelper).toContain("ordered.push({ kind: 'tidal', track: tidalTrack });");
		expect(discographyHelper).toContain('ordered.push(...localRemainder.map((track) => ({ kind: \'local\' as const, track })))');
		expect(source).toContain('buildPopularTrackItems(tracks, tidalTopTracks, localPopularityScore)');
		expect(source).not.toContain('if (a.is_favorite !== b.is_favorite) return a.is_favorite ? -1 : 1;');
	});

	test('both artist surfaces bucket releases through the shared helper', () => {
		// Guard against the old drift: each component carried a private
		// categorize() copy and LIVE releases were bucketed differently
		// between the artist page and the see-all section page.
		expect(source).toContain("from './artist_discography'");
		expect(discographySource).toContain("from './artist_discography'");
		expect(source).toContain('categorizeTidalAlbum');
		expect(discographySource).toContain('discographySectionFor(album) === section');
		expect(discographySource).not.toContain('function categorize(');
		expect(discographySource).not.toContain('function releaseSort(');
	});

	test('links artist shelves to searchable see-all routes', () => {
		expect(source).toContain('href={`${discographyBase}/discography/tracks`}');
		expect(source).toContain('href={`${discographyBase}/discography/albums`}');
		expect(source).toContain('href={`${discographyBase}/discography/singles`}');
		expect(source).toContain('href={`${discographyBase}/discography/compilations`}');
		expect(discographySource).toContain("type Section = 'tracks' | 'albums' | 'singles' | 'compilations';");
		expect(discographySource).toContain('cachedApi.getArtistDiscography(id)');
		expect(discographySource).toContain('<SearchField');
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
