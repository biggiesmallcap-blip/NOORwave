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

	test('leaves queue rows in the layout during this slice', () => {
		const layout = readFileSync('src/routes/+layout.svelte', 'utf8');

		expect(layout).toContain('class="queue-row"');
		expect(layout).toContain('oncontextmenu={(event) => openQueueRowMenu(item, event)}');
		expect(layout).toContain('onclick={isPending ? undefined : () => void handleQueueTrackPlay(item)}');
	});
});
