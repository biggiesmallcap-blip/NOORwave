import { writable, type Readable } from 'svelte/store';

/**
 * Svelte action: HTML5 drag-to-reorder for a list of rows keyed by numeric id.
 *
 * Extracted from the playback queue, which was the only implementation. Two
 * details in here are not obvious and were learned there - keep them:
 *
 * - `dataTransfer.setData('text/plain', ...)` on dragstart. Firefox refuses to
 *   start a drag without it.
 * - The row's click target must be a `div role="button"`, not a real `<button>`.
 *   A button swallows `dragstart` and the row never becomes draggable.
 *
 * Index semantics: `onDrop` receives the destination index AFTER the dragged row
 * has been spliced out, which is what both `moveQueueItem` and the playlist move
 * endpoint expect. `reorderDropIndex` in `$lib/stores/player` does that
 * conversion and is applied here, so callers get a ready-to-send index.
 */

export interface DragReorderState {
	/** Id of the row currently being dragged, or null. */
	draggingId: number | null;
	/** Id of the row the pointer is currently over, or null. */
	dragOverId: number | null;
}

export interface DragReorderOptions {
	/** Current index of `id` in the list, or -1 if it is gone. */
	indexOf: (id: number) => number;
	/** Commit the move. `toIndex` is already post-splice. */
	onDrop: (id: number, toIndex: number) => void | Promise<void>;
	/** Return false to make a row undraggable (pending rows, the play head). */
	canDrag?: (id: number) => boolean;
	/** Return false to refuse a drop on a row (e.g. above the play head). */
	canDropOn?: (sourceId: number, targetId: number) => boolean;
	/** Total rows, so keyboard reorder knows where the end is. */
	length?: () => number;
	/**
	 * Alt+ArrowUp / Alt+ArrowDown move a row without a pointer. On by default:
	 * a drag-only reorder is unusable by keyboard and screen-reader users.
	 * Handled inside the action rather than as an inline `onkeydown`, which
	 * Svelte flags as a listener on a non-interactive element.
	 */
	keyboardReorder?: boolean;
}

export interface DragReorderController {
	/** `use:controller.row={id}` on each row element. */
	row: (node: HTMLElement, id: number) => { update(id: number): void; destroy(): void };
	/** Subscribe for `class:dragging` / `class:drag-over` bindings. */
	state: Readable<DragReorderState>;
}

/**
 * Convert a drop target index into the index the list will use once the dragged
 * row has been removed.
 *
 * For a downward drag (source above target), splicing the source out shifts the
 * target up one slot, so the index has to lose one to land ON the target's top
 * edge - where the drop indicator is drawn - rather than one row below it.
 * Upward drags are unaffected.
 */
export function reorderDropIndex(sourceIndex: number, targetIndex: number): number {
	return sourceIndex !== -1 && sourceIndex < targetIndex ? targetIndex - 1 : targetIndex;
}

export function createDragReorder(options: DragReorderOptions): DragReorderController {
	const state = writable<DragReorderState>({ draggingId: null, dragOverId: null });
	let draggingId: number | null = null;

	function reset() {
		draggingId = null;
		state.set({ draggingId: null, dragOverId: null });
	}

	function row(node: HTMLElement, id: number) {
		let current = id;

		function onDragStart(event: DragEvent) {
			if (options.canDrag && !options.canDrag(current)) return;
			draggingId = current;
			state.update((s) => ({ ...s, draggingId: current }));
			const dt = event.dataTransfer;
			if (!dt) return;
			dt.effectAllowed = 'move';
			// Required for Firefox to actually start a drag.
			dt.setData('text/plain', String(current));
			// Drag the whole row as the ghost (not the bare grip glyph) so the
			// preview reads as "moving this row", aligned under the cursor.
			if (typeof dt.setDragImage === 'function') {
				const rect = node.getBoundingClientRect();
				dt.setDragImage(node, event.clientX - rect.left, event.clientY - rect.top);
			}
		}

		function onDragOver(event: DragEvent) {
			if (draggingId === null) return;
			if (options.canDropOn && !options.canDropOn(draggingId, current)) return;
			event.preventDefault();
			if (event.dataTransfer) event.dataTransfer.dropEffect = 'move';
			state.update((s) => (s.dragOverId === current ? s : { ...s, dragOverId: current }));
		}

		function onDragLeave() {
			state.update((s) => (s.dragOverId === current ? { ...s, dragOverId: null } : s));
		}

		function onDrop(event: DragEvent) {
			event.preventDefault();
			const sourceId = draggingId;
			reset();
			if (sourceId === null || sourceId === current) return;
			if (options.canDropOn && !options.canDropOn(sourceId, current)) return;
			const targetIndex = options.indexOf(current);
			if (targetIndex === -1) return;
			const sourceIndex = options.indexOf(sourceId);
			void options.onDrop(sourceId, reorderDropIndex(sourceIndex, targetIndex));
		}

		function onKeyDown(event: KeyboardEvent) {
			if (!event.altKey) return;
			const delta = event.key === 'ArrowUp' ? -1 : event.key === 'ArrowDown' ? 1 : 0;
			if (delta === 0) return;
			if (options.canDrag && !options.canDrag(current)) return;
			const index = options.indexOf(current);
			if (index === -1) return;
			const target = index + delta;
			const total = options.length?.() ?? Number.MAX_SAFE_INTEGER;
			if (target < 0 || target >= total) return;
			event.preventDefault();
			// Single-step moves need no post-splice correction in either
			// direction: up lands on target, and down past one row is target too.
			void options.onDrop(current, target);
		}

		const keyboard = options.keyboardReorder !== false;

		node.addEventListener('dragstart', onDragStart);
		node.addEventListener('dragover', onDragOver);
		node.addEventListener('dragleave', onDragLeave);
		node.addEventListener('drop', onDrop);
		node.addEventListener('dragend', reset);
		if (keyboard) node.addEventListener('keydown', onKeyDown);

		return {
			update(next: number) {
				current = next;
			},
			destroy() {
				node.removeEventListener('dragstart', onDragStart);
				node.removeEventListener('dragover', onDragOver);
				node.removeEventListener('dragleave', onDragLeave);
				node.removeEventListener('drop', onDrop);
				node.removeEventListener('dragend', reset);
				if (keyboard) node.removeEventListener('keydown', onKeyDown);
			},
		};
	}

	return { row, state };
}
