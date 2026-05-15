export function normalizeRemoteSearchQuery(input: string): string {
	return input.trim().replace(/\s+/g, ' ');
}

export function shouldRunRemoteSearch(input: string): boolean {
	return normalizeRemoteSearchQuery(input).length >= 2;
}

/**
 * Guards async search responses against a query that changed while the request
 * was in flight. `begin()` opens a new search and returns a token; the response
 * handler applies its result only when `isCurrent(token)` is still true.
 * `invalidate()` is the part that bit us: clearing or shortening the query has
 * to advance the sequence too, otherwise a late response repopulates results
 * the user already cleared.
 */
export interface RemoteSearchGate {
	begin(): number;
	invalidate(): void;
	isCurrent(token: number): boolean;
}

export function createRemoteSearchGate(): RemoteSearchGate {
	let seq = 0;
	return {
		begin: () => ++seq,
		invalidate: () => {
			seq++;
		},
		isCurrent: (token: number) => token === seq
	};
}
