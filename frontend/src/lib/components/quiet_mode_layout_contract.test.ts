import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, 'QuietMode.svelte'), 'utf8');

function cssBlock(selector: string): string {
	const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
	const match = source.match(new RegExp(`${escaped}\\s*\\{(?<body>[^}]*)\\}`));
	if (!match?.groups?.body) {
		throw new Error(`Missing CSS block for ${selector}`);
	}
	return match.groups.body;
}

describe('quiet mode layout contracts', () => {
	test('artwork scales fluidly so 1080p is not locked to the max cover size', () => {
		expect(source).toContain('--quiet-art-size: clamp(');
		expect(source).toContain('--quiet-panel-w: min(var(--quiet-art-size), calc(100vw - (2 * var(--quiet-panel-pad))));');

		const panel = cssBlock('.quiet-panel');
		expect(panel).toContain('grid-template-columns: minmax(0, var(--quiet-panel-w))');
		expect(panel).toContain('gap: var(--quiet-panel-gap)');
		expect(panel).toContain('padding: var(--quiet-panel-pad)');

		const art = cssBlock('.quiet-art-wrap');
		expect(art).toContain('width: var(--quiet-panel-w)');
		expect(art).not.toContain('max-width: min(60vh, 520px)');
	});
});
