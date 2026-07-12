import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, '../..');
const libraryPage = readFileSync(join(root, 'routes/library/+page.svelte'), 'utf8');
const libraryHero = readFileSync(join(root, 'lib/components/LibraryHero.svelte'), 'utf8');

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
		expect(libraryHero).toContain('rgba(0,0,0,0.66) 0%');
		expect(libraryHero).toContain('rgba(0,0,0,0.08) 68%');
		expect(libraryHero).toContain('opacity: 0.96');
		expect(libraryHero).toContain('opacity: 0.22');
		expect(libraryHero).toContain('brightness(1.52)');
		expect(libraryHero).toContain('saturate(1.95)');
		expect(libraryHero).toContain('scale(1.045)');
		expect(libraryHero).toContain('.mural-panel--featured::after');
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

	test('library_home_adds_clickable_recommendation_mural_panels', () => {
		expect(libraryPage).toContain('homeMuralPanels');
		expect(libraryPage).toContain('Suggested tracks');
		expect(libraryPage).toContain('Suggested albums');
		expect(libraryPage).toContain('Listen history suggestions');
		expect(libraryPage).toContain('Random tracks');
		expect(libraryPage).toContain('Random albums');
		expect(libraryPage).toContain('stableRandomOffsets');
		expect(libraryPage).toContain("cachedApi.getTracks('date_added', 'desc', 1, offset, true, false)");
		expect(libraryPage).toContain("cachedApi.getAlbums('title', 'asc', 1, offset, true)");
		expect(libraryPage).toContain('const HOME_PANEL_CACHE_REFRESH_MS = 5 * 60 * 1000');
		expect(libraryPage).toContain('const homePanelCandidateCache = {');
		expect(libraryPage).toContain('let randomPanelTracks = $state<Track[]>(homePanelCandidateCache.randomTracks)');
		expect(libraryPage).toContain('let randomPanelAlbums = $state<HomeAlbumCard[]>(homePanelCandidateCache.randomAlbums)');
		expect(libraryPage).toContain('randomPanelTracks.map(trackToMuralItem)');
		expect(libraryPage).toContain('randomPanelAlbums.map(albumToMuralItem)');
		expect(libraryPage).toContain('let suggestionCandidateTracks = $state<Track[]>(homePanelCandidateCache.suggestionTracks)');
		expect(libraryPage).toContain('async function loadSuggestionCandidates(seedTracks: Track[], requestKey: string)');
		expect(libraryPage).toContain('cachedApi.getArtistTracks(id)');
		expect(libraryPage).toContain('cachedApi.getAlbumTracks(id)');
		expect(libraryPage).toContain('const scored = suggestionCandidateTracks');
		expect(libraryPage).toContain('homePanelCandidateCache.randomTracks = tracksForPanel');
		expect(libraryPage).toContain('homePanelCandidateCache.randomAlbums = albumsForPanel');
		expect(libraryPage).toContain('homePanelCandidateCache.suggestionTracks = candidates');
		expect(libraryPage).toContain('homePanelRefreshBucket()');
		expect(libraryPage).toContain('async function playHomeMuralTrack(item: HomeMuralItem, panel: HomeMuralPanel)');
		expect(libraryPage).toContain('api.replacePlaybackQueue(trackIds.map((track_id) => ({ track_id })), {');
		expect(libraryPage).toContain('await api.playQueueItem(selected.id);');
		// Mural tiles activate on double-click / Enter (parity with the library
		// track rows and the home-recs murals), not a single stray click.
		expect(libraryPage).toContain('ondblclick={() => openHomeMuralItem(item, panel)}');
		expect(libraryPage).toContain("onkeydown={(event) => { if (event.key === 'Enter') openHomeMuralItem(item, panel); }}");
		expect(libraryPage).toContain('oncontextmenu={(event) => openHomeMuralItemContextMenu(event, item)}');
		expect(libraryPage).toContain('void openAlbumDetail(found ?? albumFromHomeCard(card))');
		// Tiles resolve missing artwork lazily and paint previously-cached art on
		// first launch via the shared peek, so panels are never left artwork-less.
		expect(libraryPage).toContain("import { lazyTidalArt, composeTidalArtQuery, peekTidalArt } from '$lib/actions/lazy-tidal-art'");
		expect(libraryPage).toContain('function muralItemArtwork(item: HomeMuralItem): string | null');
		expect(libraryPage).toContain('peekTidalArt(composeTidalArtQuery(query.artist, query.title))');
		expect(libraryPage).toContain('onResolve: (url) => (lazyArt[muralItemKey(item)] = url)');
		expect(libraryPage).toContain('<ArtworkImage');
		expect(libraryPage).toContain('className="home-mural-art"');
		expect(libraryPage).toContain('src={muralArt}');
		expect(libraryPage).toContain('size={320}');
		expect(libraryPage).toContain('fallbackText={fallbackLetters(item.title)}');
		expect(libraryPage).toContain('decorative={true}');
		expect(libraryPage).toContain(':global(.home-mural-art)');
		expect(libraryPage).not.toContain('<img src={artUrl}');
		expect(libraryPage).not.toContain('homePanelArtUrl');
		expect(libraryPage).not.toContain('markHomePanelArtFailed');
		expect(libraryPage).not.toContain('failedHomePanelArtUrls');
		expect(libraryPage).toContain('class="home-mural-grid"');
		expect(libraryPage).toContain('class="home-mural-panel"');
		expect(libraryPage).not.toContain('Automix tracks');
		expect(libraryPage).not.toContain('Automix albums');
		expect(libraryPage).not.toContain('selected.push(...pickStableRandom($tracks');
		expect(libraryPage).not.toContain('selected.push(...pickStableRandom(allHomeAlbumCards');
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
