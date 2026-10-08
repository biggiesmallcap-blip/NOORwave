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
		expect(dock).toContain("class:mini={place === 'mini'}");
	});

	test('leaving the stage glides the same player into place, with no snap or second pop', () => {
		// Bounds are known in every mode, so the first non-full frame is
		// already placed rather than parked at the CSS fallback corner.
		expect(dock).toMatch(/if \(active\) \{\s*if \(!workspace\?\.isConnected\)/);
		expect(dock).toContain('const moving = previousPlace !== null && previousPlace !== next;');
		// Page moves are a compositor transform (FLIP), not a per-frame
		// relayout of the playing video.
		expect(dock).toContain('glide = el.animate(');
		// Measured with the arrival animation finished and snap transitions off,
		// and the start pinned inline so no frame shows the end box early.
		expect(dock).toContain("dockEl.style.animationDuration = '0s';");
		expect(dock).toContain('el.style.transform = start;');
		expect(dock).toContain('animation: dock-in 0.22s ease backwards;');
		expect(dock).toContain('if (dockEl?.isConnected) lastDockRect = dockEl.getBoundingClientRect();');
		expect(dock).toContain('const started = fullscreenMove ? sizeGlideFrom(lastDockRect) : flipFrom(lastDockRect);');
		// The FLIP starts when the compositor runs it (a pinned start time
		// skipped part of the path after a busy page change), and the dock
		// lands when the glide really finishes.
		expect(dock).not.toMatch(/function flipFrom[\s\S]*?glide\.startTime[\s\S]*?\n\t\}/);
		expect(dock).toContain('started?.finished.then(');
		// Moving the host restarts CSS animations; they are finished on the spot.
		expect(dock).toContain('if (animation !== glide) animation.finish();');
		// Zero duration, not animation: none - removing it would replay dock-in.
		expect(dock).toContain('animation-duration: 0s;');
		expect(dock).not.toMatch(/\.morphing \{[^}]*\n\s*animation: none;/);
	});

	test('fullscreen grows the same player to fill the window instead of a hard cut', () => {
		const player = readFileSync(join(here, 'VideoPlayer.svelte'), 'utf8');
		expect(dock).toContain('onFullscreenToggle={toggleExpanded}');
		expect(dock).toContain('document.documentElement.requestFullscreen?.()');
		expect(dock).toContain("if (event.key === 'Escape' && expanded && !document.fullscreenElement) collapse();");
		// One motion at a time, window first both ways: going in, the glide
		// waits for the window to finish growing (gliding first showed the
		// native title bar over the filled window); going out, window first,
		// then glide.
		expect(dock).toContain('if (expanded) return null;');
		expect(dock).toContain('fullscreenMove ? sizeGlideFrom(lastDockRect)');
		expect(dock).not.toMatch(/expanded = true;\s*fullscreenTimer = setTimeout/);
		expect(dock).toMatch(/enteringFullscreen = false;\s*afterWindowSettles\(\(\) => \{[\s\S]*?if \(active\) expanded = true;/);
		expect(dock).toContain('requestAnimationFrame(() => requestAnimationFrame(() => (expanded = false)));');
		expect(player).toMatch(/if \(onFullscreenToggle\) \{\s*onFullscreenToggle\(\);\s*return;/);
	});

	test('in the desktop app the window switch is native, window first then the glide', () => {
		// Tauri's own fullscreen showed the desktop and the old title bar
		// through the window while it resized; noor-app switches in one step.
		expect(dock).toContain("import { hasNativeVideoFullscreen, setNativeVideoFullscreen } from '$lib/tauri/video_fullscreen';");
		expect(dock).toMatch(/if \(nativeSwitch\) \{\s*if \(expanded\) collapse\(\);\s*else enterNativeFullscreen\(\);\s*return;/);
		// In: the window switches first (the command returns once it has),
		// then the dock grows into it, same order as the browser path.
		expect(dock).toContain('requestAnimationFrame(() => requestAnimationFrame(() => (expanded = nativeOn && active)));');
		expect(dock).not.toMatch(/function endGlide\(\) \{\s*morphing = false;\s*if \(place === 'expanded'\)/);
		// Out: the window comes back first, then the dock glides home.
		expect(dock).toMatch(/void leaveNativeFullscreen\(\)\.then\(\(\) =>\s*requestAnimationFrame\(\(\) => requestAnimationFrame\(\(\) => \(expanded = false\)\)\)/);
		// Ending the session or tearing down never strands the window fullscreen.
		expect(dock).toMatch(/if \(active\) return;\s*void leaveNativeFullscreen\(\);\s*expanded = false;/);
		expect(dock).toMatch(/glide\?\.cancel\(\);\s*void leaveNativeFullscreen\(\);/);
	});

	test('on the watch page the player lives in the stage and scrolls natively', () => {
		// A fixed layer chasing the stage rect trailed compositor scrolling by
		// a frame, so the video slid out of its frame while scrolling.
		expect(dock).toContain('<div class="video-dock-host" bind:this={host}>');
		expect(dock).toContain('stage.appendChild(host);');
		expect(dock).toContain('const unsubscribeStage = videoStageAnchor.subscribe((stage) => {');
		expect(dock).toContain(':global(.stage-anchor) > .video-dock-host > .video-dock {');
		expect(dock).not.toContain('onwheel=');
		// Gliding onto the stage stays a fixed layer until it lands: inside the
		// stage the glide was clipped by its overflow and faded with the page.
		expect(dock).toContain("else if ((!moving || reducedMotion) && $videoStageAnchor) moveIntoStage($videoStageAnchor);");
		expect(dock).toContain("if (place === 'full' && stage?.isConnected) moveIntoStage(stage);");
		// The move in lands as the glide ends, so stage-in must not replay there.
		expect(dock).toMatch(/> \.video-dock-host > \.video-dock \{[^}]*animation: none !important;/);
	});

	test('window <-> pill only covers and uncovers a video that keeps its size', () => {
		// Resizing the playing video with the box re-laid it out every frame
		// and it lagged the box edges.
		expect(dock).toContain('let windowSize = $derived(miniSize(viewportWidth, false));');
		expect(dock).toContain('width: calc(var(--window-w) - 2px);');
		expect(dock).toContain(".video-dock.mini.placed[data-corner^='b'] .player-surface { bottom: 0; }");
		expect(dock).toContain(".video-dock.mini.placed[data-corner$='r'] .player-surface { right: 0; }");
		expect(dock).toContain('onclick={() => setCollapsed(true)}');
		expect(dock).toContain('.video-dock:is(.morphing, .unfolding) .mini-chrome');
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
