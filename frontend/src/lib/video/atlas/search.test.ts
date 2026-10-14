import { describe, expect, test } from 'vitest';
import type { AtlasScene } from '$lib/api/video_explore_types';
import { searchScenes } from './search';

function scene(id: number, name: string, tags: string[], channel_no: number | null, videos = 100): AtlasScene {
	return {
		id,
		name,
		exemplars: [],
		tags,
		hue: 0,
		x: 0,
		y: 0,
		r: 10,
		videos,
		artists: 10,
		hours: 5,
		channel_no,
		lit_artists: 0,
		seen_videos: 0,
	};
}

const scenes = [
	scene(1, 'Pop Rock Legends', ['rock', 'pop', 'classic rock'], 58, 1098),
	scene(2, 'Dance Hits', ['house', 'electronic', 'pop'], 12, 900),
	scene(3, 'Deep House', ['house', 'deep house'], null, 300),
	scene(4, 'Trap Now', ['trap', 'hip hop'], 3, 2500),
];

describe('finding scenes on the Atlas', () => {
	test('by channel number, with or without "ch"', () => {
		expect(searchScenes(scenes, '58')[0].name).toBe('Pop Rock Legends');
		expect(searchScenes(scenes, 'ch 58')[0].name).toBe('Pop Rock Legends');
		expect(searchScenes(scenes, 'Channel 12')[0].name).toBe('Dance Hits');
	});

	test('by name, closest first', () => {
		const hits = searchScenes(scenes, 'deep');
		expect(hits[0].name).toBe('Deep House');
		expect(hits[0].tag).toBeNull();
	});

	test('by genre, saying which genre matched', () => {
		const hits = searchScenes(scenes, 'house');
		expect(hits.map((hit) => hit.name)).toEqual(['Dance Hits', 'Deep House']);
		expect(hits[0].tag).toBe('house');
		expect(hits[0].channelNo).toBe(12);
	});

	test('nothing for an empty query', () => {
		expect(searchScenes(scenes, '  ')).toEqual([]);
	});
});
