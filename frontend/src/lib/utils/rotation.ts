/**
 * Rotating windows over a list that is longer than the space it is shown in.
 *
 * Several surfaces cache far more than they display: the Home recommendation
 * shelves hold fifty items behind a twenty-card rail, and the /search idle state
 * holds every playlist behind a twenty-four card stack. Those extra items are
 * already fetched and already paid for, so showing the same slice every time is
 * the only actual waste. Rotating the window is a fresh-looking surface for zero
 * additional requests.
 */

/**
 * `size` items starting at `offset`, wrapping around the end of the list.
 *
 * Wrapping rather than paging is deliberate: a plain page boundary makes the
 * last page ragged (fifty items in pages of twenty ends on a page of ten, so
 * that rail is visibly half empty), while a wrapping window is always exactly
 * `size` long. It is the same semantics as `rotate_take` in
 * `noor-server/src/server/routes/home_routes.rs`, which rotates the Last.fm seed
 * set the same way.
 *
 * Returns the list unchanged when it already fits, so a caller never has to
 * check first.
 */
export function rotatingWindow<T>(items: T[], size: number, offset: number): T[] {
	if (items.length === 0 || size <= 0) return [];
	if (items.length <= size) return items;
	const start = ((offset % items.length) + items.length) % items.length;
	return Array.from({ length: size }, (_, i) => items[(start + i) % items.length]);
}

/**
 * Which rotation a surface is on right now, derived from the clock.
 *
 * Clock-derived rather than random or per-visit, for two reasons. It has to be
 * stable for the length of a session: an offset that moved on every read would
 * reshuffle cards under the user mid-scroll, and a rail whose contents change
 * while being dragged through is worse than one that repeats. And it has to
 * change on its own, so coming back after lunch shows a different set without
 * anything having to be stored or invalidated.
 *
 * Call once per component instance and keep the result; do not recompute inside
 * a `$derived`.
 */
export function rotationForPeriod(periodMs: number, now: number = Date.now()): number {
	if (periodMs <= 0) return 0;
	return Math.floor(now / periodMs);
}
