import { createPersistedStore } from '$lib/stores/persisted';

// How videos look as they change, per device: the curtain between videos,
// and the song title card (the lower third) when one starts, each off, on
// channels only, or on every video. Channel timing (joining where the guide
// says, or from the start) is the tune mode in lib/video/onair/tune_mode.ts;
// the Viewing menu under the player shows all three together.

/** Where a viewing touch applies: nowhere, on channels only, or everywhere. */
export type ViewingScope = 'off' | 'onair' | 'all';

export const VIEWING_SCOPES: readonly { value: ViewingScope; label: string }[] = [
	{ value: 'off', label: 'Off' },
	{ value: 'onair', label: 'On Air' },
	{ value: 'all', label: 'All videos' },
];

const parseScope = (raw: string): ViewingScope | undefined =>
	raw === 'off' || raw === 'onair' || raw === 'all' ? raw : undefined;

/** Whether a scope covers a video, on a channel or not. */
export function scopeCovers(scope: ViewingScope, onAir: boolean): boolean {
	return scope === 'all' || (scope === 'onair' && onAir);
}

export const curtainScope = createPersistedStore<ViewingScope>('noor.video.curtain.v2', 'all', { parse: parseScope });

export const videoCredits = createPersistedStore<ViewingScope>('noor.video.credits.v1', 'onair', { parse: parseScope });
