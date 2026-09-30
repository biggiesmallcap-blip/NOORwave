import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const routeSource = readFileSync(join(here, '+page.svelte'), 'utf8');
const shellSource = readFileSync(join(here, '../+layout.svelte'), 'utf8');

function cssBlock(source: string, selector: string): string {
	const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
	const match = source.match(new RegExp(`(?:^|\\n)\\s*${escaped}\\s*\\{(?<body>[^}]*)\\}`));
	if (!match?.groups?.body) throw new Error(`Missing CSS block for ${selector}`);
	return match.groups.body;
}

describe('genre galaxy layout contracts', () => {
	test('full-bleed geometry follows the shell workspace instead of the viewport', () => {
		const workspace = cssBlock(shellSource, '.workspace');
		const route = cssBlock(routeSource, '.genres-route');
		const stage = cssBlock(routeSource, '.galaxy-stage');

		for (const edge of ['top', 'right', 'bottom', 'left']) {
			expect(workspace).toContain(`--workspace-pad-${edge}:`);
			expect(route).toContain(`var(--workspace-pad-${edge})`);
		}

		expect(workspace).toContain(
			'padding: var(--workspace-pad-top) var(--workspace-pad-right) var(--workspace-pad-bottom) var(--workspace-pad-left)'
		);
		expect(route).toContain(
			'height: calc(100% + var(--workspace-pad-top) + var(--workspace-pad-bottom))'
		);
		expect(route).toContain('min-height: 0');
		expect(stage).toContain('height: 100%');
		expect(routeSource).not.toContain('min-height: 100vh');
		expect(routeSource).not.toContain('min-height: calc(100dvh - 40px)');
	});
});
