import { videoSession } from '$lib/stores/video_session';
import { dispatchVideoControl, type VideoControl } from '$lib/video/controls';
import { hasNativeVideoFullscreen } from './video_fullscreen';

/** Listen before handing native media keys to the video session, including in the tray. */
export function installVideoMediaKeys(): () => void {
	if (!hasNativeVideoFullscreen()) return () => {};
	let disposed = false;
	let cleanup = () => {};
	void (async () => {
		const [{ invoke }, { getCurrentWebviewWindow }] = await Promise.all([
			import('@tauri-apps/api/core'), import('@tauri-apps/api/webviewWindow'),
		]);
		if (disposed) return;
		const unlisten = await getCurrentWebviewWindow().listen<VideoControl>('video-media-key', ({ payload }) => {
			if (!disposed) dispatchVideoControl(payload);
		});
		if (disposed) { unlisten(); return; }
		let pending = Promise.resolve();
		const setActive = (active: boolean) => {
			pending = pending.then(() => invoke<void>('set_video_media_active', { active }))
				.catch((error) => { console.warn('Video media key routing failed', error); });
		};
		let lastActive: boolean | undefined;
		const unsubscribe = videoSession.subscribe(({ active }) => {
			if (active === lastActive) return;
			lastActive = active;
			setActive(active);
		});
		cleanup = () => { unsubscribe(); setActive(false); unlisten(); };
	})().catch((error) => { console.warn('Video media key listener failed', error); });
	return () => { disposed = true; cleanup(); };
}
