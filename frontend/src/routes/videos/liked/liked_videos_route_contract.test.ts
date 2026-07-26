import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, test } from 'vitest';

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, '+page.svelte'), 'utf8');

describe('liked videos route contract', () => {
	// This page hand-rolls its card instead of using VideoCard (it needs the
	// versions chip and popout inside the slot), so the hover treatment has to be
	// restated here. It was missing once already, which left a wall of cards that
	// did nothing under the cursor and a play badge that never appeared.
	test('reveals the play overlay on card hover and focus', () => {
		expect(source).toContain('.video-card:hover :global(.play-overlay)');
		expect(source).toContain('.video-card:focus-visible :global(.play-overlay)');
	});

	test('lifts the whole slot so the versions chip travels with the poster', () => {
		expect(source).toContain('transition: transform var(--motion-base);');
		expect(source).toContain('.card-slot:hover:not(.open) {');
		expect(source).toContain('transform: translateY(-4px);');
	});

	test('deepens the poster shadow with the lift', () => {
		expect(source).toContain('box-shadow: 0 2px 8px rgba(0, 0, 0, 0.22);');
		expect(source).toContain('.card-slot:hover:not(.open) .poster-wrap {');
		expect(source).toContain('box-shadow: 0 12px 26px -6px rgba(0, 0, 0, 0.5);');
	});

	test('keeps the duration clear of the corner play badge', () => {
		expect(source).toContain('position="corner"');
		expect(source).toMatch(/\.duration \{[^}]*left: 6px;/);
		expect(source).not.toMatch(/\.duration \{[^}]*right: 6px;/);
	});

	test('stands the motion down under prefers-reduced-motion', () => {
		expect(source).toMatch(
			/@media \(prefers-reduced-motion: reduce\) \{[\s\S]*\.card-slot:hover:not\(\.open\) \{\s*transform: none;/
		);
	});
});
