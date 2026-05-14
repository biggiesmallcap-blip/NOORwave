import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, '[id]', '+page.svelte'), 'utf8');

function cssBlock(selector: string): string {
	const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
	const match = source.match(new RegExp(`${escaped}\\s*\\{(?<body>[^}]*)\\}`));
	if (!match?.groups?.body) {
		throw new Error(`Missing CSS block for ${selector}`);
	}
	return match.groups.body;
}

describe('artist page layout contracts', () => {
	test('keeps the artist portrait top-aligned as the biography expands', () => {
		const hero = cssBlock('.hero');
		expect(hero).toContain('align-items: flex-start');

		const body = cssBlock('.hero-body');
		expect(body).toContain('align-items: flex-start');

		const portraitWrap = cssBlock('.hero-portrait-wrap');
		expect(portraitWrap).toContain('align-self: flex-start');
	});

	test('keeps expanded biography readable without pushing the page too far down', () => {
		expect(source).toContain('class="hero-bio-panel" class:expanded={bioExpanded}');

		const expandedBio = cssBlock('.hero-bio-panel.expanded .hero-bio');
		expect(expandedBio).toContain('max-height: clamp(');
		expect(expandedBio).toContain('overflow-y: auto');

		const bio = cssBlock('.hero-bio');
		expect(bio).toContain('white-space: pre-line');
	});

	test('shows Spotify world plays beside local plays only through TrackRow', () => {
		expect(source).toContain('worldPlayCount={streamCount ?? null}');
		expect(source).not.toContain('class="stream-badge"');
	});
});
