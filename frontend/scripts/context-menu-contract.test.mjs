import { readFileSync } from 'node:fs';
import { describe, expect, test } from 'vitest';

describe('context menu coverage contract', () => {
	test('discover detail uses shared track and album menu builders', () => {
		const source = readFileSync('src/routes/search/discover/[id]/+page.svelte', 'utf8');

		expect(source).toContain('buildTidalTrackMenu');
		expect(source).toContain('buildAlbumMenu');
		expect(source).toContain('buildArtistMenu');
		expect(source).toContain('oncontextmenu');
	});

	test('discover shelves use shared menus for cards and secondary references', () => {
		// The shelf renderer was extracted into TidalDiscoverShelves so /charts
		// and /moods can reuse the exact same row/card markup + menus. The old
		// DiscoverShelves wrapper now just handles fetch/cache and delegates
		// rendering: the menu contract lives in the renderer.
		const source = readFileSync(
			'src/lib/components/search/TidalDiscoverShelves.svelte',
			'utf8',
		);

		expect(source).toContain('buildTidalTrackMenu');
		expect(source).toContain('buildAlbumMenu');
		expect(source).toContain('buildArtistMenu');
		expect(source).toContain('handleItemContextMenu');
		expect(source).toContain('openArtistContextMenu');
		expect(source).toContain('function handleItemKeydown');
		expect(source).toContain('function openArtist');
		expect(source).toContain('class="sub sub-link"');
	});

	test('automix track references use the shared track menu builder', () => {
		const source = readFileSync('src/routes/automix/+page.svelte', 'utf8');

		expect(source).toContain('buildTrackMenu');
		expect(source).toContain('oncontextmenu');
	});

	test('local and tidal track rows expose secondary artist and album menus', () => {
		const localRow = readFileSync('src/lib/components/TrackRow.svelte', 'utf8');
		const tidalRow = readFileSync('src/lib/components/TidalTrackRow.svelte', 'utf8');

		for (const source of [localRow, tidalRow]) {
			expect(source).toContain('buildArtistMenu');
			expect(source).toContain('buildAlbumMenu');
			expect(source).toContain('openArtistContextMenu');
			expect(source).toContain('openAlbumContextMenu');
		}
	});

	test('tidal-only album and artist page rows use tidal track menu builder', () => {
		const albumPage = readFileSync('src/routes/albums/[id]/+page.svelte', 'utf8');
		const artistPage = readFileSync('src/routes/artists/ArtistDetail.svelte', 'utf8');

		for (const source of [albumPage, artistPage]) {
			expect(source).toContain('buildTidalTrackMenu');
			expect(source).toContain('oncontextmenu');
		}
	});

	test('search track rows expose secondary artist and album menus', () => {
		const source = readFileSync('src/routes/search/+page.svelte', 'utf8');

		expect(source).toContain('openTidalArtistContextMenu');
		expect(source).toContain('openTidalAlbumContextMenu');
		expect(source).toContain('oncontextmenu={(e) => openTidalArtistContextMenu');
		expect(source).toContain('oncontextmenu={(e) => openTidalAlbumContextMenu');
	});

	test('duplicates members use shared track context menus', () => {
		const source = readFileSync('src/routes/duplicates/+page.svelte', 'utf8');

		expect(source).toContain('buildTrackMenu');
		expect(source).toContain('openDuplicateTrackContextMenu');
		expect(source).toContain('oncontextmenu={(event) => openDuplicateTrackContextMenu');
	});

	test('queue artist links and mobile now playing expose context menus', () => {
		const source = readFileSync('src/routes/+layout.svelte', 'utf8');

		expect(source).toContain('buildArtistMenu');
		expect(source).toContain('openQueueArtistContextMenu');
		expect(source).toContain('oncontextmenu={(event) => openQueueArtistContextMenu');
		expect(source).toContain('oncontextmenu={openNowPlayingContextMenu}');
	});

	test('album popup exposes album, artist, and track menus', () => {
		const source = readFileSync('src/lib/components/AlbumDetailPopup.svelte', 'utf8');

		expect(source).toContain('buildAlbumMenu');
		expect(source).toContain('buildArtistMenu');
		expect(source).toContain('openAlbumContextMenu');
		expect(source).toContain('openTrackArtistContextMenu');
	});

	test('chart murals expose shared track menus', () => {
		const trending = readFileSync('src/lib/components/charts/TrendingShelf.svelte', 'utf8');
		const daily = readFileSync('src/lib/components/charts/DailyChartShelf.svelte', 'utf8');
		const mural = readFileSync('src/lib/components/charts/ChartMural.svelte', 'utf8');

		expect(trending).toContain('buildTrackMenu');
		expect(trending).toContain('buildTidalTrackMenu');
		expect(trending).toContain('handleEntryContext');
		expect(trending).toContain('onCardContext');
		expect(trending).toContain('onItemContext');
		expect(daily).toContain('buildTidalTrackMenu');
		expect(daily).toContain('openEntryContext');
		expect(daily).toContain('openMatrixCellContext');
		expect(mural).toContain('onCardContext');
		expect(mural).toContain('onItemContext');
		expect(mural).toContain('oncontextmenu');
	});

	test('genre interior artist chips use shared artist menus', () => {
		const source = readFileSync('src/lib/components/Genre/GenreInterior.svelte', 'utf8');

		expect(source).toContain('buildArtistMenu');
		expect(source).toContain('handleArtistContextMenu');
		expect(source).toContain('oncontextmenu={(event) => handleArtistContextMenu');
	});

	test('artist page similar artists and fallback album rails use shared menus', () => {
		const source = readFileSync('src/routes/artists/ArtistDetail.svelte', 'utf8');

		expect(source).toContain('buildArtistMenu');
		expect(source).toContain('similarArtistMenu');
		expect(source).toContain('fallbackAlbumMenu');
		expect(source).toContain('openContextMenu(e, similarArtistMenu(similar), similar.name)');
		expect(source).toContain('openContextMenu(e, fallbackAlbumMenu(album), album.title)');
	});

	test('library hero exposes artist navigation and shared artist menu hook', () => {
		const hero = readFileSync('src/lib/components/LibraryHero.svelte', 'utf8');
		const library = readFileSync('src/routes/library/+page.svelte', 'utf8');

		expect(hero).toContain('onArtistClick');
		expect(hero).toContain('onContextMenu');
		expect(hero).toContain('openHeroContextMenu');
		expect(hero).toContain('onclick={() => onArtistClick?.(current.id)}');
		expect(library).toContain('onArtistClick={handleHomeArtistClick}');
		expect(library).toContain('onContextMenu={handleHomeArtistContextMenu}');
	});

	test('album carousel exposes secondary artist navigation and menu hook', () => {
		const carousel = readFileSync('src/lib/components/AlbumCarousel.svelte', 'utf8');
		const library = readFileSync('src/routes/library/+page.svelte', 'utf8');

		expect(carousel).toContain('artist_id: number | null');
		expect(carousel).toContain('onArtistClick');
		expect(carousel).toContain('onArtistContextMenu');
		expect(carousel).toContain('openArtistContextMenu');
		expect(carousel).toContain('class="album-artist album-artist-link"');
		expect(library).toContain('artist_id: track.artist_id ?? null');
		expect(library).toContain('onArtistClick={handleHomeArtistClick}');
		expect(library).toContain('onArtistContextMenu={handleHomeArtistContextMenu}');
	});
});
