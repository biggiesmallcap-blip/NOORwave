import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));

function source(path: string): string {
	return readFileSync(join(here, path), 'utf8');
}

const sidePanel = source('DiscoverSidePanel.svelte');
const adapter = source('discover_space_adapter.ts');

describe('DiscoverSpace artwork contracts', () => {
	test('normalizes API node artwork before canvas and panel rendering', () => {
		expect(adapter).toContain("import { upscaleTidalArtwork } from '$lib/utils/artwork';");
		expect(adapter).toContain('const artworkUrl = upscaleTidalArtwork(api.artwork_url, 320) ?? undefined;');
		expect(adapter).toContain('artworkUrl,');
		expect(adapter).not.toContain('artworkUrl: api.artwork_url');
	});

	test('routes side panel artwork through ArtworkImage fallback handling', () => {
		expect(sidePanel).toContain("import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';");
		expect(sidePanel).toContain('<ArtworkImage');
		expect(sidePanel).toContain('className="artwork"');
		expect(sidePanel).toContain('src={node.artworkUrl}');
		expect(sidePanel).toContain('alt={`${node.title} artwork`}');
		expect(sidePanel).toContain('className="idle-artwork"');
		expect(sidePanel).toContain('src={seedNode.artworkUrl}');
		expect(sidePanel).toContain('size={320}');
		expect(sidePanel).toContain('fallbackText={fallbackText(');
		expect(sidePanel).toContain('decorative={true}');
		expect(sidePanel).toContain(':global(.artwork)');
		expect(sidePanel).toContain(':global(.idle-artwork)');
		expect(sidePanel).not.toContain('<img class="artwork"');
		expect(sidePanel).not.toContain('<img class="idle-artwork"');
	});

	test('keeps pending TIDAL resolutions bound to the selected node snapshot', () => {
		expect(sidePanel).toContain('function updateNodePlayable(trackId: number, playable: PlayableTrack)');
		expect(sidePanel).toContain('if (node?.trackId === trackId)');
		expect(sidePanel).toContain('artwork_url: hit.artwork_url ?? targetNode.artworkUrl ?? null');
		expect(sidePanel).toContain('updateNodePlayable(targetNode.trackId, resolved)');
		expect(sidePanel).toContain('const targetNode = node;');
		expect(sidePanel).toContain('const playable = await resolveExternalPlayable(targetNode, basePlayable);');
		expect(sidePanel).toContain("showToast(`Couldn't find \"${targetNode.title}\" on Tidal`, 'error');");
		expect(sidePanel).not.toContain('artwork_url: hit.artwork_url ?? node?.artworkUrl ?? null');
	});
});
