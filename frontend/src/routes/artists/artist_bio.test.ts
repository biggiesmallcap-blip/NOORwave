import { describe, expect, test } from 'vitest';
import { cleanArtistBio } from './artist_bio';

describe('cleanArtistBio', () => {
	test('keeps visible wimpLink text and removes source markup', () => {
		expect(
			cleanArtistBio(
				'First line<br/>Second [wimpLink artistId="1"]Artist Name[/wimpLink] &amp; friends<br />Third line',
			),
		).toBe('First line\nSecond Artist Name & friends\nThird line');
	});

	test('returns null for markup-only biography text', () => {
		expect(cleanArtistBio('<br/><br />')).toBeNull();
	});
});
