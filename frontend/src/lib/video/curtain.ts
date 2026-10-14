import { get, writable } from 'svelte/store';
import { reduceMotion } from '$lib/stores/motion';

// The curtain between videos: from the moment a video change starts until
// the new one is really playing, the picture is the new video's own still
// (softened, a little dark, with film grain), so the black frame and the
// loading spinner in between never show; then it dissolves into the video.
// The player raises it on every change of video; a channel flip raises it
// earlier, the moment the flip starts, with the target channel's still.

export interface CurtainState {
	/** The incoming video's artwork, when known. */
	art: string | null;
	/** The video on screen when it went up: it lifts only once another one
	 *  has arrived and moved forward. */
	fromId: number | null;
	/** Changes on every raise, so a raise resets the curtain's own timer. */
	seq: number;
}

export const videoCurtain = writable<CurtainState | null>(null);

let seq = 0;

export function raiseVideoCurtain(art: string | null, fromId: number | null): void {
	if (get(reduceMotion)) return;
	seq += 1;
	videoCurtain.set({ art, fromId, seq });
}

export function lowerVideoCurtain(): void {
	videoCurtain.set(null);
}
