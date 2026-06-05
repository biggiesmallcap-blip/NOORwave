import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, 'TrackRow.svelte'), 'utf8');
const tidalSource = readFileSync(join(here, 'TidalTrackRow.svelte'), 'utf8');
const trackRowSources = [
	['TrackRow', source],
	['TidalTrackRow', tidalSource]
] as const;

describe('TrackRow world play count contract', () => {
	test('keeps world play counts opt-in for scoped pages', () => {
		expect(source).toContain('worldPlayCount');
		expect(source).toContain('formatCompactCount');
		expect(source).toContain('{#if worldPlayCount != null}');
		expect(source).toContain('play-count-local-label');
		expect(source).toContain('class="play-count-world"');
		expect(source).toContain('showPlayCount = false');
	});
});

describe('shared track row artwork contract', () => {
	test.each(trackRowSources)('%s routes artwork through ArtworkImage', (_name, rowSource) => {
		expect(rowSource).toContain("import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';");
		expect(rowSource).toContain('<ArtworkImage');
		expect(rowSource).toContain('src={track.artwork_url}');
		expect(rowSource).toContain('size={320}');
		expect(rowSource).toContain('fallbackText={track.title.slice(0, 2).toUpperCase()}');
	});

	test.each(trackRowSources)('%s does not render artwork URLs with raw img tags', (_name, rowSource) => {
		expect(rowSource).not.toMatch(/<img[\s\S]*(artwork_url|photo_url|picture_url|image_url|cover_url|thumbnail_url)/);
	});

	test.each(trackRowSources)('%s keeps separators and section comments ASCII-safe', (_name, rowSource) => {
		expect(rowSource).not.toMatch(/\u2014/);
		expect(rowSource).not.toMatch(/[\u2500-\u257F]/);
	});

	test.each(trackRowSources)('%s suppresses nested artist and album browser menus', (_name, rowSource) => {
		expect(rowSource).toContain('function openArtistContextMenu(e: MouseEvent)');
		expect(rowSource).toContain('function openAlbumContextMenu(e: MouseEvent)');
		expect(rowSource).toContain('e.preventDefault();');
		expect(rowSource).toContain('e.stopPropagation();');
		expect(rowSource).toContain('oncontextmenu={openArtistContextMenu}');
		expect(rowSource).toContain('oncontextmenu={openAlbumContextMenu}');
	});
});
