interface TauriWindow extends Window {
	__TAURI_INTERNALS__?: { invoke?: unknown };
}

function hasTauriInternals(): boolean {
	if (typeof window === 'undefined') return false;
	return Boolean((window as TauriWindow).__TAURI_INTERNALS__?.invoke);
}

export async function setWebviewZoom(factor: number): Promise<void> {
	if (!hasTauriInternals()) return;
	try {
		const { getCurrentWebview } = await import('@tauri-apps/api/webview');
		await getCurrentWebview().setZoom(factor);
	} catch (err) {
		console.warn('set_webview_zoom failed', err);
	}
}
