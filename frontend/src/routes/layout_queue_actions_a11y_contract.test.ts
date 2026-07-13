import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, '+layout.svelte'), 'utf8');
const normalizedSource = source.replace(/\r\n/g, '\n');

describe('queue row accessibility contracts', () => {
	test('the whole row is one labelled play target, with a single labelled overflow action', () => {
		// Play is a full-bleed hit button (sidebar + now-playing blocks).
		expect(source.match(/class="queue-row-hit"/g)?.length).toBe(2);
		expect(source).toContain(
			'aria-label={isPending ? `Play ${item.track.title} (resolving)` : `Play ${item.track.title}`}'
		);
		// Actions collapse to one always-present, labelled overflow button.
		expect(source.match(/class="queue-overflow"/g)?.length).toBe(2);
		expect(source).toContain("aria-label=\"More actions\"");
		// The old hover-gated visibility machinery is gone, so nothing is hidden
		// from the accessibility tree.
		expect(source).not.toContain('queueActionsAccessible');
		expect(source).not.toContain('inert={!actionsAccessible}');
	});

	test('pending rows are playable (play-item resolves them); duration is not hidden on hover', () => {
		// Pending rows used to be play-disabled; the play-item route now resolves
		// (imports) them on click, so the play target is never disabled.
		expect(source).not.toContain('disabled={isPending}');
		expect(source).not.toContain('onclick={isPending ? undefined');
		// Duration stays visible at all times now (no opacity dance).
		expect(normalizedSource).not.toContain('.queue-row:hover .queue-time {');
		expect(normalizedSource).not.toContain('.queue-actions {');
	});
});
