import { get } from 'svelte/store';
import { isPlaying } from '$lib/stores/player';

// Keep the phone screen awake while music is playing so the remote stays usable
// without tapping every 30s. The OS releases the lock when the tab is hidden,
// so re-acquire on visibilitychange whenever we're still meant to be playing.

interface WakeLockSentinelLike {
	released: boolean;
	release: () => Promise<void>;
	addEventListener: (event: 'release', listener: () => void) => void;
}

interface WakeLockNavigator {
	wakeLock: {
		request: (type: 'screen') => Promise<WakeLockSentinelLike>;
	};
}

function getWakeLockApi(): WakeLockNavigator['wakeLock'] | null {
	if (typeof navigator === 'undefined') return null;
	const nav = navigator as unknown as Partial<WakeLockNavigator>;
	return nav.wakeLock ?? null;
}

export function installWakeLock(): () => void {
	const apiOrNull = getWakeLockApi();
	if (!apiOrNull) return () => {};
	const api = apiOrNull;

	let sentinel: WakeLockSentinelLike | null = null;
	let disposed = false;

	async function acquire() {
		if (disposed) return;
		if (sentinel && !sentinel.released) return;
		try {
			const next = await api.request('screen');
			if (disposed) {
				void next.release().catch(() => {});
				return;
			}
			sentinel = next;
			sentinel.addEventListener('release', () => {
				sentinel = null;
			});
		} catch {
			sentinel = null;
		}
	}

	async function release() {
		const current = sentinel;
		sentinel = null;
		if (!current || current.released) return;
		try {
			await current.release();
		} catch {
			// Best-effort; sentinel is already detached.
		}
	}

	const unsubPlay = isPlaying.subscribe((playing) => {
		if (playing) void acquire();
		else void release();
	});

	function onVisibility() {
		if (document.visibilityState === 'visible' && get(isPlaying)) {
			void acquire();
		}
	}

	document.addEventListener('visibilitychange', onVisibility);

	return () => {
		disposed = true;
		document.removeEventListener('visibilitychange', onVisibility);
		unsubPlay();
		void release();
	};
}
