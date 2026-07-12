import { readFileSync } from 'node:fs';
import { describe, expect, test } from 'vitest';

function tidalMixRouteSource() {
	const source = readFileSync('../noor-server/src/server/routes/tidal_home_routes.rs', 'utf8');
	const start = source.indexOf('async fn get_tidal_mix_tracks');
	const end = source.indexOf('async fn get_tidal_home_module_items', start);
	return source.slice(start, end);
}

describe('TIDAL mix link contracts', () => {
	test('TIDAL mix playback preserves album and artist ids for links', () => {
		const client = readFileSync('src/lib/api/client.ts', 'utf8');
		const player = readFileSync('src/lib/stores/player.ts', 'utf8');
		const trackUtils = readFileSync('src/lib/utils/track.ts', 'utf8');

		expect(trackUtils).toContain('artist_tidal_id: track.artist_tidal_id ?? null');
		expect(trackUtils).toContain('album_tidal_id: track.album_tidal_id ?? null');
		expect(player).toContain('rememberTidalPlayables(playable)');
		expect(player).toContain('album_tidal_id: track.album_tidal_id ?? null');
		expect(trackUtils).toContain('album_tidal_id: track.album_tidal_id ?? null');
	});

	test('TIDAL mix track endpoint includes album ids for now-playing links', () => {
		const routes = readFileSync('../noor-server/src/server/routes.rs', 'utf8');
		const mixRoute = tidalMixRouteSource();

		expect(routes).toContain('"artist_tidal_id": t.artist.id');
		expect(routes).toContain('"album_tidal_id": t.album.as_ref().map(|al| al.id)');
		expect(mixRoute).toContain('super::tidal_track_playable_json(t, library_state, 640)');
	});

	test('TIDAL mix playback preserves local liked state when available', () => {
		const client = readFileSync('src/lib/api/client.ts', 'utf8');
		const player = readFileSync('src/lib/stores/player.ts', 'utf8');
		const routes = readFileSync('../noor-server/src/server/routes.rs', 'utf8');
		const mixRoute = tidalMixRouteSource();

		expect(client).toContain('is_favorite?: boolean');
		expect(player).toContain('track.is_favorite ?? false');
		expect(player).toContain('setOptimisticTidalTrack(playable[0])');
		expect(player).toContain('track_id: t.track_id');
		expect(player).toContain('local_id: t.local_id ?? null');
		expect(player).toContain('is_favorite: t.is_favorite');
		expect(routes).toContain('"is_favorite": library_state.map(|s| s.is_favorite).unwrap_or(false)');
		expect(mixRoute).toContain('queries::get_tidal_track_library_states');
	});

	test('quiet mode exposes title, album art, and album text as links', () => {
		const quietMode = readFileSync('src/lib/components/QuietMode.svelte', 'utf8');
		const metadata = readFileSync('src/lib/components/now-playing/NowPlayingMetadata.svelte', 'utf8');

		expect(quietMode).toContain('quietAlbumHref');
		expect(quietMode).toContain('class="quiet-art-link"');
		expect(metadata).toContain('trackRefFromTrack');
		expect(metadata).toContain('mediaHref(titleRef)');
		expect(metadata).toContain('class="np-title np-title-link"');
		expect(metadata).not.toContain('tidal.com/browse/track');
		expect(metadata).toContain('albumRefFromTrack');
		expect(metadata).toContain('buildMediaMenu(albumRef)');
	});
});
