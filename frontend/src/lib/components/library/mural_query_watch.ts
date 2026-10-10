import type { CacheState } from '$lib/cache/query';

export interface WatchableQuery<T> {
	subscribe(run: (state: CacheState<T>) => void): () => void;
	refresh(): Promise<T>;
}

export interface MuralWatchOptions<T> {
	isEmpty: (data: T) => boolean;
	onData: (data: T) => void;
	onError?: (error: unknown) => void;
	/** Delay before each follow-up refresh; its length bounds the attempts. */
	delaysMs: readonly number[];
}

// Default backoff: an empty or failed mural sample is usually the server still
// warming up at boot, which settles within seconds; a library with no listen
// history stays empty and stops after the last attempt.
export const MURAL_REFRESH_DELAYS_MS = [2000, 5000, 15000, 30000, 60000] as const;

/**
 * Subscribe a mounted mural to its cache query and keep refreshing, on a
 * bounded backoff, while the result is an error or an empty payload. The cache
 * has no expiry timer, so a short staleMs alone never refetched a page that
 * stayed open. Returns the cleanup for unmount, which also cancels any pending
 * refresh.
 */
export function watchMuralQuery<T>(query: WatchableQuery<T>, options: MuralWatchOptions<T>): () => void {
	let attempt = 0;
	let timer: ReturnType<typeof setTimeout> | null = null;
	let stopped = false;

	const unsubscribe = query.subscribe((state) => {
		if (state.data !== undefined) options.onData(state.data);
		if (state.error) options.onError?.(state.error);
		if (state.loading || state.refreshing || timer !== null || stopped) return;
		const needsRefresh = state.error != null || (state.data !== undefined && options.isEmpty(state.data));
		if (!needsRefresh) {
			attempt = 0;
			return;
		}
		const delay = options.delaysMs[attempt];
		if (delay === undefined) return;
		attempt++;
		timer = setTimeout(() => {
			timer = null;
			if (!stopped) void query.refresh().catch(() => undefined);
		}, delay);
	});

	return () => {
		stopped = true;
		if (timer !== null) clearTimeout(timer);
		timer = null;
		unsubscribe();
	};
}
