// Rules for the Home recommendation shelves: how a progressive server payload
// merges over what is on screen, and which state the shelf renders. Kept out of
// the component so they can be tested by behaviour.

import type { ProviderRecommendationShelf } from '$lib/api/client';

export type ShelfViewState = 'hidden' | 'loading' | 'ready' | 'empty' | 'error';

/** The rail shows this many items; the rest goes to the View all page. */
export const PANEL_LIMIT = 20;

export function shelfKey(shelf: ProviderRecommendationShelf): string {
	return `${shelf.provider}:${shelf.entity_type ?? 'track'}:${shelf.title}`;
}

export function hasItems(list: readonly ProviderRecommendationShelf[]): boolean {
	return list.some((shelf) => shelf.items.length > 0);
}

/**
 * A shelf the server is still building. It is empty right now but more is
 * coming, so it must never be rendered as "nothing to recommend".
 */
export function isWarming(list: readonly ProviderRecommendationShelf[]): boolean {
	return list.some((shelf) => shelf.status === 'warming');
}

/**
 * Merge an incoming payload over what is on screen, shelf by shelf.
 *
 * The server publishes one shelf at a time, so mid-rebuild the rails it has
 * not reached yet come back empty and warming. Taking that payload wholesale
 * would blank rails that are currently full and refill them seconds later. A
 * warming shelf with no items therefore never displaces one that has them;
 * anything else, warming or not, is the newer truth and wins.
 */
export function mergeShelves(
	current: readonly ProviderRecommendationShelf[],
	next: readonly ProviderRecommendationShelf[],
): ProviderRecommendationShelf[] {
	const byKey = new Map(current.map((shelf) => [shelfKey(shelf), shelf]));
	return next.map((shelf) => {
		if (shelf.items.length > 0 || shelf.status !== 'warming') return shelf;
		return byKey.get(shelfKey(shelf)) ?? shelf;
	});
}

/** What to render given the shelves in hand (once the provider gate is open). */
export function paintState(shelves: readonly ProviderRecommendationShelf[]): ShelfViewState {
	if (hasItems(shelves)) return 'ready';
	return isWarming(shelves) ? 'loading' : 'empty';
}

/** The first frame on boot: paint cached shelves when a provider can recommend. */
export function seededViewState(
	canRecommend: boolean,
	seeded: readonly ProviderRecommendationShelf[],
): ShelfViewState {
	return canRecommend && hasItems(seeded) ? 'ready' : 'hidden';
}

/** True when the shelf is holding back items the rail is not showing. */
export function hasMoreThanShelf(shelf: ProviderRecommendationShelf): boolean {
	return shelf.items.length > PANEL_LIMIT;
}
