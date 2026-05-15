import { describe, expect, test } from 'vitest';

import { firstArtworkUrl, upscaleTidalArtwork } from './artwork';

describe('firstArtworkUrl', () => {
	test('uses the first non-empty artwork value across ordered sources', () => {
		expect(firstArtworkUrl(null, undefined, '', 'https://img.example/cover.jpg')).toBe(
			'https://img.example/cover.jpg',
		);
	});

	test('can scan object lists for an artwork key', () => {
		const tracks = [
			{ artwork_url: null },
			{ artwork_url: '' },
			{ artwork_url: 'https://img.example/track.jpg' },
		];

		expect(firstArtworkUrl(tracks, 'fallback')).toBe('https://img.example/track.jpg');
	});

	test('prefers artist portraits before album and track art when ordered first', () => {
		const albums = [{ artwork_url: 'https://img.example/album.jpg' }];
		const tracks = [{ artwork_url: 'https://img.example/track.jpg' }];

		expect(firstArtworkUrl('https://img.example/portrait.jpg', albums, tracks)).toBe(
			'https://img.example/portrait.jpg',
		);
	});
});

describe('upscaleTidalArtwork', () => {
	test('swaps the baked-in size segment on TIDAL URLs', () => {
		expect(
			upscaleTidalArtwork('https://resources.tidal.com/images/bc8d/cf41/640x640.jpg'),
		).toBe('https://resources.tidal.com/images/bc8d/cf41/1280x1280.jpg');
	});

	test('honours a custom target size', () => {
		expect(
			upscaleTidalArtwork('https://resources.tidal.com/images/bc8d/320x320.jpg', 750),
		).toBe('https://resources.tidal.com/images/bc8d/750x750.jpg');
	});

	test('preserves a query string after the size segment', () => {
		expect(
			upscaleTidalArtwork('https://resources.tidal.com/images/bc8d/640x640.jpg?v=2'),
		).toBe('https://resources.tidal.com/images/bc8d/1280x1280.jpg?v=2');
	});

	test('leaves non-TIDAL URLs untouched', () => {
		expect(upscaleTidalArtwork('https://img.example/cover-640x640.jpg')).toBe(
			'https://img.example/cover-640x640.jpg',
		);
	});

	test('returns null for missing input', () => {
		expect(upscaleTidalArtwork(null)).toBeNull();
		expect(upscaleTidalArtwork(undefined)).toBeNull();
	});
});
