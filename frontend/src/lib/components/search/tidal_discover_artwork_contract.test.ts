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
});
