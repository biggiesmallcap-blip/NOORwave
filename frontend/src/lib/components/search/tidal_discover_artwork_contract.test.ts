import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

const sourceRoot = join(process.cwd(), 'src');

function readSource(path: string): string {
	return readFileSync(join(sourceRoot, path), 'utf8');
}

describe('TIDAL discover artwork rendering contract', () => {
	it('guards home discover shelf loads against stale responses', () => {
		const source = readSource('lib/components/search/DiscoverShelves.svelte');

		expect(source).toContain('let loadSeq = 0;');
		expect(source).toContain('return () => { loadSeq += 1; };');
		expect(source).toContain('const seq = ++loadSeq;');
		expect(source).toContain('if (seq !== loadSeq) return;');
		expect(source).toContain('const nextModules = data.modules ?? [];');
		expect(source).toContain('putCachedHomeModules(nextModules)');
	});

	it('routes home shelf artwork through ArtworkImage with a fallback', () => {
		const source = readSource('lib/components/search/TidalDiscoverShelves.svelte');

		expect(source).toContain("import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte'");
		expect(source).toContain('<ArtworkImage');
		expect(source).toContain('src={item.artwork_url}');
		expect(source).toContain('fallbackText={fallbackGlyph(item.kind)}');
		expect(source).not.toContain("style=\"background-image: url('{item.artwork_url}')\"");
	});

	it('routes discover detail artwork through ArtworkImage with a fallback', () => {
		const source = readSource('routes/search/discover/[id]/+page.svelte');

		expect(source).toContain("import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte'");
		expect(source).toContain('<ArtworkImage');
		expect(source).toContain('src={item.artwork_url}');
		expect(source).toContain('fallbackText={fallbackGlyph(item.kind)}');
		expect(source).not.toContain("style=\"background-image: url('{item.artwork_url}')\"");
	});

	it('keeps home shelf context menus app-owned', () => {
		const source = readSource('lib/components/search/TidalDiscoverShelves.svelte');

		expect(source).toContain('function handleItemContextMenu(event: MouseEvent, item: TidalHomeItem)');
		expect(source).toContain('function openArtistContextMenu(event: MouseEvent, item: TidalHomeItem)');
		expect(source).toContain('function openAlbumContextMenu(event: MouseEvent, item: TidalHomeItem)');
		expect(source).toContain('event.preventDefault();');
		expect(source).toContain('event.stopPropagation();');
		expect(source).toContain('openContextMenu(event, buildTidalTrackMenu(tidalHomeItemToPlayable(item)), item.title);');
		expect(source).toContain('openContextMenu(event, buildArtistMenu({');
		expect(source).toContain('openContextMenu(event, buildAlbumMenu({');
	});

	it('opens every album tile in the shared detail popup, on every surface', () => {
		const source = readSource('lib/components/search/TidalDiscoverShelves.svelte');
		const popup = readSource('lib/components/album/AlbumPopup.svelte');
		const detailPopup = readSource('lib/components/AlbumDetailPopup.svelte');

		// One component renders the cards for the discover rails, the editorial
		// "View all" pages, /moods and the discover detail route, so wiring the
		// popup here covers all of them. It replaces a navigation to
		// /tidal/albums/{id}, which is a lot of ceremony for a browse surface.
		expect(source).toContain('albumPopupItem = item;');
		expect(source).not.toContain('void goto(`/tidal/albums/${item.album_id}`)');
		expect(source).toContain('{#key albumPopupItem}');
		expect(source).toContain('<AlbumPopup');
		expect(source).toContain('tidalAlbumId={albumPopupItem.album_id}');
		expect(source).toContain('artistTidalId={albumPopupItem.artist_id}');

		// The same popup Library and the recommendation rails open, loading through
		// the same module rather than a per-surface copy.
		expect(popup).toContain("from '$lib/album/album_detail'");
		expect(popup).toContain('loadAlbumDetail(');

		// The artist name inside the popup is a link. A TIDAL-only album has no
		// local artist id, so the href has to be passed in.
		expect(detailPopup).toContain('artistHref');
		expect(detailPopup).toContain('popup-artist-link');
		expect(detailPopup).toContain('function openArtistPage()');
		// Close first, then navigate: a popup left mounted over the page it opened
		// swallows the next click through its window-level outside-click handler.
		expect(detailPopup).toContain('requestClose();');
		expect(detailPopup).toContain('void goto(href);');
		expect(popup).toContain('/tidal/artists/${artistTidalId}');
	});
});
