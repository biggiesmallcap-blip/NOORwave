import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, test } from 'vitest';

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, 'HomeRecommendationsShelf.svelte'), 'utf8');
const homePage = readFileSync(join(here, '../../../routes/+page.svelte'), 'utf8');
const client = readFileSync(join(here, '../../api/client.ts'), 'utf8');
const playTrending = readFileSync(join(here, '../../player/play_trending.ts'), 'utf8');
const serverRoutes = readFileSync(join(here, '../../../../../noor-server/src/server/routes.rs'), 'utf8');

describe('home recommendations shelf contract', () => {
	test('loads provider shelves independently after Home renders', () => {
		expect(homePage).toContain('HomeRecommendationsShelf');
		expect(source).toContain('onMount');
		expect(source).toContain('api.getLastfmStatus()');
		expect(source).toContain('api.getListenBrainzStatus()');
		expect(source).toContain('lastfm.value.recommendations');
		expect(source).toContain('listenbrainz.value.recommendations');
		expect(source).toContain('api.getHomeRecommendations()');
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
		expect(serverRoutes).toContain('track_get_similar_with_artist_fallback');
		expect(serverRoutes).toContain('recommendation_placeholder_item');
		expect(serverRoutes).toContain('RECOMMENDATION_HOME_CACHE_KEY: &str = "home:v6"');
		expect(serverRoutes).toContain('LASTFM_HOME_RECOMMENDATION_LIMIT: usize = 20');
		expect(serverRoutes).toContain('LASTFM_HOME_SEED_LIMIT: usize = 12');
		expect(serverRoutes).toContain('LASTFM_HOME_SIMILAR_LIMIT: usize = 20');
		expect(serverRoutes).toContain('LASTFM_HOME_ARTIST_LIMIT: usize = 20');
		expect(serverRoutes).toContain('LASTFM_HOME_ALBUM_LIMIT: usize = 20');
	});

	test('varies Last.fm seed reasons beyond stable top artists', () => {
		expect(serverRoutes).toContain('load_lastfm_track_seeds');
		expect(serverRoutes).toContain('load_lastfm_artist_seeds');
		expect(serverRoutes).toContain('user_recent_tracks');
		expect(serverRoutes).toContain('Because you played {} recently');
		expect(serverRoutes).toContain('Because you loved {}');
		expect(serverRoutes).toContain('Near your top artist {}');
		expect(serverRoutes).toContain('recommendation_seed_window');
	});

	test('splits Last.fm tracks, artists, and albums into separate panels', () => {
		expect(client).toContain("entity_type?: 'track' | 'artist' | 'album' | string");
		expect(serverRoutes).toContain('Last.fm recommended tracks');
		expect(serverRoutes).toContain('Last.fm recommended artists');
		expect(serverRoutes).toContain('Last.fm recommended albums');
		expect(serverRoutes).toContain('fetch_lastfm_artist_recommendations');
		expect(serverRoutes).toContain('fetch_lastfm_album_recommendations');
		expect(serverRoutes).toContain('resolve_recommendation_artist_item');
		expect(serverRoutes).toContain('resolve_recommendation_album_item');
		expect(source).toContain("shelf.entity_type === 'artist'");
		expect(source).toContain("shelf.entity_type === 'album'");
		expect(source).toContain('itemMetric');
		expect(source).toContain('${index + 1} of ${count}');
		expect(source).toContain('openRecommendationItem');
		expect(source).toContain('local_artist_id');
		expect(source).toContain('local_album_id');
	});

	test('plays local matches directly and resolves unresolved Last.fm items through TIDAL', () => {
		expect(source).toContain('playTrackNow');
		expect(source).toContain('item.local_track_id');
		expect(source).toContain('playChartTidalTrack');
		expect(source).toContain('tidal_id: item.tidal_id ?? 0');
		expect(source).toContain('Resolve on TIDAL');
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
