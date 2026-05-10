import { readFileSync } from 'node:fs';
import { describe, expect, test } from 'vitest';

describe('TIDAL mix link contracts', () => {
	test('TIDAL mix playback preserves album and artist ids for links', () => {
		const client = readFileSync('src/lib/api/client.ts', 'utf8');
		const player = readFileSync('src/lib/stores/player.ts', 'utf8');
		const trackUtils = readFileSync('src/lib/utils/track.ts', 'utf8');

		expect(client).toContain('artist_tidal_id: t.artist_tidal_id ?? null');
		expect(client).toContain('album_tidal_id: t.album_tidal_id ?? null');
		expect(player).toContain('rememberTidalPlayables(playable)');
		expect(player).toContain('album_tidal_id: track.album_tidal_id ?? null');
		expect(trackUtils).toContain('album_tidal_id: track.album_tidal_id ?? null');
	});

	test('TIDAL mix track endpoint includes album ids for now-playing links', () => {
		const routes = readFileSync('../noor-server/src/server/routes.rs', 'utf8');
		const mixRoute = routes.slice(
			routes.indexOf('async fn get_tidal_mix_tracks'),
			routes.indexOf('// ─── Last.fm scrobble auth', routes.indexOf('async fn get_tidal_mix_tracks'))
		);

		expect(mixRoute).toContain('"artist_tidal_id": t.artist.id');
		expect(mixRoute).toContain('"album_tidal_id": t.album.as_ref().map(|al| al.id)');
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
