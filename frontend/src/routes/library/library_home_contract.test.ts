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
		// Both random murals come from one server call fired on mount. The old
		// path derived offsets from the library totals and issued a single-row
		// request per pick, so it could not start until the library store had
		// paged in - the panels visibly popped in after everything else.
		expect(libraryPage).toContain('async function loadRandomPanelCandidates(requestKey: string)');
		expect(libraryPage).toContain('.getHomeShufflePicks(HOME_MURAL_ITEM_LIMIT)');
		expect(libraryPage).not.toContain('stableRandomOffsets');
		expect(libraryPage).not.toContain('function dailySalt');
		expect(libraryPage).not.toContain("cachedApi.getTracks('date_added', 'desc', 1, offset, true, false)");
		expect(libraryPage).not.toContain("cachedApi.getAlbums('title', 'asc', 1, offset, true)");
		expect(libraryPage).toContain('const HOME_PANEL_CACHE_REFRESH_MS = 5 * 60 * 1000');
		expect(libraryPage).toContain('const homePanelCandidateCache = {');
		expect(libraryPage).toContain('let randomPanelTracks = $state<Track[]>(homePanelCandidateCache.randomTracks)');
		expect(libraryPage).toContain('let randomPanelAlbums = $state<HomeAlbumCard[]>(homePanelCandidateCache.randomAlbums)');
		expect(libraryPage).toContain('randomPanelTracks.map(trackToMuralItem)');
		expect(libraryPage).toContain('randomPanelAlbums.map(albumToMuralItem)');
		expect(libraryPage).toContain('let suggestionTracks = $state<Track[]>(homePanelCandidateCache.suggestionTracks)');
		expect(libraryPage).toContain('let suggestionAlbums = $state<HomeAlbumCard[]>(homePanelCandidateCache.suggestionAlbums)');
		// Seedless by design: deriving seeds client-side made the request key churn
		// as the library store paged in during boot, refiring the fetch with a new
		// seed set (and so a new server cache key) each time. The server seeds
		// itself, so this fires once on mount in parallel with the library load.
		expect(libraryPage).toContain('async function loadSuggestionCandidates(requestKey: string)');
		expect(libraryPage).toContain('.getHomeSuggestions([], 50)');
		expect(libraryPage).toContain('const requestKey = String(homePanelRefreshBucket())');
		expect(libraryPage).not.toContain('listenHistorySeeds()');
		// The artist cap shapes the head of the mural; it must top up from what
		// it skipped rather than hand back a short panel (5 of 12 picks).
		expect(libraryPage).toContain('function capPerArtist(tracks: Track[], max: number, limit: number): Track[]');
		expect(libraryPage).toContain('capPerArtist(suggestionTracks, SUGGESTION_ARTIST_CAP, HOME_MURAL_ITEM_LIMIT)');
		expect(libraryPage).toContain('const SUGGESTION_ARTIST_CAP = 2');
		// Both murals render the server lists directly. The old same-artist
		// expansion tail-filled them with the album that was just played, which
		// is the bug this panel exists to avoid - it must not come back.
		expect(libraryPage).toContain('suggestionAlbums.slice(0, HOME_MURAL_ITEM_LIMIT).map(albumToMuralItem)');
		expect(libraryPage).not.toContain('sameArtistExpansion');
		expect(libraryPage).not.toContain('sameArtistScoredTracks');
		expect(libraryPage).not.toContain('combinedSuggestionTracks');
		expect(libraryPage).not.toContain('listenHistoryTrackScore');
		expect(libraryPage).toContain('homePanelCandidateCache.randomTracks = tracksForPanel');
		expect(libraryPage).toContain('homePanelCandidateCache.randomAlbums = albumsForPanel');
		expect(libraryPage).toContain('homePanelCandidateCache.suggestionTracks = suggestionTracks');
		expect(libraryPage).toContain('homePanelCandidateCache.suggestionAlbums = suggestionAlbums');
		expect(libraryPage).toContain('homePanelRefreshBucket()');
		expect(libraryPage).toContain('async function playHomeMuralTrack(item: HomeMuralItem, panel: HomeMuralPanel)');
		expect(libraryPage).toContain('const replaced = await api.replacePlaybackQueue(');
		expect(libraryPage).toContain('trackIds.map((track_id) => ({ track_id })),');
		expect(libraryPage).toContain('await api.playQueueItem(selected.id);');
		// Mural tiles activate on a single click; as native <button>s they also
		// fire on Enter/Space, so no separate keydown handler is needed (a second
		// one would double-activate on Enter).
		expect(libraryPage).toContain('onclick={() => openHomeMuralItem(item, panel)}');
		expect(libraryPage).not.toContain("onkeydown={(event) => { if (event.key === 'Enter') openHomeMuralItem(item, panel); }}");
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
		// Both carry an entrance-motion class too; see
		// scripts/library-route-motion-contract.test.mjs for that half.
		expect(libraryPage).toContain('class="home-mural-grid rise-in-shelf"');
		expect(libraryPage).toContain('class="home-mural-panel rise-in-card"');
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
