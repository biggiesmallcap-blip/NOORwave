import { derived, writable } from 'svelte/store';
import type { TidalSearchVideo, TidalVideoMixItem } from '$lib/api/client';

export type VideoSessionItem = TidalSearchVideo | TidalVideoMixItem;
export type VideoSessionSource = 'none' | 'direct' | 'search' | 'mix';

export interface VideoSessionState {
	active: boolean;
	current: VideoSessionItem | null;
	queue: VideoSessionItem[];
	currentIndex: number;
	source: VideoSessionSource;
	sourceLabel: string | null;
	autoplay: boolean;
	loading: boolean;
	error: string | null;
}

interface VideoSessionSyncInput {
	current: VideoSessionItem | null;
	queue: VideoSessionItem[];
	source: VideoSessionSource;
	sourceLabel: string | null;
	autoplay: boolean;
	loading: boolean;
	error: string | null;
}

const initialState: VideoSessionState = {
	active: false,
	current: null,
	queue: [],
	currentIndex: -1,
	source: 'none',
	sourceLabel: null,
	autoplay: false,
	loading: false,
	error: null,
};

function findCurrentIndex(queue: VideoSessionItem[], current: VideoSessionItem | null): number {
	if (!current) return -1;
	return queue.findIndex((item) => item.tidal_id === current.tidal_id);
}

const session = writable<VideoSessionState>(initialState);

export const videoSession = {
	subscribe: session.subscribe,
	sync(input: VideoSessionSyncInput) {
		const currentIndex = findCurrentIndex(input.queue, input.current);
		session.set({
			active: input.current !== null,
			current: input.current,
			queue: input.queue,
			currentIndex,
			source: input.source,
			sourceLabel: input.sourceLabel,
			autoplay: input.autoplay,
			loading: input.loading,
			error: input.error,
		});
	},
	reset() {
		session.set(initialState);
	},
};

export const videoSessionUpcoming = derived(session, ($session) => {
	if ($session.currentIndex < 0) return $session.queue;
	return $session.queue.slice($session.currentIndex + 1);
});

export const videoJumpRequest = writable<{ videoId: number; nonce: number } | null>(null);
export const videoAutoplayToggleRequest = writable(0);

let jumpNonce = 0;

export function requestVideoJump(videoId: number) {
	videoJumpRequest.set({ videoId, nonce: ++jumpNonce });
}

export function requestVideoAutoplayToggle() {
	videoAutoplayToggleRequest.update((nonce) => nonce + 1);
}
