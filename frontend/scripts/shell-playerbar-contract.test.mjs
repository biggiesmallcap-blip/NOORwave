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

	test('keeps artwork overlays limited to quiet mode and the bottom favorite', () => {
		const playerBar = readFileSync('src/lib/shell/PlayerBar.svelte', 'utf8');
		const artwork = playerBar.slice(
			playerBar.indexOf('<div class="np-artwork-wrap">'),
			playerBar.indexOf('<NowPlayingMetadata')
		);

		expect(artwork).not.toContain('np-art-dl');
		expect(artwork).not.toContain('np-quality');
		expect(artwork).not.toContain('np-resolution');
		expect(artwork).toContain('np-artwork-quiet-cue');
		expect(artwork).toContain("layout === 'bottom'");
		expect(artwork).toContain('np-artwork-favorite');
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

	test('truncates long titles with a full-title tooltip and bounded link', () => {
		const metadata = readFileSync('src/lib/components/now-playing/NowPlayingMetadata.svelte', 'utf8');

		expect(metadata).toContain('title={track.title}');
		expect(metadata).toContain('width: fit-content;');
		expect(metadata).toContain('text-overflow: ellipsis;');
		expect(metadata).not.toContain('marquee-ready');
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

	test('phone layout drops the desktop compositor transform so fixed chrome tracks the screen', () => {
		const layout = readFileSync('src/routes/+layout.svelte', 'utf8');
		const phone = layout.slice(layout.indexOf('@media (max-width: 679px) {'));

		expect(layout).toContain('transform: translateZ(0);');
		expect(phone.slice(0, phone.indexOf('.workspace {'))).toContain('transform: none;');
	});

	test('renders desktop and mobile queue rows through the shared shell row', () => {
		const layout = readFileSync('src/routes/+layout.svelte', 'utf8');
		const row = readFileSync('src/lib/shell/QueueRow.svelte', 'utf8');

		expect(layout).toContain("import QueueRow from '$lib/shell/QueueRow.svelte'");
		expect(layout.match(/\{@render queueRow\(item, (true|false)\)\}/g)?.length).toBe(2);
		expect(row).toContain('class="queue-row"');
		expect(layout).not.toContain('.queue-row {');
		expect(layout).toContain('onmenu={(event) => openQueueRowMenu(item, event)}');
		// Every queue row (including pending) plays via play-item, which resolves
		// pending rows on the way in.
		expect(layout).toContain('onplay={() => void handleQueueTrackPlay(item)}');
		expect(row).toContain('onclick={onplay}');
	});
});
