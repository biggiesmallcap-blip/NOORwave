// The video pages' last answers, kept for the session. A revisit shows them
// at once, with no skeleton and no entry motion (STYLING.md: entry motion
// is not replayed when a cached view is shown again), and the page asks
// again underneath and swaps the fresh answer in quietly. Memory only: a
// relaunch starts clean, so nothing stale outlives the app.

const views = new Map<string, unknown>();

/** The last answer kept under `key`, if this session has one. */
export function lastView<T>(key: string): T | null {
	return (views.get(key) as T | undefined) ?? null;
}

/** Keep `value` as the answer to show next time `key` opens. */
export function keepView<T>(key: string, value: T): void {
	views.set(key, value);
}

/** Forget one answer (after an edit the cache can no longer stand for). */
export function dropView(key: string): void {
	views.delete(key);
}
