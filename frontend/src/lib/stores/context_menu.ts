import { writable } from 'svelte/store';

export interface MenuItem {
	label: string;
	icon?: string;
	hint?: string;
	onSelect?: () => void | Promise<void>;
	danger?: boolean;
	disabled?: boolean;
	submenu?: MenuItem[];
	separator?: boolean;
}

export interface ContextMenuState {
	open: boolean;
	x: number;
	y: number;
	items: MenuItem[];
	title?: string;
}

const initial: ContextMenuState = { open: false, x: 0, y: 0, items: [], title: undefined };

export const contextMenu = writable<ContextMenuState>(initial);

export function openContextMenu(
	event: MouseEvent | { clientX: number; clientY: number; preventDefault?: () => void; stopPropagation?: () => void },
	items: MenuItem[],
	title?: string
) {
	if ('preventDefault' in event && typeof event.preventDefault === 'function') {
		event.preventDefault();
	}
	// Stop the same contextmenu event from bubbling up to <svelte:window> in
	// ContextMenu.svelte, where the global handler would close the menu the
	// instant it opens (the click that triggered open isn't inside menuEl yet).
	if ('stopPropagation' in event && typeof event.stopPropagation === 'function') {
		event.stopPropagation();
	}
	contextMenu.set({
		open: true,
		x: event.clientX,
		y: event.clientY,
		items,
		title
	});
}

export function openMenuAtElement(el: HTMLElement, items: MenuItem[], title?: string) {
	const rect = el.getBoundingClientRect();
	contextMenu.set({
		open: true,
		x: rect.right,
		y: rect.bottom + 4,
		items,
		title
	});
}

export function closeContextMenu() {
	contextMenu.set(initial);
}
