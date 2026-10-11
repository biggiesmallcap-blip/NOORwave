// The primary phase of a search: library results and TIDAL results race, and
// the page shows the merge of whatever has arrived. Local results wait a short
// hold for TIDAL so the list does not jump twice; a slow TIDAL still gets a
// local-only view after the hold. Every callback is skipped once the search is
// no longer current (newer query, abort).

import type { SearchResults, TidalSearchResults } from '$lib/api/client';
import { mergeLocalIntoTidal } from './merge_local';

/** How long local results wait for TIDAL before showing on their own. */
export const LOCAL_RESULTS_HOLD_MS = 300;

export const EMPTY_TIDAL_RESULTS: TidalSearchResults = {
	tracks: [],
	albums: [],
	artists: [],
	videos: [],
};

export interface PrimarySearch {
	local: Promise<SearchResults>;
	/** Resolves with TIDAL's results (or the cached ones). */
	tidal: Promise<TidalSearchResults>;
	/** False once a newer search started or this one was aborted. */
	isCurrent: () => boolean;
	/** Show this merged result set. */
	show: (results: TidalSearchResults) => void;
	/** TIDAL answered (first time for this search): page/caching bookkeeping. */
	onTidal: (raw: TidalSearchResults) => void;
	/** TIDAL failed; `hadLocal` says whether library results are already showing. */
	onTidalError: (error: unknown, hadLocal: boolean) => void;
	onLocalSettled: () => void;
	onTidalSettled: () => void;
	holdMs?: number;
}

/** Run the primary phase; resolves when both providers have settled. */
export async function runPrimarySearch(search: PrimarySearch): Promise<void> {
	const holdMs = search.holdMs ?? LOCAL_RESULTS_HOLD_MS;
	let localSnapshot: SearchResults | null = null;
	let tidalSnapshot: TidalSearchResults | null = null;

	const local = search.local
		.then((localResults) => {
			if (!search.isCurrent()) return;
			localSnapshot = localResults;
			const apply = () => {
				if (!search.isCurrent()) return;
				search.show(mergeLocalIntoTidal(localResults, tidalSnapshot ?? EMPTY_TIDAL_RESULTS));
			};
			if (tidalSnapshot) apply();
			else
				setTimeout(() => {
					if (!tidalSnapshot) apply();
				}, holdMs);
		})
		.catch(() => undefined)
		.finally(() => {
			if (search.isCurrent()) search.onLocalSettled();
		});

	const tidal = search.tidal
		.then((tidalResults) => {
			if (!search.isCurrent()) return;
			tidalSnapshot = tidalResults;
			search.show(localSnapshot ? mergeLocalIntoTidal(localSnapshot, tidalResults) : tidalResults);
			search.onTidal(tidalResults);
		})
		.catch((error) => {
			if (!search.isCurrent()) return;
			search.onTidalError(error, localSnapshot !== null);
		})
		.finally(() => {
			if (search.isCurrent()) search.onTidalSettled();
		});

	await Promise.allSettled([local, tidal]);
}

/**
 * One secondary provider (TIDAL or Spotify playlists): apply its answer only
 * while the search is still current, swallow its failure (the rail just stays
 * empty), and clear its loading flag once it settles.
 */
export async function settleWhileCurrent<T>(
	answer: Promise<T>,
	isCurrent: () => boolean,
	onResult: (value: T) => void,
	onSettled: () => void,
): Promise<void> {
	try {
		const value = await answer;
		if (isCurrent()) onResult(value);
	} catch {
		// A secondary provider failing never surfaces as a search error.
	} finally {
		if (isCurrent()) onSettled();
	}
}
