import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));

function source(path: string): string {
	return readFileSync(join(here, path), 'utf8');
}

const mixes = source('YourMixesShelf.svelte');
const radio = source('PersonalRadioShelf.svelte');

describe('TIDAL home artwork contracts', () => {
	test('routes mix artwork through ArtworkImage fallback handling', () => {
		expect(mixes).toContain("import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';");
		expect(mixes).toContain('<ArtworkImage');
		expect(mixes).toContain('className="art"');
		expect(mixes).toContain('src={mix.image_url}');
		expect(mixes).toContain('size={320}');
		expect(mixes).toContain('fallbackText="MIX"');
		expect(mixes).toContain('decorative={true}');
		expect(mixes).toContain(':global(.art)');
		expect(mixes).not.toContain("style=\"background-image: url('{mix.image_url}')\"");
	});

	test('routes radio artwork through ArtworkImage fallback handling', () => {
		expect(radio).toContain("import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';");
		expect(radio).toContain('<ArtworkImage');
		expect(radio).toContain('className="art"');
		expect(radio).toContain('src={station.image_url}');
		expect(radio).toContain('size={320}');
		expect(radio).toContain('fallbackText="RAD"');
		expect(radio).toContain('decorative={true}');
		expect(radio).toContain(':global(.art)');
		expect(radio).not.toContain("style=\"background-image: url('{station.image_url}')\"");
	});
});
