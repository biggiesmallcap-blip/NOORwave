import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

const routeSource = readFileSync(join(process.cwd(), 'src/routes/library/+page.svelte'), 'utf8');

describe('library detail artwork rendering contract', () => {
	it('routes track detail modal artwork through ArtworkImage with fallbacks', () => {
		expect(routeSource).toContain("import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte'");
		expect(routeSource).toContain('className="detail-track-art-large"');
		expect(routeSource).toContain('src={detailTrack.artwork_url}');
		expect(routeSource).toContain('size={640}');
		expect(routeSource).toContain('className="detail-track-art"');
		expect(routeSource).toContain('src={track.artwork_url}');
		expect(routeSource).toContain('size={320}');
		expect(routeSource).not.toContain('<img class="detail-track-art-large" src={detailTrack.artwork_url}');
		expect(routeSource).not.toContain('<img class="detail-track-art" src={track.artwork_url}');
	});
});
