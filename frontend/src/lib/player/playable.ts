import { get } from 'svelte/store';
import type { QueueItem, TidalPlayable, Track } from '$lib/api/client';
import { tidalStatus } from '$lib/stores/tidal';

export type UnavailableReason = 'missing-source' | 'tidal-only-not-authed';

export type PlayableTrack =
	| { kind: 'library'; track: Track; track_id: number }
	| { kind: 'tidal'; track: TidalPlayable; tidal_id: number }
	| { kind: 'pending-lastfm'; artist: string; title: string; reason?: string | null }
	| {
			kind: 'unavailable';
			reason: UnavailableReason;
			track?: Track | TidalPlayable;
			label?: string;
	  };

function hasKind(input: unknown): input is PlayableTrack {
	return (
		typeof input === 'object' &&
		input !== null &&
		'kind' in input &&
		typeof (input as { kind?: unknown }).kind === 'string'
	);
}

function isQueueItem(input: Track | TidalPlayable | QueueItem): input is QueueItem {
	return 'track' in input && 'position' in input;
}

function isTrack(input: Track | TidalPlayable): input is Track {
	return 'id' in input;
}

export function fromTidalPlayable(track: TidalPlayable): PlayableTrack {
	if (track.tidal_id > 0) {
		return { kind: 'tidal', track, tidal_id: track.tidal_id };
	}
	return { kind: 'unavailable', reason: 'missing-source', track, label: track.title };
}

export function fromQueueItem(item: QueueItem): PlayableTrack {
	if (item.is_pending) {
		return {
			kind: 'pending-lastfm',
			artist: item.track.artist_name ?? '',
			title: item.track.title,
			reason: item.reason
		};
	}
	return toPlayableTrack(item.track);
}

export function toPlayableTrack(input: Track | TidalPlayable | QueueItem | PlayableTrack): PlayableTrack {
	if (hasKind(input)) return input;
	if (isQueueItem(input)) return fromQueueItem(input);
	if (isTrack(input)) {
		if (input.id > 0) {
			return { kind: 'library', track: input, track_id: input.id };
		}
		if ((input.tidal_id ?? 0) > 0) {
			return fromTidalPlayable({
				tidal_id: input.tidal_id ?? 0,
				title: input.title,
				artist_name: input.artist_name,
				album_title: input.album_title,
				artwork_url: input.artwork_url,
				duration_ms: input.duration_ms,
				artist_tidal_id: input.artist_tidal_id ?? null
			});
		}
		return { kind: 'unavailable', reason: 'missing-source', track: input, label: input.title };
	}
	return fromTidalPlayable(input);
}

export function canPlayTrack(input: Track | TidalPlayable | QueueItem | PlayableTrack): boolean {
	const playable = toPlayableTrack(input);
	if (playable.kind === 'library') return true;
	if (playable.kind === 'tidal') return get(tidalStatus) === 'connected';
	return false;
}

export function getPlayableLabel(input: Track | TidalPlayable | QueueItem | PlayableTrack): string {
	const playable = toPlayableTrack(input);
	switch (playable.kind) {
		case 'library':
			return 'Play';
		case 'tidal':
			return get(tidalStatus) === 'connected' ? 'Play from TIDAL' : 'Sign in to TIDAL to play';
		case 'pending-lastfm':
			return 'Resolving on TIDAL...';
		case 'unavailable':
			return playable.reason === 'tidal-only-not-authed' ? 'Sign in to TIDAL to play' : 'Unavailable';
	}
}
