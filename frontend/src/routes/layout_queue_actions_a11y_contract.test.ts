import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, '+layout.svelte'), 'utf8');

describe('queue action accessibility contracts', () => {
	test('hidden queue actions leave the accessibility tree until row hover or focus', () => {
		expect(source).toContain('let activeQueueActionsRowId = $state<number | null>(null)');
		expect(source).toContain("window.matchMedia('(max-width: 760px)')");
		expect(source).toContain('function queueActionsAccessible(itemId: number): boolean');
		expect(source).toContain('onfocusin={() => setQueueActionsRowActive(item.id)}');
		expect(source).toContain('onfocusout={(event) => handleQueueRowFocusOut(event, item.id)}');
		expect(source.match(/inert=\{!actionsAccessible\}/g)?.length).toBe(2);
		expect(source.match(/aria-hidden=\{actionsAccessible \? undefined : 'true'\}/g)?.length).toBe(2);
	});
});
