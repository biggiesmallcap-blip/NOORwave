import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const srcRoot = resolve(here, '../../..');

function source(path: string): string {
	return readFileSync(join(srcRoot, path), 'utf8');
}

const remoteTransport = source('lib/components/remote/RemoteTransport.svelte');
const remoteLayout = source('routes/remote/+layout.svelte');

describe('remote artwork contracts', () => {
	test('remote now-playing art uses an allowed TIDAL size and broken-image fallback', () => {
		expect(remoteTransport).toContain('let artworkFailed = $state(false);');
		expect(remoteTransport).toContain("let artworkUrl = $derived(upscaleTidalArtwork(track?.artwork_url, 640));");
		expect(remoteTransport).toContain('{#if artworkUrl && !artworkFailed}');
		expect(remoteTransport).toContain('onerror={() => (artworkFailed = true)}');
		expect(remoteTransport).toContain('remote-art-empty');
	});

	test('remote blurred backdrop uses lockscreen-sized artwork and hides failed images', () => {
		expect(remoteLayout).toContain('let backdropArtFailed = $state(false);');
		expect(remoteLayout).toContain("let backdropArt = $derived(upscaleTidalArtwork($currentTrack?.artwork_url, 1280));");
		expect(remoteLayout).toContain('{#if backdropArt && !backdropArtFailed}');
		expect(remoteLayout).toContain('onerror={() => (backdropArtFailed = true)}');
	});
});
