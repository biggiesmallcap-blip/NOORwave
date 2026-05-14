import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, 'TrackRow.svelte'), 'utf8');

describe('TrackRow world play count contract', () => {
	test('keeps world play counts opt-in for scoped pages', () => {
		expect(source).toContain('worldPlayCount');
		expect(source).toContain('formatCompactCount');
		expect(source).toContain('{#if worldPlayCount != null}');
		expect(source).toContain('play-count-local-label');
		expect(source).toContain('class="play-count-world"');
		expect(source).toContain('showPlayCount = false');
	});
});
