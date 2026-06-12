import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const layoutSource = readFileSync(resolve(__dirname, '../../routes/+layout.svelte'), 'utf8');
const menuSource = readFileSync(resolve(__dirname, './track_menu.ts'), 'utf8');

describe('queue row actions contract', () => {
	test('per-row actions collapse into a single overflow menu, not inline pills', () => {
		// The noisy hover pills (favourite / play-next / remove) were replaced by
		// one overflow button that opens the shared context menu.
		expect(layoutSource).toContain('class="queue-overflow"');
		expect(layoutSource).toContain('openQueueRowMenuFromButton(item, event)');
		// The old inline-pill machinery is gone.
		expect(layoutSource).not.toContain('canFavoriteQueueRow');
		expect(layoutSource).not.toContain('class="queue-action icon"');
	});

	test('the shared menu reaches favourite + queue mutations for ephemeral TIDAL rows', () => {
		// Ephemeral TIDAL rows are real, mutable queue rows now, so their menu
		// carries the queue-aware actions keyed on the real queue id.
		expect(menuSource).toContain('queueItemId?: number');
		expect(menuSource).toContain("label: 'Move next'");
		expect(menuSource).toContain("label: 'Remove from queue'");
	});
});
