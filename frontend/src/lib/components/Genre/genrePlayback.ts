import type { Track } from '$lib/api/client';

// The core of the galaxy: play YOUR LOCAL tracks for a genre, shuffled. A
// bounded random sample keeps the queue sane on huge genres; shuffle mode
// keeps it fresh each launch. This is local library playback, not radio.
export const MAX_GENRE_QUEUE = 300;

export function shuffled<T>(items: T[]): T[] {
	const copy = items.slice();
	for (let i = copy.length - 1; i > 0; i -= 1) {
		const j = Math.floor(Math.random() * (i + 1));
		[copy[i], copy[j]] = [copy[j], copy[i]];
	}
	return copy;
}

export function randomItem<T>(items: T[]): T | undefined {
	if (items.length === 0) return undefined;
	return items[Math.floor(Math.random() * items.length)];
}

export function sampleGenreQueue(tracks: Track[]): number[] {
	return shuffled(tracks.filter((track) => track.id > 0).map((track) => track.id)).slice(0, MAX_GENRE_QUEUE);
}

/** A random library track to seed the optional Radio station from. */
export function pickSeedTrackId(tracks: Track[]): number | null {
	return randomItem(tracks.filter((track) => track.id > 0))?.id ?? null;
}
