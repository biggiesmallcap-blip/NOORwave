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

	test('docks into the watch page stage, corner player everywhere else', () => {
		// Full mode follows the published stage, not the path: only the watch
		// page publishes one, and a stage with no size (hidden under search
		// results) drops to the corner instead of shrinking the video to nothing.
		expect(dock).toContain("stageUsable ? 'full' : panelUsable ? 'panel' : 'mini'");
		expect(dock).toContain('stageRect.width > 0 && stageRect.height > 0');
		expect(dock).not.toContain("page.url.pathname === '/videos'");
		expect(dock).toContain('getBoundingClientRect()');
		expect(dock).toContain("class:mini={mode === 'mini'}");
	});

	test('leaving the stage glides the same player into place, with no snap or second pop', () => {
		// Bounds are known in every mode, so the first non-full frame is
		// already placed rather than parked at the CSS fallback corner.
		expect(dock).toContain('		if (active) {
			if (!workspace?.isConnected)');
		expect(dock).toContain("if (previousMode === 'full' && next !== 'full')");
		// Zero duration, not animation: none - removing it would replay dock-in.
		expect(dock).toContain('animation-duration: 0s;');
		expect(dock).not.toMatch(/\.morphing \{[^}]*animation: none/);
	});

	test('wheel over the docked video scrolls the page underneath', () => {
		expect(dock).toContain('onwheel={forwardWheel}');
		expect(dock).toContain('workspace.scrollBy({ top: event.deltaY * scale');
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
