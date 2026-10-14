// The artist card's opening lines, in words: how much there is, what it is
// tagged, and how well you know the artist. Plain sentences instead of a
// stats row, tag chips and a coloured light badge, so nothing depends on
// telling colours apart. Pure functions.

import type { Light } from '$lib/api/video_explore_types';

/** "40 minutes", "1.8 hours", "1 hour", "12 hours". */
export function hoursWords(hours: number): string {
	if (!Number.isFinite(hours) || hours <= 0) return '';
	if (hours < 1) {
		const minutes = Math.max(1, Math.round(hours * 60));
		return `${minutes} ${minutes === 1 ? 'minute' : 'minutes'}`;
	}
	const shown = hours < 10 ? hours.toFixed(1).replace(/\.0$/, '') : Math.round(hours).toLocaleString();
	return `${shown} ${shown === '1' ? 'hour' : 'hours'}`;
}

/** "a", "a and b", "a, b and c": the first `max` items as a sentence. */
export function listWords(items: readonly string[], max = 3): string {
	const list = items.map((item) => item.trim()).filter(Boolean).slice(0, max);
	if (list.length <= 1) return list[0] ?? '';
	return `${list.slice(0, -1).join(', ')} and ${list[list.length - 1]}`;
}

/** "house", "house and pop", "house, pop and electronic" (the first three). */
export function genreWords(genres: readonly string[]): string {
	return listWords(genres, 3);
}

/** How much of a scene you have seen, as a sentence. */
export function sceneSeenLine(seenVideos: number, litArtists: number): string {
	if (seenVideos <= 0) return 'You have not watched any of it yet.';
	const from = litArtists > 0 ? `, from ${litArtists.toLocaleString()} of its artists` : '';
	return `You have seen ${seenVideos.toLocaleString()} of its videos${from}.`;
}

/** How well you know the artist, as a sentence (none for uncharted: the
 *  card says that in its own words). */
export const KNOWN_LINE: Record<Light, string> = {
	0: '',
	1: 'New to you.',
	2: 'In your library, or one step from it.',
	3: 'You have watched, liked or saved them.',
};
