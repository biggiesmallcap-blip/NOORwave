import { readFileSync } from 'node:fs';
import { describe, expect, test } from 'vitest';

describe('overlay stack contract', () => {
	test('context menu renders above modal panels', () => {
		const contextMenu = readFileSync('src/lib/components/ContextMenu.svelte', 'utf8');
		const quietMode = readFileSync('src/lib/components/QuietMode.svelte', 'utf8');

		expect(quietMode).toContain('z-index: calc(var(--z-modal) + 1)');
		expect(contextMenu).toContain('z-index: var(--z-toast)');
		expect(contextMenu).toContain('max-height: calc(100dvh - 16px)');
		expect(contextMenu).toContain('overflow-y: auto');
	});

	test('context menu closes when pointer leaves the menu surface', () => {
		const contextMenu = readFileSync('src/lib/components/ContextMenu.svelte', 'utf8');

		expect(contextMenu).toContain('function handlePointerLeave');
		expect(contextMenu).toContain('function handlePointerEnter');
		expect(contextMenu).toContain('onpointerleave={handlePointerLeave}');
		expect(contextMenu).toContain('onpointerenter={handlePointerEnter}');
		expect(contextMenu).toContain('closeContextMenu();');
	});

	test('context menu has matching enter and soft-exit animation states', () => {
		const component = readFileSync('src/lib/components/ContextMenu.svelte', 'utf8');
		const store = readFileSync('src/lib/stores/context_menu.ts', 'utf8');

		expect(store).toContain('closing: boolean');
		expect(store).toContain('CONTEXT_MENU_EXIT_MS = 160');
		expect(store).toContain('cancelContextMenuClose');
		expect(store).toContain('closing: true');
		expect(store).toContain('closing: false');
		expect(component).toContain('class:closing={$contextMenu.closing}');
		expect(component).toContain('context-menu-enter');
		expect(component).toContain('context-menu-exit');
		expect(component).toContain('animation: context-menu-enter 160ms');
		expect(component).toContain('animation: context-menu-exit 160ms');
		expect(component).toContain('.context-menu.closing');
		expect(component).not.toContain('pointer-events: none;');
	});
});
