import { artworkFallbackCandidate, type TidalArtworkSize } from './artwork';

/**
 * Per-component artwork fallback: `candidate` picks the URL to render and
 * `markFailed` (wired to the img onerror) steps it down to the next size.
 */
export function createArtworkFallback() {
	let failed = $state<Record<string, boolean>>({});

	return {
		candidate(rawUrl: string | null | undefined, size: TidalArtworkSize): string | null {
			return artworkFallbackCandidate(rawUrl, size, (url) => failed[url] === true);
		},
		markFailed(renderedUrl: string | null | undefined): void {
			if (!renderedUrl) return;
			failed = { ...failed, [renderedUrl]: true };
		},
		/** Forget failures, e.g. when the page switches to another album or artist. */
		reset(): void {
			failed = {};
		},
	};
}
