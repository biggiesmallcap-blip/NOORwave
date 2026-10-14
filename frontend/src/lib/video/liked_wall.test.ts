import { afterEach, describe, expect, test, vi } from 'vitest';
import { api, type LikedVideo } from '$lib/api/client';
import { filterLikedWall, likedWallExtender, likedWallQueue, type LikedWallLens } from './liked_wall';

vi.mock('$lib/api/client', () => ({ api: { getLikedVideos: vi.fn() } }));

function liked(id: number, overrides: Partial<LikedVideo> = {}): LikedVideo {
	return {
		song_key: `song-${id}`,
		track_ids: [id],
		track_title: `Song ${id}`,
		artist_name: `Artist ${id}`,
		artist_tidal_id: id * 10,
		genre: null,
		album_year: null,
		liked_at: `2026-01-${String(id).padStart(2, '0')}`,
		versions: [{
			tidal_video_id: id * 100,
			video_title: `Song ${id} (Official Video)`,
			duration_ms: 200_000,
			artwork_url: null,
			release_year: null,
		}],
		...overrides,
	} as LikedVideo;
}

const everything: LikedWallLens = { query: '', genre: null, year: null, sort: 'recent' };

afterEach(() => vi.clearAllMocks());

describe('liked wall', () => {
	test('newest like first, one face per song', () => {
		const queue = likedWallQueue([liked(1), liked(3), liked(2)], everything);
		expect(queue.map((item) => item.tidal_id)).toEqual([300, 200, 100]);
		expect(queue[0]).toMatchObject({ artist_id: 30, artist_name: 'Artist 3', title: 'Song 3 (Official Video)' });
	});

	test('search, genre and year all narrow it', () => {
		const wall = [liked(1, { genre: 'Rock', album_year: 1999 }), liked(2, { genre: 'Pop' }), liked(3, { genre: 'Rock' })];
		expect(filterLikedWall(wall, { ...everything, genre: 'Rock' }).map((v) => v.song_key)).toEqual(['song-3', 'song-1']);
		expect(filterLikedWall(wall, { ...everything, year: 1999 }).map((v) => v.song_key)).toEqual(['song-1']);
		expect(filterLikedWall(wall, { ...everything, query: 'artist 2' }).map((v) => v.song_key)).toEqual(['song-2']);
	});

	test('the extender asks the server again under the lens of the click', async () => {
		const lens = { ...everything, genre: 'Rock' };
		const extend = likedWallExtender(lens, false);
		// A later change to the page's lens does not move a queue already playing.
		lens.genre = 'Pop';
		vi.mocked(api.getLikedVideos).mockResolvedValue({
			videos: [liked(1, { genre: 'Rock' }), liked(2, { genre: 'Pop' }), liked(4, { genre: 'Rock' })],
		} as Awaited<ReturnType<typeof api.getLikedVideos>>);
		expect((await extend()).map((item) => item.tidal_id)).toEqual([400, 100]);
		expect(api.getLikedVideos).toHaveBeenCalledTimes(1);
	});
});
