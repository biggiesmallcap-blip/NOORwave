/**
 * Parse a queue-row provenance string into a structured breakdown.
 *
 * The backend emits reasons as `"<prefix> | <json>"` (sometimes with more
 * human text after the JSON) where the prefix is human-readable and the JSON
 * carries scoring components for
 * the queue tooltip (genre Jaccard, affinity multiplier, etc.). Older
 * reasons from before Phase 2b only have the prefix; we degrade
 * gracefully — the prefix is always preserved.
 *
 * Returns `null` for null / empty input so callers can early-exit and
 * skip the tooltip entirely.
 */

export interface ReasonBreakdown {
	/** Human-readable prefix from the backend. Always present. */
	prefix: string;
	/**
	 * Weighted-Jaccard genre similarity with the seed, in [0, 1].
	 * Phase 2b Stage 1 emits this for every library/engine candidate
	 * with genre data. Last.fm hits leave it absent.
	 */
	genre_jaccard?: number;
	/**
	 * Affinity multiplier applied by `apply_taste_signals` —
	 * `post_score / pre_score`. 1.0 means "no change", > 1.0 means
	 * "user likes this artist", < 1.0 means "user has skipped this
	 * artist recently".
	 */
	affinity_mult?: number;
}

const SEPARATOR = ' | ';

export function parseReason(raw: string | null | undefined): ReasonBreakdown | null {
	if (!raw) return null;
	const trimmed = raw.trim();
	if (!trimmed) return null;

	// Reasons are ' | '-joined segments: human text and JSON score objects,
	// in any order ("automix: audio texture | {...} | dj: hub penalty |
	// {"dj_score":...}"). The JSON is never for display.
	const segments = trimmed.split(SEPARATOR).map((segment) => segment.trim()).filter(Boolean);
	const isJson = (segment: string) => segment.startsWith('{');
	const human = segments.filter((segment) => !isJson(segment)).join('; ');
	const out: ReasonBreakdown = { prefix: human || trimmed };

	for (const segment of segments.filter(isJson)) {
		try {
			const parsed = JSON.parse(segment) as Partial<ReasonBreakdown>;
			if (typeof parsed.genre_jaccard === 'number' && Number.isFinite(parsed.genre_jaccard)) {
				out.genre_jaccard ??= parsed.genre_jaccard;
			}
			if (typeof parsed.affinity_mult === 'number' && Number.isFinite(parsed.affinity_mult)) {
				out.affinity_mult ??= parsed.affinity_mult;
			}
		} catch {
			// Malformed or truncated JSON (the server caps reason length):
			// the human segments are still the reason.
		}
	}
	return out;
}

/**
 * Format the affinity multiplier as a percentage delta — 1.08 → "+8%",
 * 0.82 → "-18%". Returns null for missing or near-1.0 values that
 * wouldn't be useful to display.
 */
export function formatAffinityDelta(multiplier: number | undefined): string | null {
	if (multiplier === undefined || !Number.isFinite(multiplier)) return null;
	const delta = (multiplier - 1.0) * 100;
	if (Math.abs(delta) < 0.5) return null;
	const sign = delta > 0 ? '+' : '';
	return `${sign}${delta.toFixed(0)}%`;
}

/**
 * Format Jaccard as a percentage. 0.67 → "67%". Returns null for
 * missing values.
 */
export function formatJaccardPct(jaccard: number | undefined): string | null {
	if (jaccard === undefined || !Number.isFinite(jaccard)) return null;
	return `${Math.round(jaccard * 100)}%`;
}

/**
 * Render a queue row's reason as a single plain-text sentence for
 * screen readers. The hover card is `pointer-events: none` and
 * positioned globally, so SR cannot reach it via aria-describedby on
 * the floating element. We mirror the parsed breakdown inline next to
 * the row's ⓘ trigger and point aria-describedby at that span.
 *
 * Returns null when there's nothing useful to read (no reason at all).
 */
export function formatReasonForScreenReader(raw: string | null | undefined): string | null {
	const parsed = parseReason(raw);
	if (!parsed) return null;
	const parts: string[] = [];
	if (parsed.prefix) parts.push(parsed.prefix);
	const jaccard = formatJaccardPct(parsed.genre_jaccard);
	if (jaccard !== null) parts.push(`Genre overlap ${jaccard}`);
	const affinity = formatAffinityDelta(parsed.affinity_mult);
	if (affinity !== null) parts.push(`Affinity ${affinity}`);
	if (parts.length === 0) return null;
	return parts.join('. ');
}
