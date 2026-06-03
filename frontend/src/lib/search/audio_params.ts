import type { AudioSearchParams, Genre } from '$lib/api/client';
import type { FilterValue, ParsedQuery } from '$lib/search/query_parser';

function resolveGenreIds(filter: FilterValue, genres: Genre[]): number[] {
	const slugs =
		filter.type === 'multi'
			? filter.values
			: filter.type === 'exact'
			? [filter.value]
			: [];
	const found: number[] = [];
	for (const slug of slugs) {
		const match = genres.find(
			(g) => g.slug === slug || g.name.toLowerCase() === slug.toLowerCase()
		);
		if (match) {
			found.push(match.id);
			for (const child of match.children ?? []) found.push(child.id);
		}
	}
	return found;
}

export function buildAudioParams(pq: ParsedQuery, genres: Genre[]): AudioSearchParams {
	const f = pq.filters;
	const params: AudioSearchParams = {};
	if (pq.free_text) params.free_text = pq.free_text;

	const bpm = f['bpm'];
	if (bpm?.type === 'range') {
		params.bpm_min = bpm.min;
		params.bpm_max = bpm.max;
	}
	if (bpm?.type === 'comparison') {
		if (bpm.op === '>' || bpm.op === '>=') params.bpm_min = bpm.value;
		if (bpm.op === '<' || bpm.op === '<=') params.bpm_max = bpm.value;
	}

	const energy = f['energy'];
	if (energy?.type === 'range') {
		params.energy_min = energy.min;
		params.energy_max = energy.max;
	}
	if (energy?.type === 'comparison') {
		if (energy.op === '>' || energy.op === '>=') params.energy_min = energy.value;
		if (energy.op === '<' || energy.op === '<=') params.energy_max = energy.value;
	}

	const dance = f['danceability'];
	if (dance?.type === 'range') {
		params.danceability_min = dance.min;
		params.danceability_max = dance.max;
	}
	if (dance?.type === 'comparison') {
		if (dance.op === '>' || dance.op === '>=') params.danceability_min = dance.value;
		if (dance.op === '<' || dance.op === '<=') params.danceability_max = dance.value;
	}

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

	const vocal = f['vocal'];
	if (vocal?.type === 'exact') params.is_instrumental = vocal.value === 'false';

	const genreFilter = f['genre'];
	if (genreFilter) {
		const ids = resolveGenreIds(genreFilter, genres);
		if (ids.length > 0) params.genre_ids = ids;
	}

	return params;
}

export function hasAnyFilter(pq: ParsedQuery): boolean {
	return Object.keys(pq.filters).length > 0;
}
