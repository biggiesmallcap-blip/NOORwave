import { describe, expect, test } from 'vitest';

import { firstArtworkUrl } from './artwork';

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
