import { describe, expect, it } from 'vitest';
import type { ProviderRecommendationItem } from '$lib/api/client';
import { recommendationItemKey } from './recommendation_navigation';

const SHELF = 'lastfm:album:Last.fm recommended albums';

// The two spellings Last.fm actually returned: a combining diaeresis (NFD) and
// the precomposed letter (NFC). Written as escapes so this file stays ASCII.
const DECOMPOSED = 'Du\u0308nyala';
const PRECOMPOSED = 'D\u00fcnyala';

function album(title: string, tidalAlbumId: number | null): ProviderRecommendationItem {
	return {
		entity_type: 'album',
		provider: 'lastfm',
		title,
		artist_name: 'Aykut Bilir',
		artwork_url: null,
		local_track_id: null,
		mbid: null,
		playable: true,
		reason: '',
		score: 0.5,
		tidal_id: 0,
		tidal_album_id: tidalAlbumId,
	} as unknown as ProviderRecommendationItem;
}

describe('recommendationItemKey', () => {
	it('separates two items that resolved to the same album', () => {
		// The real payload that took Home down: Last.fm returns the same record
		// twice because the titles differ only by Unicode normalisation, so the
		// server's title dedupe keeps both, and both then resolve to TIDAL album
		// 167919206.
		expect(recommendationItemKey(SHELF, album(DECOMPOSED, 167919206), 3)).not.toBe(
			recommendationItemKey(SHELF, album(PRECOMPOSED, 167919206), 4),
		);
	});

	it('keeps every key in a shelf window unique', () => {
		const items = [
			album(DECOMPOSED, 167919206),
			album(PRECOMPOSED, 167919206),
			album('Owned', null),
			album('Owned', null),
		];
		const keys = items.map((item, index) => recommendationItemKey(SHELF, item, index));
		expect(new Set(keys).size).toBe(items.length);
	});

	it('is stable for the same item at the same position', () => {
		const item = album(PRECOMPOSED, 167919206);
		expect(recommendationItemKey(SHELF, item, 2)).toBe(recommendationItemKey(SHELF, item, 2));
	});

	it('carries the resolved identity so a key still says what it points at', () => {
		expect(recommendationItemKey(SHELF, album(PRECOMPOSED, 167919206), 2)).toContain(
			'album:tidal:167919206',
		);
	});
});
