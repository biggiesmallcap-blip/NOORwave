import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, '../..');
const libraryPage = readFileSync(join(root, 'routes/library/+page.svelte'), 'utf8');
const libraryHero = readFileSync(join(root, 'lib/components/LibraryHero.svelte'), 'utf8');
const murals = readFileSync(join(root, 'lib/components/library/LibraryMurals.svelte'), 'utf8');
const muralModel = readFileSync(join(root, 'lib/components/library/library_murals.ts'), 'utf8');

function countOccurrences(source: string, needle: string): number {
	return source.split(needle).length - 1;
}

describe('library home hero contract', () => {
	test('top_artist_hero_uses_a_full_top_20_mural', () => {
		expect(libraryPage).toContain('played.slice(0, 20)');
		expect(libraryHero).toContain('YOUR TOP 20 ARTISTS');
		expect(libraryHero).toContain('hero-bg-mural');
		expect(libraryHero).toContain('function selectMuralArtist');
		expect(libraryHero).toContain('onclick={() => selectMuralArtist(artist.id)}');
		expect(libraryHero).toContain('aria-label={`Select ${artist.name}`}');
		expect(libraryHero).toContain('grid-template-columns: repeat(10');
		expect(libraryHero).toContain('grid-template-rows: repeat(2');
		expect(libraryHero).toContain('mural-panel--featured');
		// Text over the collage is white on a dark scrim in every theme; theme
		// text tokens turned the title dark-on-dark in light mode.
		expect(libraryHero).toContain('rgba(8,8,12,0.66) 0%');
		expect(libraryHero).toContain('rgba(8,8,12,0.08) 68%');
		expect(libraryHero).toContain('color: #fff;');
		expect(libraryHero).not.toContain('color: var(--text-primary, #fff)');
		// Collage filters are theme tokens, and the featured tile is lifted, not blown out.
		expect(libraryHero).toContain('filter: var(--art-collage-filter)');
		expect(libraryHero).not.toContain('brightness(1.52)');
		expect(libraryHero).not.toContain('saturate(1.95)');
		expect(libraryHero).toContain('scale(1.045)');
		expect(libraryHero).toContain('pointer-events: none');
		expect(libraryHero).toContain('pointer-events: auto');
		expect(libraryHero).toContain("import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte'");
		expect(libraryHero).toContain('function artistArtworkSources(artist: Artist): string[]');
		expect(libraryHero).toContain('<ArtworkImage');
		expect(libraryHero).toContain('className="mural-panel-art"');
		expect(libraryHero).toContain('src={artistArtworkSources(artist)}');
		expect(libraryHero).toContain('size={640}');
		expect(libraryHero).toContain('fallbackText={initials(artist.name)}');
		expect(libraryHero).toContain('decorative={true}');
		expect(libraryHero).toContain(':global(.mural-panel-art)');
		expect(libraryHero).not.toContain('upscaleTidalArtwork');
		expect(libraryHero).not.toContain('failedImageUrls');
		expect(libraryHero).not.toContain('onerror={() => markArtistArtFailed(artist)}');
		expect(libraryHero).not.toContain('<img src={panelArt}');
		expect(libraryHero).not.toContain('class="mural-fallback"');
		expect(libraryHero).not.toContain('letterColor');
		expect(libraryHero).not.toContain('played.slice(0, 5)');
	});

	test('library_home_mounts_the_extracted_suggestion_murals', () => {
		// The murals live in their own component; the page only mounts them and
		// hands over the album popup and album menu it owns.
		expect(libraryPage).toContain("import LibraryMurals from '$lib/components/library/LibraryMurals.svelte'");
		expect(libraryPage).toContain('<LibraryMurals');
		expect(libraryPage).toContain('onOpenAlbum={openHomeAlbumCard}');
		expect(libraryPage).toContain('onAlbumContextMenu={(event, card) => handleHomeAlbumContextMenu(event, card.id, card)}');
		expect(libraryPage).toContain('void openAlbumDetail(found ?? albumFromHomeCard(card))');
		expect(libraryPage).not.toContain('homeMuralPanels');
		expect(libraryPage).not.toContain('home-mural');
		expect(libraryPage).not.toContain('getHomeShufflePicks');
		expect(libraryPage).not.toContain('getHomeSuggestions');

		expect(muralModel).toContain("label: 'Suggested tracks'");
		expect(muralModel).toContain("label: 'Suggested albums'");
		expect(muralModel).toContain("caption: 'Listen history suggestions'");
		expect(muralModel).toContain("label: 'Random tracks'");
		expect(muralModel).toContain("label: 'Random albums'");
		expect(muralModel).toContain('export const HOME_PANEL_CACHE_REFRESH_MS = 5 * 60 * 1000');
		expect(muralModel).toContain('export const SUGGESTION_ARTIST_CAP = 2');
		// The artist cap shapes the head of the mural; it must top up from what
		// it skipped rather than hand back a short panel (5 of 12 picks).
		expect(muralModel).toContain('export function capPerArtist(tracks: Track[], max: number, limit: number): Track[]');
		expect(muralModel).toContain('capPerArtist(sources.suggestionTracks, SUGGESTION_ARTIST_CAP, HOME_MURAL_ITEM_LIMIT)');
		// Suggested albums render the server list directly; the old same-artist
		// tail-fill brought back the album that was just played.
		expect(muralModel).toContain('sources.suggestionAlbums.slice(0, HOME_MURAL_ITEM_LIMIT).map(albumToMuralItem)');
		expect(muralModel).not.toContain('sameArtistExpansion');

		// Both random murals come from one server call fired on mount, and the
		// suggestions are seedless, so neither waits for the library store.
		expect(murals).toContain('async function loadRandomPanelCandidates(requestKey: string)');
		expect(murals).toContain('cachedApi.getHomeShufflePicks(HOME_MURAL_ITEM_LIMIT)');
		expect(murals).toContain('async function loadSuggestionCandidates(requestKey: string)');
		expect(murals).toContain('cachedApi.getHomeSuggestions([], 50)');
		expect(murals).toContain('const requestKey = String(homePanelRefreshBucket())');
		expect(murals).toContain('const muralCandidateCache = {');
		expect(murals).not.toContain('listenHistorySeeds()');
		expect(murals).not.toContain('stableRandomOffsets');

		// Playing a track tile queues its panel and starts at the clicked track.
		expect(murals).toContain('const replaced = await api.replacePlaybackQueue(');
		expect(murals).toContain('trackIds.map((track_id) => ({ track_id })),');
		expect(murals).toContain('await api.playQueueItem(selected.id);');
		// Native buttons: Enter/Space activate, so no keydown handler doubles it.
		expect(murals).toContain('onclick={() => activateTile(item, panel)}');
		expect(murals).toContain('oncontextmenu={(event) => openTileMenu(event, item)}');
		expect(murals).not.toContain('onkeydown');
		expect(murals).toContain('openContextMenu(event, buildTrackMenu(item.track), item.title)');

		// Artwork: baked, then lazy-resolved, then the cached peek, always through
		// ArtworkImage with a fallback at the row/tile TIDAL size.
		expect(murals).toContain("import { lazyTidalArt, composeTidalArtQuery, peekTidalArt } from '$lib/actions/lazy-tidal-art'");
		expect(murals).toContain('peekTidalArt(composeTidalArtQuery(query.artist, query.title))');
		expect(murals).toContain('onResolve: (url) => (lazyArt[muralItemKey(item)] = url)');
		expect(murals).toContain('<ArtworkImage');
		expect(murals).toContain('size={320}');
		expect(murals).toContain('fallbackText={fallbackLetters(item.title)}');
		expect(murals).not.toContain('<img');

		// Text over art is white on a scrim in every theme; the collage filter is
		// a theme token so light themes are not dimmed.
		expect(murals).toContain('filter: var(--art-collage-filter)');
		expect(murals).toContain('color: #fff;');
		expect(murals).not.toContain('border: 1px solid');
		expect(murals.includes(String.fromCharCode(0x2014))).toBe(false);
	});

	test('library_home_track_rows_use_shared_artwork_fallbacks', () => {
		expect(libraryPage).toContain('class="home-track-list"');
		expect(libraryPage).toContain('{#each allSearchTrackPreview as track (track.id)}');
		expect(libraryPage).toContain('{#each recentTracks as track (track.id)}');
		expect(countOccurrences(libraryPage, 'className="ht-art-img"')).toBe(2);
		expect(countOccurrences(libraryPage, 'src={trackArt}')).toBe(2);
		expect(countOccurrences(libraryPage, 'size={320}')).toBeGreaterThanOrEqual(3);
		expect(countOccurrences(libraryPage, 'fallbackText={track.title.slice(0, 2).toUpperCase()}')).toBeGreaterThanOrEqual(3);
		expect(libraryPage).toContain(':global(.ht-art-img)');
		expect(libraryPage).not.toContain('<img class="ht-art-img" src={trackArt}');
		expect(libraryPage.includes(String.fromCharCode(0x2014))).toBe(false);
	});
});
