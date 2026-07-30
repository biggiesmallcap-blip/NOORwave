import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, test } from 'vitest';

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, '[id]', '+page.svelte'), 'utf8');
const shelves = readFileSync(
	join(here, '..', '..', '..', 'lib', 'components', 'search', 'TidalDiscoverShelves.svelte'),
	'utf8',
);
const discoverShelves = readFileSync(
	join(here, '..', '..', '..', 'lib', 'components', 'search', 'DiscoverShelves.svelte'),
	'utf8',
);
const editorialPreview = readFileSync(
	join(here, '..', '..', '..', 'lib', 'components', 'home', 'HomeEditorialPreview.svelte'),
	'utf8',
);
const editorialPage = readFileSync(
	join(here, '..', '..', '..', 'lib', 'components', 'tidal', 'TidalEditorialPage.svelte'),
	'utf8',
);

describe('View all is only offered where this route can resolve the id', () => {
	// This route resolves an id via /api/tidal/discover-modules/{id}/items, whose
	// handler searches the home-modules cache and 404s on a miss. A module from
	// /api/tidal/page/{section} is never in that cache, so a View all on an
	// editorial shelf could only ever reach the "doesn't exist anymore" state.
	test('the button is gated on the modules having come from home-modules', () => {
		expect(shelves).toContain('homeModules?: boolean;');
		expect(shelves).toContain('homeModules = false');
		expect(shelves).toContain('(homeModules || Boolean(onViewAll))');
	});

	test('only the home-modules caller opts in', () => {
		expect(discoverShelves).toContain('homeModules');
		// Editorial surfaces fetch /api/tidal/page/... and must not claim it.
		expect(editorialPreview).not.toContain('homeModules');
		expect(editorialPage).not.toContain('homeModules');
	});

	test('no button when the carousel already holds every item', () => {
		// Without `more_path` the handler returns `module.items` unchanged, so the
		// detail page would show exactly what the rail is already showing.
		expect(shelves).toContain('return showViewAll && Boolean(mod.more_path);');
	});

	test('editorial surfaces still offer a working way to see everything', () => {
		// The section header links to the full route, which does exist.
		expect(editorialPreview).toContain('linkLabel="See all"');
		expect(editorialPreview).toContain('{href}');
	});
});

describe('TIDAL discover detail route contract', () => {
	test('guards discover shelf loads against stale route responses', () => {
		expect(source).toContain('let loadSeq = 0;');
		expect(source).toContain('loadSeq += 1;');
		expect(source).toContain("error = 'Missing discover shelf.';");
		expect(source).toContain('const seq = ++loadSeq;');
		expect(source).toContain('const res = await api.getTidalDiscoverModule(id, 50);');
		expect(source).toContain('if (seq !== loadSeq) return;');
		expect(source).toContain('if (seq === loadSeq) loading = false;');
		expect(source).toContain('void load(id);');
	});

	test('keeps shelf item actions wired through shared TIDAL helpers', () => {
		expect(source).toContain('void playTidalTrackNow(tidalHomeItemToPlayable(item));');
		expect(source).toContain('void playTidalPlaylist(item.id);');
		expect(source).toContain('openContextMenu(event, buildTidalTrackMenu(tidalHomeItemToPlayable(item)), item.title);');
		expect(source).toContain('openContextMenu(event, buildAlbumMenu({');
		expect(source).toContain('openContextMenu(event, buildArtistMenu({');
		expect(source).toContain("label: 'Play playlist'");
		expect(source).toContain('event.preventDefault();');
		expect(source).toContain('event.stopPropagation();');
		expect(source).not.toContain('$:');
	});

	test('album tiles in the View all grid open the shared detail popup', () => {
		// This grid is its own route, not the rail component, so it needs the same
		// wiring rather than inheriting it. A tile here must behave like the tile
		// on the shelf the user clicked View all from.
		expect(source).toContain('albumPopupItem = item;');
		expect(source).not.toContain('void goto(`/tidal/albums/${item.album_id}`)');
		expect(source).toContain('{#key albumPopupItem}');
		expect(source).toContain('<AlbumPopup');
		expect(source).toContain('tidalAlbumId={albumPopupItem.album_id}');
		// Passed so the artist name inside the popup is a working link; a
		// TIDAL-only album has no local artist id to derive one from.
		expect(source).toContain('artistTidalId={albumPopupItem.artist_id}');
	});
});
