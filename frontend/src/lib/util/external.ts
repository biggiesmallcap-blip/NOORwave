import { openUrl } from '@tauri-apps/plugin-opener';

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
	const w = typeof window === 'undefined' ? null : window;
	if (isTauri()) {
		void openUrl(url).catch((err) => {
			console.warn('opener.openUrl failed', err);
		});
		return;
	}
	if (w) w.open(url, '_blank', 'noopener');
}
