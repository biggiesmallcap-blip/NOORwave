// A small contract-shaped slice of a real atlas payload (three scenes, a few
// artists, one uncharted) for the Atlas unit tests.

import type { AtlasNamesResponse, AtlasResponse } from '$lib/api/video_explore_types';

export const ATLAS_SLICE: AtlasResponse = {
	built_at: '2026-10-14T01:00:00Z',
	building: false,
	size: 1000,
	scenes: [
		{ id: 0, name: 'Latin Pop & Reggaeton', exemplars: ['Shakira', 'Carlos Rivera'], tags: ['latin'], hue: 27.8, x: 854.6, y: 672.9, r: 120.5, videos: 23640, artists: 918, hours: 1615.4, channel_no: 10, lit_artists: 7, seen_videos: 14 },
		{ id: 1, name: 'Trap Now', exemplars: ['Future', 'Berner'], tags: ['trap'], hue: 199.8, x: 118.1, y: 351.1, r: 119.2, videos: 23146, artists: 1284, hours: 1581.6, channel_no: 11, lit_artists: 52, seen_videos: 104 },
		{ id: 31, name: 'K-pop', exemplars: ['NCT 127'], tags: [], hue: 260, x: 500, y: 300, r: 16, videos: 800, artists: 40, hours: 50, channel_no: null, lit_artists: 0, seen_videos: 0 },
	],
	artists: {
		ids: [101, 102, 103, 201, 202, 301, 999],
		x: [850, 860, 840, 120, 110, 500, 980],
		y: [670, 680, 660, 350, 360, 300, 500],
		scene: [0, 0, 0, 1, 1, 31, -1],
		videos: [120, 9, 30, 400, 4, 60, 2],
		light: [3, 1, 2, 3, 1, 2, 0],
	},
	recent_artist_ids: [202, 555],
};

export const NAMES_SLICE: AtlasNamesResponse = {
	ids: [101, 102, 103, 201, 202, 301, 999],
	names: ['Shakira', 'Carlos Rivera', 'Beyonc\u00e9 Tribute', 'Future', 'Berner', 'NCT 127', 'Unlinked Band'],
};
