import { get } from 'svelte/store';
import type { Track } from '$lib/api/client';
import {
	currentTrack,
	isPlaying,
	pausePlayer,
	playNextTrack,
	playPreviousTrack,
	position,
	resumePlayer,
	setPlayerPosition
} from '$lib/stores/player';
import { upscaleTidalArtwork } from '$lib/utils/artwork';

// Wires the player stores into the browser's MediaSession API so the OS shows
// the right lockscreen / notification controls and routes hardware media keys
// (Bluetooth play/pause, headphone buttons) back to NOOR. Lockscreen art is the
// real reason this exists: without metadata, iOS draws a generic "Web Browser"
// card that gives no clue what's playing.

// TIDAL only serves a fixed set of square cover sizes (80, 160, 320, 640, 750,
// 1080, 1280). Asking for anything else 404s and the lockscreen draws no art.
// Hand the OS two valid sizes so it can pick the right one per surface.
const TIDAL_ARTWORK_SIZES = [320, 640] as const;

function metadataFromTrack(track: Track | null): MediaMetadata | null {
	if (!track) return null;
	const raw = track.artwork_url ?? null;
	const artwork: MediaImage[] = raw
		? TIDAL_ARTWORK_SIZES.flatMap((size) => {
				const src = upscaleTidalArtwork(raw, size);
				return src ? [{ src, sizes: `${size}x${size}`, type: 'image/jpeg' }] : [];
			})
		: [];
	return new MediaMetadata({
		title: track.title ?? '',
		artist: track.artist_name ?? '',
		album: track.album_title ?? '',
		artwork
	});
}

export function installMediaSessionBridge(): () => void {
	if (typeof navigator === 'undefined' || !('mediaSession' in navigator)) {
		return () => {};
	}

	const ms = navigator.mediaSession;
	const actions: MediaSessionAction[] = ['play', 'pause', 'nexttrack', 'previoustrack', 'seekto'];

	const safeSet = (action: MediaSessionAction, handler: MediaSessionActionHandler | null) => {
		try {
			ms.setActionHandler(action, handler);
		} catch {
			// Older browsers reject unknown actions; nothing we can do.
		}
	};

	// MediaSession dispatches `play` and `pause` as explicit commands from the
	// OS / headset / lockscreen. Calling `togglePlayback` here would let stale
	// WS state flip a pause press into a resume — see the codex review note.
	safeSet('play', () => void resumePlayer());
	safeSet('pause', () => void pausePlayer());
	safeSet('nexttrack', () => void playNextTrack());
	safeSet('previoustrack', () => void playPreviousTrack());
	safeSet('seekto', (event) => {
		const details = event as MediaSessionActionDetails;
		if (typeof details.seekTime === 'number') {
			void setPlayerPosition(Math.round(details.seekTime * 1000));
		}
	});

	const unsubTrack = currentTrack.subscribe((track) => {
		ms.metadata = metadataFromTrack(track);
	});

	const unsubPlay = isPlaying.subscribe((playing) => {
		ms.playbackState = playing ? 'playing' : 'paused';
	});

	const unsubPos = position.subscribe((pos) => {
		const track = get(currentTrack);
		if (!track?.duration_ms || typeof ms.setPositionState !== 'function') return;
		try {
			ms.setPositionState({
				duration: track.duration_ms / 1000,
				position: Math.min(pos, track.duration_ms) / 1000,
				playbackRate: 1.0
			});
		} catch {
			// setPositionState throws if position > duration after a rapid track swap.
		}
	});

	return () => {
		unsubTrack();
		unsubPlay();
		unsubPos();
		ms.metadata = null;
		ms.playbackState = 'none';
		for (const action of actions) safeSet(action, null);
	};
}
