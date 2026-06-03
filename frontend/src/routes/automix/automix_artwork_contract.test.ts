import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

const sourceRoot = join(process.cwd(), 'src');

function readSource(path: string): string {
	return readFileSync(join(sourceRoot, path), 'utf8');
}

describe('automix artwork rendering contract', () => {
	it('routes seed and forecast artwork through ArtworkImage with TIDAL-safe sizes', () => {
		const source = readSource('routes/automix/+page.svelte');

		expect(source).toContain("import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte'");
		expect(source).toContain('className="seed-art"');
		expect(source).toContain('src={$currentTrack?.artwork_url}');
		expect(source).toContain('size={640}');
		expect(source).toContain('className="queue-art"');
		expect(source).toContain('src={row.item.track.artwork_url}');
		expect(source).toContain('size={320}');
		expect(source).not.toContain('<img src={$currentTrack.artwork_url}');
		expect(source).not.toContain('<img class="queue-art" src={row.item.track.artwork_url}');
	});
});
