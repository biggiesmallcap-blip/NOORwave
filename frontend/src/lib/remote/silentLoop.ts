// iOS Safari only attaches the lockscreen "Now Playing" card to a tab when an
// actual <audio> or <video> element is playing media. NOORwave plays audio in
// the Rust backend, so the tab is silent and iOS ignores any MediaSession
// metadata we set. Looping a tiny silent WAV gives the tab a real media
// playback context for iOS to latch the metadata + transport actions onto.
//
// Pattern is the same one Plexamp web and Spotify Connect web use. The audio
// must be unlocked by a real user gesture before iOS will let it play.

export function installSilentMediaLoop(audio: HTMLAudioElement): () => void {
	if (typeof document === 'undefined') return () => {};

	let unlocked = false;

	async function unlock() {
		if (unlocked) return;
		try {
			await audio.play();
			unlocked = true;
			document.removeEventListener('pointerdown', unlock, true);
			document.removeEventListener('keydown', unlock, true);
		} catch {
			// Gesture wasn't real (programmatic event) or the element isn't ready.
			// Listener stays attached for the next tap.
		}
	}

	document.addEventListener('pointerdown', unlock, true);
	document.addEventListener('keydown', unlock, true);

	return () => {
		document.removeEventListener('pointerdown', unlock, true);
		document.removeEventListener('keydown', unlock, true);
		try {
			audio.pause();
		} catch {
			// Already torn down.
		}
	};
}
