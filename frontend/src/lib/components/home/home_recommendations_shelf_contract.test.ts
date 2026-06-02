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
		expect(homeRoutes).toContain('RECOMMENDATION_HOME_CACHE_KEY: &str = "home:v6"');
		expect(homeRoutes).toContain('LASTFM_HOME_RECOMMENDATION_LIMIT: usize = 20');
		expect(homeRoutes).toContain('LASTFM_HOME_SEED_LIMIT: usize = 12');
		expect(homeRoutes).toContain('LASTFM_HOME_SIMILAR_LIMIT: usize = 20');
		expect(homeRoutes).toContain('LASTFM_HOME_ARTIST_LIMIT: usize = 20');
		expect(homeRoutes).toContain('LASTFM_HOME_ALBUM_LIMIT: usize = 20');
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
		expect(recommendationActionLabel(unresolvedArtist)).toBe('Resolve artist');
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
		expect(recommendationActionLabel(unresolvedAlbum)).toBe('Resolve album');
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

	test('plays local matches directly and resolves unresolved Last.fm items through TIDAL', () => {
		expect(source).toContain('playTrackNow');
		expect(source).toContain('item.local_track_id');
		expect(source).toContain('playChartTidalTrack');
		expect(source).toContain('tidal_id: item.tidal_id ?? 0');
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
});
