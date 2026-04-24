// Camelot wheel helpers — harmonic mixing compatibility.
//
// A Camelot key is encoded as "<number><letter>", where number is 1..12 and
// letter is "A" (minor) or "B" (major). Adjacent numbers (±1, wrapping around
// the 12-position wheel) with the same letter are harmonically adjacent.
// Same number across A/B is a relative major/minor match ("number match").

import type { AudioDspFeatures } from '$lib/api/client';

/** Parse "8A" → 8. Returns NaN if invalid. */
export function camelotFamily(key: string | null | undefined): number {
	if (!key) return NaN;
	const match = /^(\d{1,2})([AB])$/i.exec(key.trim());
	if (!match) return NaN;
	const n = Number(match[1]);
	if (!Number.isFinite(n) || n < 1 || n > 12) return NaN;
	return n;
}

export function camelotLetter(key: string | null | undefined): 'A' | 'B' | null {
	if (!key) return null;
	const match = /^(\d{1,2})([AB])$/i.exec(key.trim());
	if (!match) return null;
	return match[2].toUpperCase() as 'A' | 'B';
}

function wheelDistance(a: number, b: number): number {
	const raw = Math.abs(a - b);
	return Math.min(raw, 12 - raw);
}

export interface HarmonicCompat {
	level: 'good' | 'okay' | 'clash';
	bpmDelta: number | null;
	keyLabel: string | null;
}

/**
 * Evaluate harmonic compatibility between two tracks. Returns null if either
 * side lacks the DSP features required to reason about compat.
 */
export function harmonicCompat(
	a: AudioDspFeatures | null | undefined,
	b: AudioDspFeatures | null | undefined
): HarmonicCompat | null {
	if (!a || !b) return null;

	const aFamily = camelotFamily(a.camelot_key);
	const bFamily = camelotFamily(b.camelot_key);
	const aLetter = camelotLetter(a.camelot_key);
	const bLetter = camelotLetter(b.camelot_key);

	const hasKeys =
		Number.isFinite(aFamily) && Number.isFinite(bFamily) && aLetter !== null && bLetter !== null;

	const bpmDelta =
		a.bpm != null && b.bpm != null ? Math.round((b.bpm - a.bpm) * 10) / 10 : null;
	const bpmAbs = bpmDelta !== null ? Math.abs(bpmDelta) : null;

	// If neither axis of comparison exists → no indicator.
	if (!hasKeys && bpmAbs === null) return null;

	let keyLabel: string | null = null;
	let sameKey = false;
	let numberMatch = false;
	let adjacent = false;

	if (hasKeys) {
		const distance = wheelDistance(aFamily, bFamily);
		sameKey = aFamily === bFamily && aLetter === bLetter;
		numberMatch = aFamily === bFamily && aLetter !== bLetter;
		adjacent = distance === 1 && aLetter === bLetter;

		if (sameKey) keyLabel = 'same key';
		else if (numberMatch) keyLabel = 'relative';
		else if (adjacent) keyLabel = '+1 step';
		else keyLabel = 'clash';
	}

	const goodKey = sameKey || numberMatch;
	const okayKey = adjacent;
	const goodBpm = bpmAbs !== null && bpmAbs < 10;
	const okayBpm = bpmAbs !== null && bpmAbs < 20;

	let level: HarmonicCompat['level'];
	if (goodKey && (goodBpm || bpmAbs === null)) {
		level = 'good';
	} else if (okayKey || goodBpm || (okayBpm && !hasKeys)) {
		level = 'okay';
	} else if (okayBpm && hasKeys) {
		level = 'okay';
	} else {
		level = 'clash';
	}

	return { level, bpmDelta, keyLabel };
}
