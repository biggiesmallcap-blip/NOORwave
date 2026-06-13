import { derived, get, writable } from 'svelte/store';
import { api } from '$lib/api/client';
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
	/** HLS stream URL for `current`. Lives in the store so the persistent dock
	 *  can keep playing across route changes without the route owning it. */
	streamUrl: string | null;
	streamExpiresAt: string | null;
	playing: boolean;
	/** Last reported playback position, kept so returning to /videos resumes
	 *  the picture in sync with the audio that never stopped. */
	positionMs: number;
}

/** Browse context the route hands to the controller when it starts a video:
 *  the queue to autoplay through and how it was sourced. */
export interface VideoPlayContext {
	queue: VideoSessionItem[];
	source: VideoSessionSource;
	sourceLabel: string | null;
	autoplay?: boolean;
}

export interface PreloadedVideoStream {
	url: string;
	expiresAt: string | null;
}

const AUTOPLAY_KEY = 'noor_video_autoplay_next';

function loadAutoplayPreference(): boolean {
	if (typeof localStorage === 'undefined') return false;
	return localStorage.getItem(AUTOPLAY_KEY) === 'true';
}

function persistAutoplayPreference(autoplay: boolean) {
	if (typeof localStorage === 'undefined') return;
	localStorage.setItem(AUTOPLAY_KEY, String(autoplay));
}

const initialState: VideoSessionState = {
	active: false,
	current: null,
	queue: [],
	currentIndex: -1,
	source: 'none',
	sourceLabel: null,
	autoplay: loadAutoplayPreference(),
	loading: false,
	error: null,
	streamUrl: null,
	streamExpiresAt: null,
	playing: false,
	positionMs: 0,
};

function findCurrentIndex(queue: VideoSessionItem[], current: VideoSessionItem | null): number {
	if (!current) return -1;
	return queue.findIndex((item) => item.tidal_id === current.tidal_id);
}

const session = writable<VideoSessionState>(initialState);

function update(patch: Partial<VideoSessionState>) {
	session.update((state) => {
		const next = { ...state, ...patch };
		next.currentIndex = findCurrentIndex(next.queue, next.current);
		next.active = next.current !== null;
		return next;
	});
}

export const videoSession = {
	subscribe: session.subscribe,
	/** Refresh the autoplay queue + source attribution without touching the
	 *  currently-playing video (called as the route's search results change). */
	setContext(ctx: { queue: VideoSessionItem[]; source: VideoSessionSource; sourceLabel: string | null }) {
		update({ queue: ctx.queue, source: ctx.source, sourceLabel: ctx.sourceLabel });
	},
	setAutoplay(autoplay: boolean) {
		persistAutoplayPreference(autoplay);
		update({ autoplay });
	},
	setPlaying(playing: boolean) {
		update({ playing });
	},
	setPosition(positionMs: number) {
		update({ positionMs });
	},
	reset() {
		session.set({ ...initialState, autoplay: loadAutoplayPreference() });
	},
};

export const videoSessionUpcoming = derived(session, ($session) => {
	if ($session.currentIndex < 0) return $session.queue;
	return $session.queue.slice($session.currentIndex + 1);
});

// ─── Controller: owns the stream lifecycle so playback survives navigation ───

let streamSeq = 0;

function sourceFor(item: VideoSessionItem, ctx: VideoPlayContext): VideoSessionSource {
	if (ctx.source !== 'none') return ctx.source;
	if ('mix_id' in item && item.mix_id != null) return 'mix';
	return 'direct';
}

/** Start (or switch to) a video: set it current, fetch its HLS stream, and let
 *  the persistent dock render it. Returns false if the request was superseded
 *  or the stream failed. */
export async function playVideo(
	item: VideoSessionItem,
	ctx: VideoPlayContext,
	opts: { preloaded?: PreloadedVideoStream | null } = {}
): Promise<boolean> {
	const seq = ++streamSeq;
	update({
		current: item,
		queue: ctx.queue,
		source: sourceFor(item, ctx),
		sourceLabel: ctx.sourceLabel,
		autoplay: ctx.autoplay ?? get(session).autoplay,
		loading: true,
		error: null,
		streamUrl: opts.preloaded?.url ?? null,
		streamExpiresAt: opts.preloaded?.expiresAt ?? null,
		positionMs: 0,
	});

	try {
		let url = opts.preloaded?.url ?? null;
		let expiresAt = opts.preloaded?.expiresAt ?? null;
		if (!url) {
			const stream = await api.getTidalVideoStream(item.tidal_id);
			if (seq !== streamSeq) return false;
			url = stream.hls_url;
			expiresAt = stream.expires_at;
		}
		if (seq !== streamSeq) return false;
		update({ streamUrl: url, streamExpiresAt: expiresAt, loading: false, error: null });
		return true;
	} catch (err) {
		if (seq !== streamSeq) return false;
		const message = err instanceof Error ? err.message : 'This video could not be loaded.';
		update({ loading: false, error: message });
		return false;
	}
}

/** Re-fetch the current video's stream (expiry / network recovery). */
export async function refreshVideoStream(): Promise<string> {
	const current = get(session).current;
	if (!current) throw new Error('No video selected.');
	const seq = ++streamSeq;
	const stream = await api.getTidalVideoStream(current.tidal_id);
	if (seq !== streamSeq) throw Object.assign(new Error('Stream request superseded.'), { name: 'StaleStreamRequest' });
	update({ streamUrl: stream.hls_url, streamExpiresAt: stream.expires_at });
	return stream.hls_url;
}

/** Advance to the next queued video when autoplay is on. Returns false at the
 *  end of the loaded queue (the route tops the queue up while it's mounted). */
export async function advanceVideo(opts: { preloaded?: PreloadedVideoStream | null } = {}): Promise<boolean> {
	const state = get(session);
	if (!state.autoplay) return false;
	const index = findCurrentIndex(state.queue, state.current);
	if (index < 0) return false;
	const next = state.queue[index + 1];
	if (!next) {
		update({ playing: false });
		return false;
	}
	return playVideo(next, {
		queue: state.queue,
		source: state.source,
		sourceLabel: state.sourceLabel,
		autoplay: true,
	}, opts);
}

/** Stop the video session entirely and free the dock. */
export function clearVideoSession() {
	streamSeq += 1;
	session.set({ ...initialState, autoplay: loadAutoplayPreference() });
}

// ─── Cross-component requests (dispatched from layout, served by the dock) ───

/** The /videos route's in-page placeholder. The persistent dock copies this
 *  element's rect each frame so the live player appears docked into the hero
 *  while actually being a fixed element that never unmounts on navigation. */
export const videoStageAnchor = writable<HTMLElement | null>(null);

export const videoJumpRequest = writable<{ videoId: number; nonce: number } | null>(null);
export const videoAutoplayToggleRequest = writable(0);
export const videoClearRequest = writable(0);

let jumpNonce = 0;

export function requestVideoJump(videoId: number) {
	videoJumpRequest.set({ videoId, nonce: ++jumpNonce });
}

export function requestVideoAutoplayToggle() {
	videoAutoplayToggleRequest.update((nonce) => nonce + 1);
}

export function requestVideoClear() {
	videoClearRequest.update((n) => n + 1);
}
