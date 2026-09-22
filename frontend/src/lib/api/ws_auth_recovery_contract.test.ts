import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const source = readFileSync(join(dirname(fileURLToPath(import.meta.url)), 'ws.ts'), 'utf8');

describe('WebSocket authentication recovery', () => {
	it('revalidates a saved device credential instead of deleting it on one socket rejection', () => {
		const rejected = source.slice(source.indexOf('if (event.code === 4001)'), source.indexOf('if (reconnectAllowed)'));
		expect(rejected).toContain("new CustomEvent('noor:unauthorized')");
		expect(rejected).not.toContain('clearRemoteSession()');
	});
});
