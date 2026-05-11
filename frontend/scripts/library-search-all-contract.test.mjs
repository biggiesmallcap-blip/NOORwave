import { readFileSync } from 'node:fs';
import { describe, expect, test } from 'vitest';

describe('library all-tab search contract', () => {
	const source = () => readFileSync('src/routes/library/+page.svelte', 'utf8');

	test('all search renders mixed result previews instead of the library home', () => {
		const s = source();

		expect(s).toContain('const ALL_SEARCH_ARTIST_PREVIEW_LIMIT = 12;');
		expect(s).toContain('const ALL_SEARCH_ALBUM_PREVIEW_LIMIT = 12;');
		expect(s).toContain('const ALL_SEARCH_TRACK_PREVIEW_LIMIT = 10;');
		expect(s).toContain("{:else if activeTab === 'all' && isSearchMode}");
		expect(s).toContain('class="library-search-results"');
		expect(s).toContain('class="library-search-section"');
		expect(s).toContain("{:else if activeTab === 'all'}");
	});

	test('all search uses full counts while previews stay sliced', () => {
		const s = source();

		expect(s).toContain('let allSearchArtists = $derived(searchResults.artists);');
		expect(s).toContain('let allSearchArtistPreview = $derived(allSearchArtists.slice(0, ALL_SEARCH_ARTIST_PREVIEW_LIMIT));');
		expect(s).toContain('let allSearchAlbumPreview = $derived(visibleAlbums.slice(0, ALL_SEARCH_ALBUM_PREVIEW_LIMIT));');
		expect(s).toContain('let allSearchTrackPreview = $derived(visibleTracks.slice(0, ALL_SEARCH_TRACK_PREVIEW_LIMIT));');
		expect(s).toContain('let allSearchTotal = $derived(allSearchArtists.length + visibleAlbums.length + visibleTracks.length);');
		expect(s).toContain('formatSearchSummary(allSearchArtists.length, visibleAlbums.length, visibleTracks.length)');
	});

	test('all search hides empty sections and has one combined empty state', () => {
		const s = source();

		expect(s).toContain('{#if allSearchArtists.length > 0}');
		expect(s).toContain('{#if visibleAlbums.length > 0}');
		expect(s).toContain('{#if visibleTracks.length > 0}');
		expect(s).toContain('{#if allSearchTotal === 0}');
		expect(s).toContain("title=\"No library matches\"");
	});

	test('view-all actions switch to existing category pills', () => {
		const s = source();

		expect(s).toContain("onclick={() => switchTab('artists')}");
		expect(s).toContain("onclick={() => switchTab('albums')}");
		expect(s).toContain("onclick={() => switchTab('tracks')}");
	});

	test('artists search rendering is not gated by preloaded browse artists', () => {
		const s = source();

		expect(s).toContain('let visibleArtists = $derived.by(() => {');
		expect(s).toContain('return $searchQuery.trim() ? searchResults.artists : artists;');
		expect(s).toContain("{:else if visibleArtists.length === 0}");
		expect(s).not.toContain("{:else if artists.length === 0}\n\t\t\t<EmptyState title=\"No artists yet\"");
	});
});
