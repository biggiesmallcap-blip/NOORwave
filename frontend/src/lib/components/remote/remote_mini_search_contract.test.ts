import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, 'RemoteMiniSearch.svelte'), 'utf8');

describe('remote mini search contracts', () => {
	test('aborts stale remote search requests when the query or mode changes', () => {
		expect(source).toContain('const controller = new AbortController();');
		expect(source).toContain('api.searchTidal(normalized, 12, controller.signal)');
		expect(source).toContain('api.search(normalized, 12, controller.signal)');
		expect(source).toContain('if (controller.signal.aborted) return;');
		expect(source).toContain('controller.abort();');
	});
});
