import { beforeEach, describe, expect, test } from 'vitest';
import type {
	AtlasArtistCard,
	AtlasSceneCard,
	ExploreVideo,
	SongVersionsResponse,
	VideoConcert,
} from '$lib/api/video_explore_types';
import {
	EMPTY_STAGE,
	clearStageCache,
	loadStage,
	needsFallback,
	stepOutNeighbors,
	type StageClient,
	type StageRows,
} from './stage';

function video(id: number, artistId = 5, title = `Song ${id}`): ExploreVideo {
	return {
		tidal_id: id,
		title,
		duration_ms: 200_000,
		artist_id: artistId,
		artist_name: `Artist ${artistId}`,
		album_tidal_id: null,
		artwork_url: null,
		quality: null,
		explicit: null,
		type: 'video',
	};
}

const notImplemented = () => Promise.reject(Object.assign(new Error('not implemented'), { status: 501 }));

function card(id: number, extra: Partial<AtlasArtistCard> = {}): AtlasArtistCard {
	return {
		id,
		name: `Artist ${id}`,
		scene_id: 9,
		scene_name: 'Pop',
		videos: 10,
		playable: 10,
		hours: 1,
		light: 2,
		charted: true,
		genres: [],
		best: [video(id * 100, id)],
		neighbors: [],
		...extra,
	};
}

beforeEach(() => clearStageCache());

describe('one step out', () => {
	test('keeps unlit neighbours, strongest first, never the artist itself', () => {
		const picked = stepOutNeighbors(card(1, {
			neighbors: [
				{ id: 2, name: 'B', source: 'tidal', weight: 0.4, light: 1 },
				{ id: 3, name: 'C', source: 'lastfm', weight: 0.9, light: 3 },
				{ id: 4, name: 'D', source: 'featured', weight: 0.8, light: 0 },
				{ id: 1, name: 'Self', source: 'tidal', weight: 1, light: 1 },
				{ id: 2, name: 'B', source: 'lastfm', weight: 0.2, light: 1 },
			],
		}));
		expect(picked.map((n) => n.id)).toEqual([4, 2]);
	});
});

describe('stage rows', () => {
	const current = video(500, 5, 'Swear It Again (Live)');

	function client(overrides: Partial<StageClient>): StageClient {
		return {
			getVideoSongByVideo: notImplemented,
			getVideoConcert: notImplemented,
			getVideoAtlasArtist: notImplemented,
			getVideoAtlasScene: notImplemented,
			...overrides,
		};
	}

	async function run(api: StageClient): Promise<StageRows> {
		let rows: StageRows = { ...EMPTY_STAGE };
		await loadStage(api, current, (patch) => (rows = { ...rows, ...patch }));
		return rows;
	}

	test('every endpoint answering 501 leaves the rows empty and asks for the fallback', async () => {
		const rows = await run(client({}));
		expect(rows).toEqual(EMPTY_STAGE);
		expect(needsFallback(rows)).toBe(true);
	});

	test('builds concert, scene, step-out and versions rows', async () => {
		const song: SongVersionsResponse = {
			song_key: 'id:5:swear it again',
			title: 'Swear It Again',
			artist_id: 5,
			artist_name: 'Westlife',
			song_year: 1999,
			versions: [
				{ video: video(501), cut: 'official', cut_year: 1999, label: 'Official video', concert: null, best: true },
				{ video: current, cut: 'live', cut_year: 2006, label: 'Live at Croke Park', concert: { id: 77, label: 'Live at Croke Park' }, best: false },
			],
		};
		const concert: VideoConcert = {
			id: 77, artist_id: 5, artist_name: 'Westlife', year: 2006, label: 'Live at Croke Park',
			video_count: 3, total_s: 900, videos: [video(499), current, video(502)],
		};
		const scene: AtlasSceneCard = {
			scene: { id: 9, name: 'Pop', exemplars: [], tags: [], hue: 200, x: 0, y: 0, r: 1, videos: 10, artists: 3, hours: 1, channel_no: null, lit_artists: 0, seen_videos: 0 },
			top_artists: [],
			sample: [current, video(600, 6), video(601, 7)],
		};
		const asked: number[] = [];
		const rows = await run(client({
			getVideoSongByVideo: async (id) => { asked.push(id); return song; },
			getVideoConcert: async () => concert,
			getVideoAtlasScene: async () => scene,
			getVideoAtlasArtist: async (id) => id === 5
				? card(5, { neighbors: [{ id: 8, name: 'Boyzone', source: 'lastfm', weight: 0.8, light: 1 }] })
				: card(id),
		}));
		expect(asked).toEqual([500]);
		expect(rows.versions?.others.map((v) => v.video.tidal_id)).toEqual([501]);
		expect(rows.concert?.id).toBe(77);
		expect(rows.scene?.videos.map((v) => v.tidal_id)).toEqual([600, 601]);
		expect(rows.stepOut.map((pick) => [pick.neighbor.id, pick.video.tidal_id])).toEqual([[8, 800]]);
		expect(needsFallback(rows)).toBe(false);
	});

	test('an unknown video (404) leaves the versions row empty', async () => {
		const rows = await run(client({
			getVideoSongByVideo: async () => { throw Object.assign(new Error('song not found'), { status: 404 }); },
		}));
		expect(rows.versions).toBeNull();
	});

	test('a cold link without an artist id takes the artist from the song', async () => {
		const bare = { ...current, artist_id: null };
		const song: SongVersionsResponse = {
			song_key: 'id:5:swear it again', title: 'Swear It Again', artist_id: 5, artist_name: 'Westlife', song_year: null,
			versions: [{ video: bare, cut: 'live', cut_year: null, label: 'Live', concert: null, best: true }],
		};
		const askedArtists: number[] = [];
		let rows: StageRows = { ...EMPTY_STAGE };
		await loadStage(client({
			getVideoSongByVideo: async () => song,
			getVideoAtlasArtist: async (id) => { askedArtists.push(id); return card(id); },
			getVideoAtlasScene: async () => ({ scene: { id: 9 } as never, top_artists: [], sample: [video(700, 7)] }),
		}), bare, (patch) => (rows = { ...rows, ...patch }));
		expect(askedArtists[0]).toBe(5);
		expect(rows.scene?.videos.map((v) => v.tidal_id)).toEqual([700]);
	});

	test('a stale video never writes rows', async () => {
		let rows: StageRows = { ...EMPTY_STAGE };
		await loadStage(
			client({ getVideoAtlasArtist: async () => card(5), getVideoAtlasScene: async () => ({ scene: { id: 9 } as never, top_artists: [], sample: [video(1)] }) }),
			current,
			(patch) => (rows = { ...rows, ...patch }),
			() => false
		);
		expect(rows).toEqual(EMPTY_STAGE);
	});
});
