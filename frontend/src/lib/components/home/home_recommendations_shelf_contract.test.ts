import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, test } from 'vitest';
import type { ProviderRecommendationItem, TidalSearchResults } from '$lib/api/client';
import {
	recommendationActionLabel,
	recommendationHrefFromSearch,
	recommendationKnownHref,
	recommendationSearchHref,
} from './recommendation_navigation';

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, 'HomeRecommendationsShelf.svelte'), 'utf8');
const recommendationMenu = readFileSync(join(here, 'recommendation_menu.ts'), 'utf8');
const homePage = readFileSync(join(here, '../../../routes/+page.svelte'), 'utf8');
const client = readFileSync(join(here, '../../api/client.ts'), 'utf8');
const playTrending = readFileSync(join(here, '../../player/play_trending.ts'), 'utf8');
const serverRoutes = readFileSync(join(here, '../../../../../noor-server/src/server/routes.rs'), 'utf8');
const homeRoutes = readFileSync(join(here, '../../../../../noor-server/src/server/routes/home_routes.rs'), 'utf8');

const emptySearchResults: TidalSearchResults = { tracks: [], albums: [], artists: [], videos: [] };

function rec(overrides: Partial<ProviderRecommendationItem>): ProviderRecommendationItem {
	return {
		provider: 'lastfm',
		entity_type: 'track',
		local_track_id: null,
		tidal_id: null,
		title: 'Title',
		artist_name: 'Artist',
		album_title: null,
		artwork_url: null,
		reason: 'Near your top track',
		playable: false,
		...overrides,
	};
}

describe('home recommendations shelf contract', () => {
	test('loads provider shelves independently after Home renders', () => {
		expect(homePage).toContain('HomeRecommendationsShelf');
		expect(source).toContain('onMount');
		expect(source).toContain('cachedApi.getLastfmStatus()');
		expect(source).toContain('cachedApi.getListenBrainzStatus()');
		expect(source).toContain('lastfm.value.recommendations');
		expect(source).toContain('listenbrainz.value.recommendations');
		expect(source).toContain('cachedApi.getHomeRecommendations()');
		expect(source).toContain('let loadSeq = 0;');
		expect(source).toContain('return () => { loadSeq += 1; };');
		expect(source).toContain('const seq = ++loadSeq;');
		expect(source).toContain('if (seq !== loadSeq) return;');
		expect(source).not.toContain('Boolean(lastfm.value.scrobbling)');
		expect(source).not.toContain('Boolean(listenbrainz.value.scrobbling)');
		expect(client).toContain('/api/home/recommendations');
	});

	test('has loading, empty, and error states for provider data', () => {
		expect(source).toContain("type State = 'hidden' | 'loading' | 'ready' | 'empty' | 'error'");
		expect(source).toContain("viewState === 'hidden'");
		expect(source).toContain("viewState === 'loading'");
		expect(source).toContain("viewState === 'empty'");
		expect(source).toContain("viewState === 'error'");
		expect(source).toContain('No profile recommendations yet');
		expect(source).toContain('Retry');
	});

	test('renders Last.fm recommendations through the charts mural carousel', () => {
		expect(source).toContain('ChartMural');
		expect(source).toContain('type ChartMuralItem');
		expect(source).toContain('PANEL_LIMIT = 20');
		expect(source).toContain('visibleShelves');
		expect(source).toContain('shelfMuralItems');
		expect(serverRoutes).toContain('home_routes::get_home_recommendations');
		expect(homeRoutes).toContain('track_get_similar_with_artist_fallback');
		expect(homeRoutes).toContain('recommendation_placeholder_item');
		// Pinned so a change to the resolved item shape has to bump the key.
		// The payload is cached for six hours, so shipping new resolution logic
		// without a bump leaves existing installs on the old output until it
		// expires - which is exactly what this assertion is here to catch.
		expect(homeRoutes).toContain('RECOMMENDATION_HOME_CACHE_KEY: &str = "home:v9"');
		// The track shelf stays at 20 because the mural is a fixed 10x2 grid;
		// see `layout-count-20` in ChartMural.svelte.
		expect(homeRoutes).toContain('LASTFM_HOME_RECOMMENDATION_LIMIT: usize = 20');
		// The rails are not bound by the mural, so they carry the deeper set that
		// the fan-out was already generating and discarding.
		expect(homeRoutes).toContain('LASTFM_HOME_ARTIST_LIMIT: usize = 50');
		expect(homeRoutes).toContain('LASTFM_HOME_ALBUM_LIMIT: usize = 50');
	});

	test('the shelf soft-caps in place and sends the rest to a grid page', () => {
		expect(source).toContain('PANEL_LIMIT = 20');
		expect(source).toContain('function hasMoreThanShelf');
		expect(source).toContain('shelf.items.length > PANEL_LIMIT');
		expect(source).toContain('/recommendations/${recommendationShelfSlug(shelf)}');
	});

	test('the visible window rotates over the cached shelf instead of re-fetching it', () => {
		// Rebuilding a shelf costs about a hundred Last.fm calls plus up to fifty
		// TIDAL searches, so the six-hour lease stays. The cheap knob is which
		// twenty of the fifty are on screen - those fifty are already in the
		// payload, so rotating the window costs no requests at all.
		expect(source).toContain('rotatingWindow(shelf.items, PANEL_LIMIT, viewRotation * PANEL_LIMIT)');
		expect(source).toContain('VIEW_ROTATION_MS = 2 * 60 * 60 * 1000');
		// Read once on init. Recomputed inside a `$derived` it would move the
		// window under a user who is mid-scroll.
		expect(source).toContain('const viewRotation = rotationForPeriod(VIEW_ROTATION_MS);');
		expect(source).not.toContain('shelf.items.slice(0, PANEL_LIMIT)');
		// Clock-derived, not random: the rail must hold still while being read.
		expect(source).not.toContain('Math.random()');
		// The server lease is what this is sized against, so keep them together.
		expect(homeRoutes).toContain('RECOMMENDATION_FULL_TTL_SECS: i64 = 6 * 60 * 60');
	});

	test('the View all grid presents cards exactly as the rail does', () => {
		const grid = readFileSync(
			join(here, '../../../routes/recommendations/[shelf]/+page.svelte'),
			'utf8',
		);
		// Albums open the mini detail popup and carry the corner play badge, the
		// same as a Library album card. Artists carry neither, because no artist
		// card in the app has a play affordance; clicking one opens the artist.
		expect(grid).toContain('(albumPopupItem = item)');
		expect(grid).toContain('void openRecommendationItem(item)');
		expect(grid).toContain('`Open ${item.title}`');
		expect(grid).toContain('{#if isAlbum}');
		expect(grid).toContain('<PlayOverlay position="corner" size="sm" />');
		// Same hover as `.rec-card`: the card lifts, the artwork shadow deepens.
		expect(grid).toContain('transform: translateY(-4px)');
		expect(grid).toContain('box-shadow: 0 12px 26px -6px rgba(0, 0, 0, 0.5)');
		expect(grid).toContain('box-shadow: 0 2px 8px rgba(0, 0, 0, 0.22)');
		// Artists: circular avatar, centred label.
		expect(grid).toContain('border-radius: 50%');
		expect(grid).toContain('.rec-tile.artist');
		// Shared page chrome rather than a bespoke hero.
		expect(grid).toContain('PageHeader');
		expect(grid).toContain('variant="editorial"');
		expect(grid).toContain('muted-line');
		expect(grid).not.toContain('class="hero"');
	});

	test('the shelf and its View all page share one menu builder', () => {
		// Two surfaces rendering the same cards must open the same menu, per the
		// rule that every asset reference carries the shared context menu.
		expect(source).toContain("from '$lib/components/home/recommendation_menu'");
		expect(source).toContain('recommendationItemMenu');
		expect(source).not.toContain('function recommendationItemMenu');
		expect(homeRoutes).toContain('LASTFM_HOME_SEED_LIMIT: usize = 12');
		expect(homeRoutes).toContain('LASTFM_HOME_SIMILAR_LIMIT: usize = 20');
	});

	test('varies Last.fm seed reasons beyond stable top artists', () => {
		expect(homeRoutes).toContain('load_lastfm_track_seeds');
		expect(homeRoutes).toContain('load_lastfm_artist_seeds');
		expect(homeRoutes).toContain('user_recent_tracks');
		expect(homeRoutes).toContain('Because you played {} recently');
		expect(homeRoutes).toContain('Because you loved {}');
		expect(homeRoutes).toContain('Near your top artist {}');
		expect(homeRoutes).toContain('recommendation_seed_window');
	});

	test('splits Last.fm tracks, artists, and albums into separate panels', () => {
		expect(client).toContain("entity_type?: 'track' | 'artist' | 'album' | string");
		expect(homeRoutes).toContain('Last.fm recommended tracks');
		expect(homeRoutes).toContain('Last.fm recommended artists');
		expect(homeRoutes).toContain('Last.fm recommended albums');
		expect(homeRoutes).toContain('fetch_lastfm_artist_recommendations');
		expect(homeRoutes).toContain('fetch_lastfm_album_recommendations');
		expect(homeRoutes).toContain('resolve_recommendation_artist_item');
		expect(homeRoutes).toContain('resolve_recommendation_album_item');
		expect(source).toContain("shelf.entity_type === 'artist'");
		expect(source).toContain("shelf.entity_type === 'album'");
		expect(source).toContain('itemMetric');
		expect(source).toContain('${index + 1} of ${count}');
		expect(source).toContain('openRecommendationItem');
		expect(source).toContain('local_artist_id');
		expect(source).toContain('local_album_id');
	});

	test('resolves unresolved artist and album recommendations before falling back to search', () => {
		const unresolvedArtist = rec({ entity_type: 'artist', title: 'Amara ctk100', artist_name: 'Amara ctk100' });
		expect(recommendationKnownHref(unresolvedArtist)).toBeNull();
		expect(recommendationActionLabel(unresolvedArtist)).toBe('Play artist');
		expect(recommendationHrefFromSearch(unresolvedArtist, {
			...emptySearchResults,
			artists: [{
				tidal_id: 123,
				name: 'Amara CTK100',
				artwork_url: null,
				local_id: null,
				in_library: false,
			}],
		})).toBe('/tidal/artists/123');
		expect(recommendationSearchHref(unresolvedArtist)).toBe('/search?q=Amara%20ctk100');

		const unresolvedAlbum = rec({
			entity_type: 'album',
			title: 'In the Beginning There Was Rhythm',
			artist_name: 'Switch Angel',
		});
		expect(recommendationActionLabel(unresolvedAlbum)).toBe('Play album');
		expect(recommendationHrefFromSearch(unresolvedAlbum, {
			...emptySearchResults,
			albums: [{
				tidal_id: 456,
				title: 'In The Beginning There Was Rhythm',
				artist_name: 'Switch Angel',
				artwork_url: null,
				local_id: 77,
				in_library: true,
			}],
		})).toBe('/albums/77');
		expect(recommendationHrefFromSearch(unresolvedAlbum, emptySearchResults)).toBeNull();
	});

	test('an artist search that does not clearly match goes to search, not to a guess', () => {
		const item = rec({ entity_type: 'artist', title: 'Nova', artist_name: 'Nova' });
		const artist = (tidal_id: number, name: string) => ({
			tidal_id,
			name,
			artwork_url: null,
			local_id: null,
			in_library: false,
		});

		// Names that merely spell "nova" are not matches: overlap compares whole
		// tokens, so neither "Novastar" nor "Casanova" qualifies. With no
		// candidate left this used to return artists[0], so clicking "Nova"
		// opened whatever TIDAL ranked first with no sign a guess was made.
		expect(
			recommendationHrefFromSearch(item, {
				...emptySearchResults,
				artists: [artist(1, 'Novastar'), artist(3, 'Casanova')],
			}),
		).toBeNull();

		// A trailing qualifier is still a match - that is what overlap is for.
		expect(
			recommendationHrefFromSearch(item, {
				...emptySearchResults,
				artists: [artist(1, 'Novastar'), artist(4, 'Nova (UK)')],
			}),
		).toBe('/tidal/artists/4');

		// A single result is a safe bet: the search was keyed on the name and
		// nothing else came back to confuse it.
		expect(
			recommendationHrefFromSearch(item, {
				...emptySearchResults,
				artists: [artist(9, 'NOVA (Official)')],
			}),
		).toBe('/tidal/artists/9');

		// An exact fold still wins outright, whatever else is in the list.
		expect(
			recommendationHrefFromSearch(item, {
				...emptySearchResults,
				artists: [artist(1, 'Novastar'), artist(2, 'nova'), artist(3, 'Casanova')],
			}),
		).toBe('/tidal/artists/2');
	});

	test('resolves albums/artists past exact-match drift instead of dumping to search', () => {
		// Edition suffix drift: Last.fm "Demon Days" vs TIDAL "Demon Days (Deluxe)".
		const album = rec({ entity_type: 'album', title: 'Demon Days', artist_name: 'Gorillaz' });
		expect(recommendationHrefFromSearch(album, {
			...emptySearchResults,
			albums: [{
				tidal_id: 999,
				title: 'Demon Days (Deluxe Edition)',
				artist_name: 'Gorillaz',
				artwork_url: null,
				local_id: null,
				in_library: false,
			}],
		})).toBe('/tidal/albums/999');

		// A different artist's same-named album must NOT be accepted.
		expect(recommendationHrefFromSearch(album, {
			...emptySearchResults,
			albums: [{
				tidal_id: 1000,
				title: 'Demon Days',
				artist_name: 'Some Tribute Band',
				artwork_url: null,
				local_id: null,
				in_library: false,
			}],
		})).toBeNull();

		// Artist name drift still lands on the artist.
		const artist = rec({ entity_type: 'artist', title: 'MF DOOM', artist_name: 'MF DOOM' });
		expect(recommendationHrefFromSearch(artist, {
			...emptySearchResults,
			artists: [{
				tidal_id: 321,
				name: 'MF DOOM (Daniel Dumile)',
				artwork_url: null,
				local_id: null,
				in_library: false,
			}],
		})).toBe('/tidal/artists/321');
	});

	test('tiles play on double-click and expose the shared context menus', () => {
		expect(source).toContain('onItemActivate');
		expect(source).toContain('activateItem');
		expect(source).toContain('onItemContext');
		expect(source).toContain('onCardContext');
		expect(source).toContain('openContextMenu');
		expect(source).toContain('recommendationItemMenu');
		// The builders moved into recommendation_menu.ts when the View all page
		// started sharing them, so assert them where they now live.
		expect(recommendationMenu).toContain('buildTrackMenu');
		expect(recommendationMenu).toContain('buildTidalTrackMenu');
		expect(recommendationMenu).toContain('buildAlbumMenu');
		expect(recommendationMenu).toContain('buildArtistMenu');
		// The shared mural exposes the double-click hook the shelf relies on.
		const mural = readFileSync(join(here, '../charts/ChartMural.svelte'), 'utf8');
		expect(mural).toContain('onItemActivate');
		expect(mural).toContain('ondblclick');
	});

	test('plays albums and artists in place rather than navigating away', () => {
		expect(source).toContain('playRecommendationAlbum');
		expect(source).toContain('playRecommendationArtist');
		const helpers = readFileSync(join(here, '../../player/play_recommendations.ts'), 'utf8');
		expect(helpers).toContain('resolveRecommendationAlbum');
		expect(helpers).toContain('resolveRecommendationArtist');
		expect(helpers).toContain('playTidalAlbum');
		expect(helpers).toContain('playAlbum');
		expect(helpers).toContain('playArtist');
		expect(helpers).toContain('getTidalArtistProfile');
		expect(helpers).toContain('playTidalTracksNow');
	});

	test('plays local matches directly and resolves unresolved Last.fm items through TIDAL', () => {
		expect(source).toContain('playTrackNow');
		expect(source).toContain('item.local_track_id');
		expect(source).toContain('playChartTidalTrack');
		expect(recommendationMenu).toContain('tidal_id: item.tidal_id ?? 0');
		expect(recommendationActionLabel(rec({ entity_type: 'track' }))).toBe('Resolve on TIDAL');
	});

	test('can play the visible recommendation set through standard TIDAL mix playback', () => {
		expect(source).toContain('Play all');
		expect(source).toContain('playingAllShelves');
		expect(source).toContain('playAllRecommendations');
		expect(source).toContain('playChartTidalTracks(items.map(itemToTidalPlayable)');
		expect(source).not.toContain('api.queueAppendMany(queueRequests)');
		expect(source).not.toContain('api.clearQueue()');
		expect(playTrending).toContain('resolveChartTidalTrack');
		expect(playTrending).toContain('api.searchTidal(q, 1)');
		expect(playTrending).toContain('track.stream_ready !== false');
		expect(playTrending).toContain('playTidalTracksNow(playable, label)');
	});

	test('recommended albums open the shared Library popup on both surfaces', () => {
		const popup = readFileSync(join(here, 'RecommendationAlbumPopup.svelte'), 'utf8');
		const detail = readFileSync(join(here, 'recommendation_album_detail.ts'), 'utf8');
		const grid = readFileSync(join(here, '../../../routes/recommendations/[shelf]/+page.svelte'), 'utf8');
		const albumPopup = readFileSync(join(here, '../AlbumDetailPopup.svelte'), 'utf8');

		// The rail card and the View all tile both open the popup rather than
		// navigating, so a recommended album behaves like a Library album.
		expect(source).toContain('albumPopupItem = entry.item');
		expect(source).toContain('RecommendationAlbumPopup');
		expect(grid).toContain('albumPopupItem = item');
		expect(grid).toContain('RecommendationAlbumPopup');

		// One popup instance is about one album: it loads on mount and the parents
		// key it, so picking another album mounts a fresh one instead of asking a
		// half-loaded popup to swap albums.
		expect(popup).toContain('onMount(');
		expect(popup).not.toContain('$effect(');
		expect(source).toContain('{#key albumPopupItem}');
		expect(grid).toContain('{#key albumPopupItem}');

		// A recommended album is usually not owned, so the popup takes explicit
		// play handlers and an isLocal flag instead of assuming a local album id.
		expect(albumPopup).toContain('isLocal = true');
		expect(albumPopup).toContain('onPlay?:');
		expect(albumPopup).toContain('onShuffle?:');
		expect(albumPopup).toContain('onPlayFrom?:');

		// The endpoint sends track_id 0 for an unowned track, so `??` gave every
		// row the same key and the popup threw each_key_duplicate and rendered
		// nothing. Keep the truthiness check.
		expect(detail).toContain('track.track_id ? track.track_id : -track.tidal_id');
		expect(detail).not.toContain('track.track_id ??');

		// An album that is genuinely not on TIDAL says so in place. Navigating to
		// /search for it threw the user off Home for a click that promised a popup.
		expect(popup).toContain("showToast(`Couldn't find \"${target.title}\" on Tidal`, 'error')");
		expect(popup).not.toContain('openRecommendationItem');
	});

	test('album cards that cannot open never reach the client', () => {
		// Last.fm recommends singles, regional pressings and anthologies TIDAL has
		// no album for. Those cards cannot open, play or queue, so the endpoint
		// resolves the album id server-side and drops the ones with nothing behind
		// them, rather than shipping tiles that fail on click.
		expect(homeRoutes).toContain('fn album_id_from_catalog');
		expect(homeRoutes).toContain('fn drop_unresolvable_albums');
		expect(homeRoutes).toContain('drop_unresolvable_albums(&mut items);');
		expect(homeRoutes).toContain('|| needs_album_id(item)');
		// Same refusal-to-guess rules as the client matcher, sharing the folding
		// and word-boundary overlap helpers.
		expect(homeRoutes).toContain('names_overlap');

		// Last.fm does not distinguish a single from an album, so the ones TIDAL
		// only carries as a track keep their card, say "Single" on it, and seed
		// song radio - there is no tracklist to open.
		expect(homeRoutes).toContain('fn single_id_from_catalog');
		expect(homeRoutes).toContain('"is_single".to_string()');
		expect(homeRoutes).toContain('fn album_item_is_dead');
		const menu = readFileSync(join(here, 'recommendation_menu.ts'), 'utf8');
		expect(menu).toContain('export function isRecommendationSingle');
		expect(menu).toContain('startTidalSongRadio');
		const grid = readFileSync(
			join(here, '../../../routes/recommendations/[shelf]/+page.svelte'),
			'utf8',
		);
		for (const surface of [source, grid]) {
			expect(surface).toContain('isRecommendationSingle');
			expect(surface).toContain('playRecommendationSingle');
			expect(surface).toContain('>Single</span>');
		}
	});

	test('resolves albums against enough TIDAL results to find them', () => {
		const helpers = readFileSync(join(here, '../../player/play_recommendations.ts'), 'utf8');
		// The top 5 album hits are the artist's most-played, not the one asked for,
		// so most shelf items missed. 12 is ARTWORK_SEARCH_LIMIT server-side, so
		// both ends share a tidal_search_cache row, and 12 resolves the same items
		// as 20 across the whole shelf.
		expect(helpers).toContain('api.searchTidal(recommendationSearchQuery(item), 12)');
		expect(homeRoutes).toContain('ARTWORK_SEARCH_LIMIT: i32 = 12');
		expect(helpers).not.toContain('api.searchTidal(recommendationSearchQuery(item), 5)');
	});
});
