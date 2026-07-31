import { writable, type Writable } from 'svelte/store';

// Multi-select state for a list of numeric ids, as its own instance per surface.
//
// Library owned the only implementation, as module-level singletons. A second
// surface reusing those stores would share one selection with library, so
// selecting three tracks on a playlist page would leave them selected in the
// library batch bar too. This factory keeps the semantics and scopes the state.

export interface Selection {
	/** The currently selected ids. */
	readonly ids: Writable<Set<number>>;
	/** The last id passed to `select`, for shift-range anchoring. */
	readonly lastId: Writable<number | null>;
	/**
	 * Select `ids`.
	 *
	 * `additive: false` replaces the selection. `additive: true` unions into it,
	 * except for the single-id case, which toggles - that is what makes a
	 * ctrl-click on an already-selected row deselect it.
	 */
	select(ids: number[], additive?: boolean): void;
	clear(): void;
}

export function createSelection(): Selection {
	const ids = writable<Set<number>>(new Set());
	const lastId = writable<number | null>(null);

	function select(next: number[], additive = false) {
		ids.update((set) => {
			const out = new Set(additive ? set : []);
			for (const id of next) {
				if (out.has(id) && additive && next.length === 1) {
					out.delete(id);
				} else {
					out.add(id);
				}
			}
			return out;
		});
		lastId.set(next.at(-1) ?? null);
	}

	function clear() {
		ids.set(new Set());
		lastId.set(null);
	}

	return { ids, lastId, select, clear };
}
