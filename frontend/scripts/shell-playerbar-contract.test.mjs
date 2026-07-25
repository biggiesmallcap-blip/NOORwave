import { readFileSync } from 'node:fs';

import { describe, expect, test } from 'vitest';

describe('shell player bar extraction', () => {
	test('keeps the desktop player surface in the shell component', () => {
		const layout = readFileSync('src/routes/+layout.svelte', 'utf8');
		const playerBar = readFileSync('src/lib/shell/PlayerBar.svelte', 'utf8');

		expect(layout).toContain("import PlayerBar from '$lib/shell/PlayerBar.svelte'");
		expect(layout).toContain('<PlayerBar');
		expect(playerBar).toContain('NowPlayingMetadata');
		expect(playerBar).toContain('NowPlayingProgress');
		expect(playerBar).toContain('NowPlayingTransport');
		expect(playerBar).toContain('.np-controls');
		expect(playerBar).toContain('.np-mute-btn');
		expect(playerBar).toContain('.volume-control');
		expect(playerBar).toContain('.player-error');
		expect(layout).not.toContain('.np-controls');
		expect(layout).not.toContain('.np-mute-btn');
		expect(layout).not.toContain('.volume-control');
		expect(layout).not.toContain('.player-error');
	});

	test('keeps the artwork free of floating chips', () => {
		// Four chips over a 267px square became four overlapping chips over a
		// 64px strip the moment the queue expanded. Favorite lives in the
		// transport now, download in the overflow menu, quality in the badge
		// row; only the hover-revealed quiet-mode button stays.
		const playerBar = readFileSync('src/lib/shell/PlayerBar.svelte', 'utf8');

		expect(playerBar).not.toContain('np-art-fav');
		expect(playerBar).not.toContain('np-art-dl');
		expect(playerBar).not.toContain('np-quality');
		expect(playerBar).not.toContain('np-resolution');
		expect(playerBar).toContain('np-fullscreen-btn');
		expect(playerBar).toContain('onToggleFavorite={onToggleFavorite}');
	});

	test('moves the session controls out of the queue header and drops the legend', () => {
		const layout = readFileSync('src/routes/+layout.svelte', 'utf8');

		// Source legend now lives on the automix page.
		expect(layout).not.toContain('queue-legend');
		expect(layout).not.toContain('SOURCE_LEGEND');
		// Automix / discover-new / shortcut help sit in the sidebar pill; save,
		// clear and expand stay with the list they act on.
		const footerStart = layout.indexOf('class="sidebar-footer"');
		const footerEnd = layout.indexOf('</aside>', footerStart);
		const footer = layout.slice(footerStart, footerEnd);
		for (const cls of ['queue-automix-btn', 'queue-discover-btn', 'queue-help-btn']) {
			expect(footer, cls).toContain(cls);
		}
		expect(footer).not.toContain('queue-clear-btn');
	});

	test('source labels come from the shared humanizing map', () => {
		const layout = readFileSync('src/routes/+layout.svelte', 'utf8');

		expect(layout).toContain("from '$lib/player/queue_source'");
		// The local copies leaked raw slugs like `radio_pending` into the panel.
		expect(layout).not.toContain('function formatQueueSource');
		expect(layout).not.toContain('function queueSourceSlug');
	});

	test('measures title overflow for the now-playing marquee', () => {
		const metadata = readFileSync('src/lib/components/now-playing/NowPlayingMetadata.svelte', 'utf8');

		expect(metadata).toContain('bind:this={titleShellEl}');
		expect(metadata).toContain('bind:this={titleTextEl}');
		expect(metadata).toContain('scrollWidth - titleShellEl.clientWidth');
		expect(metadata).toContain('--np-title-marquee-distance');
		expect(metadata).toContain('class:marquee-ready={titleOverflowing}');
		expect(metadata).toContain('.np-title.marquee-ready:hover .np-title-text');
		expect(metadata).not.toContain('100% - 220px');
	});

	test('lets the volume control handle mouse wheel adjustments', () => {
		const playerBar = readFileSync('src/lib/shell/PlayerBar.svelte', 'utf8');

		expect(playerBar).toContain('const VOLUME_WHEEL_STEP = 0.05');
		expect(playerBar).toContain('function clampVolume(value: number)');
		expect(playerBar).toContain('function handleVolumeWheel(event: WheelEvent)');
		expect(playerBar).toContain('event.preventDefault()');
		expect(playerBar).toContain('event.stopPropagation()');
		expect(playerBar).toContain('const direction = event.deltaY < 0 ? 1 : -1');
		expect(playerBar).toContain('const nextVolume = clampVolume(volume + direction * VOLUME_WHEEL_STEP)');
		expect(playerBar).toContain('onwheel={handleVolumeWheel}');
	});

	test('leaves queue rows in the layout during this slice', () => {
		const layout = readFileSync('src/routes/+layout.svelte', 'utf8');

		expect(layout).toContain('class="queue-row"');
		expect(layout).toContain('oncontextmenu={(event) => openQueueRowMenu(item, event)}');
		// Every queue row (including pending) plays via play-item, which resolves
		// pending rows on the way in.
		expect(layout).toContain('onclick={() => void handleQueueTrackPlay(item)}');
	});
});
