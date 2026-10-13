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
	baseTitle,
	clearStageCache,
	loadStage,
	needsFallback,
	songKeyFor,
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

describe('song key mirror', () => {
	test('matches the server rule: artist id, bare title, normalized', () => {
		expect(songKeyFor({ title: 'Swear It Again (Live at Croke Park)', artist_id: 5, artist_name: 'Westlife' })).toBe('id:5:swear it again');
		expect(songKeyFor({ title: "Don't Stop Me Now [Official Video]", artist_id: 2, artist_name: null })).toBe('id:2:don t stop me now');
		expect(songKeyFor({ title: 'Despacito', artist_id: null, artist_name: ' Luis Fonsi ' })).toBe('name:luis fonsi:despacito');
		expect(songKeyFor({ title: '(Intro)', artist_id: 1, artist_name: null })).toBe('id:1:intro');
		expect(songKeyFor({ title: 'Ao Vivo - Garota', artist_id: 3, artist_name: null })).toBe('id:3:ao vivo garota');
	});

	test('base title drops the bracketed suffix', () => {
		expect(baseTitle('Swear It Again (Live)')).toBe('Swear It Again');
		expect(baseTitle('(Intro)')).toBe('(Intro)');
	});
});

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
			getVideoSongVersions: notImplemented,
			getVideoConcert: notImplemented,
			getVideoAtlasArtist: notImplemented,
			getVideoAtlasScene: notImplemented,
			getVideoArchive: notImplemented,
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
		const asked: string[] = [];
		const rows = await run(client({
			getVideoSongVersions: async (key) => { asked.push(key); return song; },
			getVideoConcert: async () => concert,
			getVideoAtlasScene: async () => scene,
			getVideoAtlasArtist: async (id) => id === 5
				? card(5, { neighbors: [{ id: 8, name: 'Boyzone', source: 'lastfm', weight: 0.8, light: 1 }] })
				: card(id),
		}));
		expect(asked).toEqual(['id:5:swear it again']);
		expect(rows.versions?.others.map((v) => v.video.tidal_id)).toEqual([501]);
		expect(rows.concert?.id).toBe(77);
		expect(rows.scene?.videos.map((v) => v.tidal_id)).toEqual([600, 601]);
		expect(rows.stepOut.map((pick) => [pick.neighbor.id, pick.video.tidal_id])).toEqual([[8, 800]]);
		expect(needsFallback(rows)).toBe(false);
	});

	test('falls back to an archive search when the mirrored key misses', async () => {
		const song: SongVersionsResponse = {
			song_key: 'server-key', title: 'Swear It Again', artist_id: 5, artist_name: 'Westlife', song_year: null,
			versions: [
				{ video: current, cut: 'live', cut_year: null, label: 'Live', concert: null, best: true },
				{ video: video(9), cut: 'official', cut_year: null, label: 'Official', concert: null, best: false },
			],
		};
		const queries: unknown[] = [];
		const rows = await run(client({
			getVideoSongVersions: async (key) => {
				if (key !== 'server-key') throw Object.assign(new Error('missing'), { status: 404 });
				return song;
			},
			getVideoArchive: async (query) => {
				queries.push(query);
				return {
					total: 1,
					items: [{ song_key: 'server-key', title: 'Swear It Again', artist_id: 5, artist_name: 'Westlife', best: current, cut_count: 2, cuts: ['live'], song_year: null, scene_id: null, light: 2 }],
					facets: { scene: [], cut: [], decade: [], depth: [], length: [] },
				};
			},
		}));
		expect(queries).toEqual([{ q: 'Swear It Again', limit: 24 }]);
		expect(rows.versions?.others.map((v) => v.video.tidal_id)).toEqual([9]);
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
