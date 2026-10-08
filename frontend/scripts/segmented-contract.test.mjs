import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const read = (rel) => readFileSync(resolve(import.meta.dirname, rel), 'utf8');
const seg = read('../src/lib/components/ui/Segmented.svelte');
const mixIntent = read('../src/lib/components/dj-cockpit/MixIntentControl.svelte');

// STYLING.md "Segmented": choosing one value. A sunken track with a sliding
// neutral thumb, never accent; a radio group with one tab stop.
describe('Segmented', () => {
	test('is a radio group with one tab stop', () => {
		expect(seg).toContain('role="radiogroup"');
		expect(seg).toContain('role="radio"');
		expect(seg).toContain('aria-checked={value === option.value}');
		expect(seg).toContain('tabindex={value === option.value || (selected < 0 && index === 0) ? 0 : -1}');
	});

	test('arrow keys, Home and End choose the neighbouring value', () => {
		for (const key of ["'ArrowRight'", "'ArrowLeft'", "'ArrowDown'", "'ArrowUp'", "'Home'", "'End'"]) {
			expect(seg).toContain(key);
		}
		// The window's player shortcuts read arrows as seek and volume.
		expect(seg).toContain('event.stopPropagation();');
	});

	test('the thumb is neutral, slides on --motion-base, and stops sliding for reduced motion', () => {
		const thumb = seg.match(/\.thumb\s*\{([^}]*)\}/)?.[1] ?? '';
		expect(thumb).toContain('background: var(--bg-raised);');
		expect(thumb).toContain('transition: transform var(--motion-base);');
		expect(seg).not.toMatch(/--accent/);
		expect(seg).toMatch(/@media \(prefers-reduced-motion: reduce\)\s*\{\s*\.thumb\s*\{\s*transition: none;/);
	});

	test('the track is sunken: surface fill, no border', () => {
		const track = seg.match(/\.segmented\s*\{([^}]*)\}/)?.[1] ?? '';
		expect(track).toContain('background: var(--bg-surface);');
		expect(track).not.toContain('border:');
	});
});

describe('DJ mix intent and speed use Segmented', () => {
	test('both choices mount the shared control; no local segmented styles remain', () => {
		expect(mixIntent).toContain('<Segmented label="Mix intent"');
		expect(mixIntent).toContain('<Segmented label="Transition speed"');
		expect(mixIntent).not.toContain('class="segmented"');
		expect(mixIntent).not.toContain('aria-pressed');
	});
});
