import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, 'VideoCard.svelte'), 'utf8');

describe('VideoCard artwork contract', () => {
	test('normalizes TIDAL-capable posters through ArtworkImage', () => {
		expect(source).toContain("import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';");
		expect(source).toContain('<ArtworkImage');
		expect(source).toContain('className="poster"');
		expect(source).toContain('src={poster}');
		expect(source).toContain('size={320}');
		expect(source).toContain('fallbackText="VID"');
		expect(source).toContain('decorative={true}');
		expect(source).not.toContain('<img class="poster" src={poster}');
		expect(source).not.toContain('{#if poster}');
		expect(source).not.toContain('placeholder');
	});
});
