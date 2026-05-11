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

	test('leaves queue rows in the layout during this slice', () => {
		const layout = readFileSync('src/routes/+layout.svelte', 'utf8');

		expect(layout).toContain('class="queue-row"');
		expect(layout).toContain('oncontextmenu={(event) => openQueueRowMenu(item, event)}');
		expect(layout).toContain('onclick={isPending ? undefined : () => void handleQueueTrackPlay(item)}');
	});
});
