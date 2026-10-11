import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

// Wiring guard only: request-ordering behavior is proven in
// $lib/stores/library.test.ts and library_search.test.ts. This keeps the
// route from drifting back to applying async results itself.
const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, '+page.svelte'), 'utf8').replace(/\r\n/g, '\n');

describe('library route request ownership wiring', () => {
	test('search state comes from the controller, never route-local writes', () => {
		expect(source).toContain('const librarySearch = createLibrarySearch({');
		expect(source).toContain('librarySearch.setQuery($searchQuery);');
		expect(source).toContain('let searchResults = $derived($librarySearch.results);');
		expect(source).not.toMatch(/\bsearchResults\s*=\s*[{[]/);
		expect(source).not.toContain('async function runLibrarySearch');
	});

	test('leaving the route invalidates search and track-list work', () => {
		expect(source).toContain('librarySearch.dispose();');
		expect(source).toContain('cancelTrackListRequests();');
	});

	test('failures render ErrorState with a retry distinct from empty states', () => {
		expect(source).toContain('<ErrorState title="Search failed" error={$librarySearch.error} onretry={() => void librarySearch.retry()} />');
		expect(source).toContain('onretry={() => void retryTrackList()}');
		expect(source).toContain('$trackListError && !$trackListError.append');
	});

	test('a return visit reloads rows from a failed or abandoned refresh', () => {
		expect(source).toContain('if (get(tracks).length === 0 || trackListNeedsReload()) void loadTracks();');
	});

	test('Songs reloads when its rows belong to another sort or scope', () => {
		expect(source).toContain('if (trackListRequestMatches($sortBy, $sortDir, likedOnly)) return;');
	});
});
