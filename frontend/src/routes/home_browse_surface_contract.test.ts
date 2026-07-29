import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, test } from 'vitest';

const here = dirname(fileURLToPath(import.meta.url));
const home = readFileSync(join(here, '+page.svelte'), 'utf8');
const search = readFileSync(join(here, 'search/+page.svelte'), 'utf8');
const mediaRail = readFileSync(join(here, '../lib/components/ui/MediaRail.svelte'), 'utf8');
const appCss = readFileSync(join(here, '../app.css'), 'utf8');

// Home is the browse surface and /search is the search tool. Before this split
// the TIDAL editorial modules rendered only in the /search empty state, so the
// app had two browse surfaces and Home was the thinner of the two.
describe('Home owns browse, /search owns searching', () => {
	test('Home renders the TIDAL editorial modules', () => {
		expect(home).toContain('DiscoverShelves');
		// Quiet on Home: a status sentence between two working shelves reads as
		// breakage, and the dedicated routes explain themselves properly.
		expect(home).toContain('quiet');
	});

	test('Home surfaces the editorial routes that nothing else links to', () => {
		expect(home).toContain('HomeEditorialPreview');
		expect(home).toContain('pagePath="new-releases"');
		expect(home).toContain('href="/new-releases"');
		expect(home).toContain('pagePath="hires"');
		expect(home).toContain('href="/hires"');
	});

	test('/search no longer renders the discover shelves', () => {
		expect(search).not.toContain('DiscoverShelves');
	});

	test('/search idle state is built from data the page already fetched', () => {
		// Both of these come out of the two calls onMount already makes, so the
		// idle view must not add network requests of its own.
		expect(search).toContain('recentListens = dedupeByTrack(listens.listens)');
		expect(search).toContain('Jump back in');
		expect(search).toContain('Your playlists');
		expect(search).toContain('localPlaylists.slice(0, 24)');
		// The query language is otherwise undiscoverable: the facet popover and
		// Tab-completion only help once you know a filter exists.
		expect(search).toContain('Try a filter');
		expect(search).toContain('applyFacetExample');
	});

	test('the playlist rail stacks two rows and plays on click', () => {
		expect(search).toContain('rows={2}');
		expect(search).toContain('onclick={() => void playLocalPlaylist(playlist)}');
		// There is no /playlists/[id] route - the old href was a dead link.
		expect(search).not.toContain('/playlists/${playlist.id}');
		// Opening the list stays reachable, per the rule that every asset
		// reference carries the shared context menu.
		expect(search).toContain('localPlaylistMenuItems(playlist)');
	});

	test('playlist covers reuse the shared mosaic cache, not a second one', () => {
		expect(search).toContain("from '$lib/stores/playlist_artwork_cache'");
		expect(search).toContain('getCachedMosaic');
		expect(search).toContain('setCachedMosaic');
		expect(search).toContain('nameToGradient');
		// Cached covers paint on mount; the observer is only for the misses,
		// and it cannot be relied on alone since a non-compositing tab never
		// delivers its callbacks.
		expect(search).toContain('seedPlaylistMosaicsFromCache()');
		// A failed fetch must release its claim or that card stays blank for
		// the rest of the session.
		expect(search).toContain('fetchedMosaicIds.delete(id)');
	});

	test('Home hands search off rather than reimplementing it', () => {
		expect(home).toContain('SearchField');
		expect(home).toContain('/search?q=');
		// /search seeds itself from the query param on mount, so the handoff
		// needs nothing on the receiving end.
		expect(search).toContain("new URLSearchParams(window.location.search).get('q')");
		// The debounce, provider fan-out and ranking stay on /search only.
		expect(home).not.toContain('searchTidal');
	});
});

describe('Rail sizing is derived, not pinned', () => {
	test('cards are sized from the rail so a whole number always fits', () => {
		// The old rails hard-coded 180px four times over, so the visible card
		// count was whatever that divided into the content width and the last
		// card was clipped at an arbitrary fraction at every window size.
		expect(mediaRail).toContain('--cols');
		expect(mediaRail).toContain('--peek');
		expect(mediaRail).toContain('@container');
		expect(mediaRail).not.toMatch(/flex:\s*0\s+0\s+180px/);
	});

	test('text cards get their own ladder', () => {
		expect(mediaRail).toContain('.media-rail.fluid.wide');
	});
});

describe('Entry motion is shared, not recopied', () => {
	test('the rise variants live in app.css', () => {
		expect(appCss).toContain('.rise-in-shelf');
		expect(appCss).toContain('.rise-in-card');
		expect(appCss).toContain('--rise-index');
		// backwards, not both: a filled opacity/transform animation holds a
		// stacking context for the life of the element, which traps a popout's
		// z-index inside its own card.
		expect(appCss).toContain('animation: rise-in-card 300ms ease-out backwards;');
		expect(appCss).toContain('prefers-reduced-motion');
	});

	test('Home stages its sections instead of animating the page once', () => {
		expect(home).toContain('rise-in-shelf');
		// A page-level animate-in fires before any shelf has data, so everything
		// that resolves later pops in behind it anyway.
		expect(home).not.toContain('page-shell home-page animate-in');
	});
});
