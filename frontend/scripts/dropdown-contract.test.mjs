import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const read = (rel) => readFileSync(resolve(import.meta.dirname, rel), 'utf8');
const dd = read('../src/lib/components/ui/Dropdown.svelte');
const mixIntent = read('../src/lib/components/dj-cockpit/MixIntentControl.svelte');

// STYLING.md "Dropdown": a sunken trigger that opens a listbox in the shared
// overlay layer. Reachable and closable by keyboard; focus stays inside the
// open list until it closes, then returns to the trigger.
describe('Dropdown', () => {
	test('the trigger announces a listbox and its state', () => {
		expect(dd).toContain('aria-haspopup="listbox"');
		expect(dd).toContain('aria-expanded={open}');
		expect(dd).toContain('aria-labelledby="{labelId} {valueId}"');
	});

	test('the list renders through the portal in the overlay layer', () => {
		expect(dd).toContain("import { portal } from '$lib/actions/portal';");
		expect(dd).toContain('use:portal');
		expect(dd).toMatch(/\.menu\s*\{[^}]*z-index: var\(--z-overlay\);/);
		expect(dd).toContain('role="listbox"');
		expect(dd).toContain('role="option"');
		expect(dd).toContain('aria-activedescendant=');
	});

	test('keyboard: arrows open and move, Enter and Space choose, Escape and Tab close back to the trigger', () => {
		for (const key of ["'ArrowDown'", "'ArrowUp'", "'Home'", "'End'", "'Enter'", "' '", "'Escape'", "'Tab'"]) {
			expect(dd).toContain(key);
		}
		expect(dd).toContain('trigger?.focus()');
		// Space, arrows and letters are player shortcuts on the window; the
		// trigger and the open list keep them.
		expect(dd).toMatch(/function onTriggerKeydown[\s\S]*?event\.stopPropagation\(\);/);
		expect(dd).toMatch(/function onMenuKeydown\(event: KeyboardEvent\) \{[\s\S]*?event\.stopPropagation\(\);\s*const last/);
	});

	test('a press outside closes it; scrolling or resizing the page closes it', () => {
		expect(dd).toContain("addEventListener('pointerdown'");
		expect(dd).toContain("addEventListener('resize'");
		expect(dd).toContain("addEventListener('scroll'");
	});

	test('enters on --motion-fast, exits on --motion-exit, opacity only under reduced motion', () => {
		expect(dd).toMatch(/animation: menu-in var\(--motion-fast\)/);
		expect(dd).toMatch(/animation: menu-out var\(--motion-exit\)/);
		expect(dd).toMatch(/@media \(prefers-reduced-motion: reduce\)/);
	});

	test('the trigger is sunken: surface fill, no border', () => {
		const trigger = dd.match(/\.trigger\s*\{([^}]*)\}/)?.[1] ?? '';
		expect(trigger).toContain('background: var(--bg-surface);');
		expect(trigger).toContain('border: 0;');
	});
});

describe('DJ transition style uses Dropdown', () => {
	test('the native select is replaced', () => {
		expect(mixIntent).toContain('<Dropdown label="Transition style"');
		expect(mixIntent).not.toContain('<select');
	});
});
