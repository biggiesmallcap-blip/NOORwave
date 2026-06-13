import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, test } from 'vitest';

const here = dirname(fileURLToPath(import.meta.url));
const dock = readFileSync(join(here, 'VideoDock.svelte'), 'utf8');
const store = readFileSync(join(here, '../../stores/video_session.ts'), 'utf8');
const client = readFileSync(join(here, '../../api/client.ts'), 'utf8');
const layout = readFileSync(join(here, '../../../routes/+layout.svelte'), 'utf8');

describe('persistent video dock contract', () => {
	test('renders a single persistent player mounted from the layout', () => {
		// Exactly one VideoPlayer instance, and it lives in the dock (not the
		// route) so navigation never unmounts the <video> and audio keeps going.
		expect((dock.match(/<VideoPlayer/g) ?? []).length).toBe(1);
		expect(layout).toContain('<VideoDock />');
	});

	test('docks into the route placeholder when on /videos, corner thumbnail off it', () => {
		expect(dock).toContain("page.url.pathname.startsWith('/videos')");
		expect(dock).toContain('videoStageAnchor');
		expect(dock).toContain('getBoundingClientRect()');
		expect(dock).toContain("class:mini={mode === 'mini'}");
	});

	test('frees the exclusive device when a video starts playing', () => {
		expect(dock).toContain('api.releaseExclusivePlayback()');
		expect(client).toContain("'/api/playback/exclusive/release'");
	});

	test('starting music stops the video session', () => {
		expect(dock).toContain('$isPlaying');
		expect(dock).toContain('clearVideoSession()');
	});

	test('controller owns the stream lifecycle in the store', () => {
		for (const fn of ['export async function playVideo', 'export async function advanceVideo', 'export async function refreshVideoStream', 'export function clearVideoSession']) {
			expect(store).toContain(fn);
		}
		// Stream URL persists in the store so the dock can keep playing it.
		expect(store).toContain('streamUrl: string | null;');
	});
});
