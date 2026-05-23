import { describe, it, expect } from 'vitest';
import { parseReason, formatReasonForScreenReader } from './reason';

describe('parseReason', () => {
	it('returns null for null or empty input', () => {
		expect(parseReason(null)).toBeNull();
		expect(parseReason(undefined)).toBeNull();
		expect(parseReason('')).toBeNull();
		expect(parseReason('   ')).toBeNull();
	});

	it('returns just the prefix when no JSON suffix is present', () => {
		expect(parseReason('Liked artist Massive Attack')).toEqual({
			prefix: 'Liked artist Massive Attack',
		});
	});

	it('parses prefix + json into a breakdown', () => {
		expect(
			parseReason('Genre match | {"genre_jaccard":0.42,"affinity_mult":1.1}')
		).toEqual({
			prefix: 'Genre match',
			genre_jaccard: 0.42,
			affinity_mult: 1.1,
		});
	});

	it('treats malformed JSON as a prefix-only reason', () => {
		const r = parseReason('Genre match | {bad json');
		expect(r?.prefix).toBe('Genre match | {bad json');
		expect(r?.genre_jaccard).toBeUndefined();
	});
});

describe('formatReasonForScreenReader', () => {
	it('returns null when there is nothing to read', () => {
		expect(formatReasonForScreenReader(null)).toBeNull();
		expect(formatReasonForScreenReader('')).toBeNull();
	});

	it('reads the prefix when no metrics are present', () => {
		expect(formatReasonForScreenReader('Liked artist Massive Attack')).toBe(
			'Liked artist Massive Attack'
		);
	});

	it('appends formatted genre overlap and affinity to the prefix', () => {
		expect(
			formatReasonForScreenReader(
				'Genre match | {"genre_jaccard":0.42,"affinity_mult":1.08}'
			)
		).toBe('Genre match. Genre overlap 42%. Affinity +8%');
	});

	it('skips affinity when it rounds to zero', () => {
		expect(
			formatReasonForScreenReader('Library | {"genre_jaccard":0.67,"affinity_mult":1.001}')
		).toBe('Library. Genre overlap 67%');
	});

	it('handles negative affinity', () => {
		expect(
			formatReasonForScreenReader('Recent skip | {"affinity_mult":0.7}')
		).toBe('Recent skip. Affinity -30%');
	});
});
