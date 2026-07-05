import type { AudioSearchParams } from '$lib/api/client';
import type { FilterValue, ParsedQuery } from '$lib/search/query_parser';

// Exact numeric tokens are matched with a tolerance window instead of strict
// equality: DSP values are floats (bpm 137.98, energy 0.712), so bpm:138 as
// min=max=138.0 would match nothing. bpm uses round-to-nearest semantics,
// energy/danceability a small absolute window.
const BPM_EXACT_TOLERANCE = 0.5;
const UNIT_EXACT_TOLERANCE = 0.05;

function filterValues(filter: FilterValue): string[] {
	if (filter.type === 'multi') return filter.values;
	if (filter.type === 'exact') return [filter.value];
	return [];
}

function applyNumeric(
	params: AudioSearchParams,
	filter: FilterValue | undefined,
	minKey: 'bpm_min' | 'energy_min' | 'danceability_min',
	maxKey: 'bpm_max' | 'energy_max' | 'danceability_max',
	exactTolerance: number
) {
	if (!filter) return;
	if (filter.type === 'range') {
		params[minKey] = filter.min;
		params[maxKey] = filter.max;
	} else if (filter.type === 'comparison') {
		if (filter.op === '>' || filter.op === '>=') params[minKey] = filter.value;
		if (filter.op === '<' || filter.op === '<=') params[maxKey] = filter.value;
	} else if (filter.type === 'exact') {
		const value = parseFloat(filter.value);
		if (!isNaN(value)) {
			params[minKey] = value - exactTolerance;
			params[maxKey] = value + exactTolerance;
		}
	}
}

export function buildAudioParams(pq: ParsedQuery): AudioSearchParams {
	const f = pq.filters;
	const params: AudioSearchParams = {};
	if (pq.free_text) params.free_text = pq.free_text;

	applyNumeric(params, f['bpm'], 'bpm_min', 'bpm_max', BPM_EXACT_TOLERANCE);
	applyNumeric(params, f['energy'], 'energy_min', 'energy_max', UNIT_EXACT_TOLERANCE);
	applyNumeric(
		params,
		f['danceability'],
		'danceability_min',
		'danceability_max',
		UNIT_EXACT_TOLERANCE
	);

	const key = f['key'];
	if (key?.type === 'exact') params.key_signature = key.value;

	const camelot = f['camelot'];
	if (camelot?.type === 'exact') params.camelot_key = camelot.value;

	const year = f['year'];
	if (year?.type === 'range') {
		params.year_min = year.min;
		params.year_max = year.max;
	}
	if (year?.type === 'exact') {
		params.year_min = parseInt(year.value);
		params.year_max = parseInt(year.value);
	}

	// vocal:false === instrumental:true; both spellings are supported because
	// the library hint bar has always advertised instrumental:true.
	const instrumental = f['instrumental'];
	if (instrumental?.type === 'exact') params.is_instrumental = instrumental.value === 'true';
	const vocal = f['vocal'];
	if (vocal?.type === 'exact') params.is_instrumental = vocal.value === 'false';

	// Genre tokens go to the server raw; it resolves slug/name
	// case-insensitively and expands descendants. An unresolvable token yields
	// zero results plus unmatched_genres in the response - never a silent
	// unfiltered search.
	const genre = f['genre'];
	if (genre) {
		const slugs = filterValues(genre);
		if (slugs.length > 0) params.genre_slugs = slugs;
	}

	const artist = f['artist'];
	if (artist?.type === 'exact') params.artist_contains = artist.value;

	const album = f['album'];
	if (album?.type === 'exact') params.album_contains = album.value;

	return params;
}

export function hasAnyFilter(pq: ParsedQuery): boolean {
	return Object.keys(pq.filters).length > 0;
}
