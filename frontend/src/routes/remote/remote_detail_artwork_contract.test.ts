import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));

function source(path: string): string {
	return readFileSync(join(here, path), 'utf8');
}

const detailPages = [
	source('albums/[id]/+page.svelte'),
	source('tidal/albums/[id]/+page.svelte'),
	source('playlists/[id]/+page.svelte'),
	source('artists/[id]/+page.svelte'),
	source('tidal/artists/[id]/+page.svelte'),
];

describe('remote detail artwork contracts', () => {
	test('renders remote detail hero artwork through ArtworkImage', () => {
		for (const page of detailPages) {
			expect(page).toContain("import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';");
			expect(page).toContain('<ArtworkImage');
			expect(page).toContain('size={640}');
			expect(page).toContain('decorative={true}');
			expect(page).not.toContain("import { upscaleTidalArtwork } from '$lib/utils/artwork';");
			expect(page).not.toContain('let coverFailed = $state(false);');
			expect(page).not.toContain('let portraitSourceIndex = $state(0);');
			expect(page).not.toContain('let backdrop = $derived(upscaleTidalArtwork(');
			expect(page).not.toMatch(/<img\s+src=\{(?:cover|portrait)\}/);
		}
	});

	test('keeps album and playlist hero artwork sources wired to real data', () => {
		expect(detailPages[0]).toContain('src={header.artwork_url}');
		expect(detailPages[1]).toContain('src={header.artwork_url}');
		expect(detailPages[2]).toContain('src={tracks[0]?.artwork_url ?? null}');
	});

	test('keeps artist hero artwork cascades wired to candidate arrays', () => {
		expect(detailPages[3]).toContain('src={portraitSources}');
		expect(detailPages[4]).toContain('src={portraitSources}');
	});
});
