interface TauriWindow extends Window {
	__TAURI_INTERNALS__?: { invoke?: unknown };
}

/** Inside the desktop app, video fullscreen is a native window switch
 *  (noor-app's set_video_fullscreen) instead of the browser Fullscreen API:
 *  Tauri's own fullscreen showed the desktop and the old title bar through
 *  the window for a frame or two while it resized. */
export function hasNativeVideoFullscreen(): boolean {
	if (typeof window === 'undefined') return false;
	return Boolean((window as TauriWindow).__TAURI_INTERNALS__?.invoke);
}

/** Resolves once the window has switched; false if it could not. */
export async function setNativeVideoFullscreen(on: boolean): Promise<boolean> {
	if (!hasNativeVideoFullscreen()) return false;
	try {
		const { invoke } = await import('@tauri-apps/api/core');
		await invoke('set_video_fullscreen', { on });
		return true;
	} catch (err) {
		console.warn('set_video_fullscreen failed', err);
		return false;
	}
}
