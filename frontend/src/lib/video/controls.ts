import { get } from 'svelte/store';
import { nextVideo, previousVideo, videoSession } from '$lib/stores/video_session';

export type VideoControl = 'next' | 'previous' | 'toggle' | 'fullscreen' | 'mute'
	| 'seek-forward' | 'seek-back' | 'volume-up' | 'volume-down' | 'queue';

let playerControl: ((action: VideoControl) => boolean) | null = null;

/** The persistent player owns element controls; queue navigation stays available while loading. */
export function registerVideoControls(handler: (action: VideoControl) => boolean): () => void {
	playerControl = handler;
	return () => { if (playerControl === handler) playerControl = null; };
}

export function dispatchVideoControl(action: VideoControl): boolean {
	if (!get(videoSession).active) return false;
	if (action === 'next') void nextVideo();
	else if (action === 'previous') void previousVideo();
	else return playerControl?.(action) ?? true;
	// An empty video queue must never fall through to the previous music queue.
	return true;
}

export function handleVideoShortcut(event: KeyboardEvent): boolean {
	if (event.defaultPrevented || event.ctrlKey || event.metaKey || event.altKey) return false;
	let action: VideoControl | undefined;
	switch (event.key.toLowerCase()) {
		case 'mediatracknext': action = 'next'; break;
		case 'mediatrackprevious': action = 'previous'; break;
		case 'mediaplaypause': case ' ': case 'spacebar': action = 'toggle'; break;
		case 'arrowright': action = event.shiftKey ? 'next' : 'seek-forward'; break;
		case 'arrowleft': action = event.shiftKey ? 'previous' : 'seek-back'; break;
		case 'arrowup': action = 'volume-up'; break;
		case 'arrowdown': action = 'volume-down'; break;
		case 'f': action = 'fullscreen'; break;
		case 'm': action = 'mute'; break;
		case 'q': action = 'queue'; break;
	}
	if (!action || !dispatchVideoControl(action)) return false;
	event.preventDefault();
	return true;
}
