import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

const sourceRoot = join(process.cwd(), 'src');

function readSource(path: string): string {
	return readFileSync(join(sourceRoot, path), 'utf8');
}

describe('TIDAL discover artwork rendering contract', () => {
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
});
