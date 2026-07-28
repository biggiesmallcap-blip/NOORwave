import { describe, expect, test } from 'vitest';

import {
	firstArtworkUrl,
	tidalArtworkFallbackSizes,
	upscaleTidalArtwork,
	usableArtwork,
} from './artwork';

describe('usableArtwork', () => {
	const placeholder =
		'https://lastfm.freetls.fastly.net/i/u/300x300/2a96cbd8b46e442fc41c2b86b821562f.png';

	test('treats the Last.fm placeholder star as absent so fallbacks still run', () => {
		// This URL is non-null and loads fine, which is exactly the problem: a
		// plain null check accepts it, the tile shows a grey star forever, and
		// the TIDAL lookup that would have found real art never fires.
		expect(usableArtwork(placeholder)).toBeNull();
		expect(usableArtwork(placeholder, 'https://img.example/real.jpg')).toBe(
			'https://img.example/real.jpg',
		);
	});

	test('skips null, undefined and blank candidates in order', () => {
		expect(usableArtwork(null, undefined, '   ', 'https://img.example/cover.jpg')).toBe(
			'https://img.example/cover.jpg',
		);
	});

	test('trims the value it returns', () => {
		expect(usableArtwork('  https://img.example/cover.jpg  ')).toBe(
			'https://img.example/cover.jpg',
		);
	});

	test('returns null when every candidate is unusable', () => {
		expect(usableArtwork(null, '', placeholder)).toBeNull();
	});
});

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

	test('rewrites unsupported cached TIDAL sizes to the requested allowed size', () => {
		expect(
			upscaleTidalArtwork('https://resources.tidal.com/images/bc8d/cf41/480x480.jpg', 320),
		).toBe('https://resources.tidal.com/images/bc8d/cf41/320x320.jpg');
	});

	test('normalizes arbitrary runtime sizes to a supported TIDAL size', () => {
		expect(
			upscaleTidalArtwork(
				'https://resources.tidal.com/images/bc8d/cf41/480x480.jpg',
				512 as never,
			),
		).toBe('https://resources.tidal.com/images/bc8d/cf41/640x640.jpg');
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

	test('rejects malformed TIDAL image paths instead of requesting an empty CDN key', () => {
		expect(upscaleTidalArtwork('https://resources.tidal.com/images//640x640.jpg', 320)).toBeNull();
		expect(upscaleTidalArtwork('https://resources.tidal.com/images/640x640.jpg', 320)).toBeNull();
	});

	test('returns null for missing input', () => {
		expect(upscaleTidalArtwork(null)).toBeNull();
		expect(upscaleTidalArtwork(undefined)).toBeNull();
	});
});

describe('tidalArtworkFallbackSizes', () => {
	test('tries smaller TIDAL artist art when the requested hero size fails', () => {
		expect(
			tidalArtworkFallbackSizes('https://resources.tidal.com/images/bc8d/cf41/640x640.jpg', 640),
		).toEqual([640, 320, 750, 1080, 1280, 160, 80]);
	});

	test('does not retry non-TIDAL URLs with duplicate source URLs', () => {
		expect(tidalArtworkFallbackSizes('https://img.example/cover.jpg', 640)).toEqual([640]);
	});

	test('returns no retry sizes for malformed TIDAL image paths', () => {
		expect(tidalArtworkFallbackSizes('https://resources.tidal.com/images//640x640.jpg', 320)).toEqual([]);
	});
});
