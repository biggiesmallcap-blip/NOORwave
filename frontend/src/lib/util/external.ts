// Opens a URL in the system browser. In Tauri the WebView2 doesn't honor
// `<a target="_blank">` or `window.open()` for external URLs by default —
// we route through the `open_external` Tauri command (which calls the Rust
// `open` crate). Falls back to `window.open` for plain-browser usage so
// `npm run dev` still works.
//
// Tauri 2 injects `window.__TAURI_INTERNALS__.invoke`. Detect via that
// rather than adding an npm dep — keeps the bundle the same size.
type TauriInvoke = (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;

interface TauriWindow extends Window {
	__TAURI_INTERNALS__?: { invoke?: TauriInvoke };
}

export function isTauri(): boolean {
	if (typeof window === 'undefined') return false;
	return Boolean((window as TauriWindow).__TAURI_INTERNALS__?.invoke);
}

export function openExternal(url: string): void {
	if (!url) return;
	const w = typeof window === 'undefined' ? null : (window as TauriWindow);
	const invoke = w?.__TAURI_INTERNALS__?.invoke;
	if (invoke) {
		void invoke('open_external', { url }).catch((err) => {
			console.warn('open_external failed', err);
		});
		return;
	}
	if (w) w.open(url, '_blank', 'noopener');
}
