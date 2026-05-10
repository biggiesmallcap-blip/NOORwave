import { readFileSync } from 'node:fs';
import { describe, expect, test } from 'vitest';

describe('artist library query contract', () => {
	test('artist detail local counts and tracks use library-track semantics', () => {
		const source = readFileSync('../noor-server/src/db/queries.rs', 'utf8');

		expect(source).toContain('artist_library_track_predicate');
		expect(source).toContain('COUNT(*) FROM tracks t');
		expect(source).toContain('COUNT(DISTINCT t.album_id)');
		expect(source).toContain('(t.is_favorite = 1 OR COALESCE(al.is_favorite, 0) = 1)');
	});
});
