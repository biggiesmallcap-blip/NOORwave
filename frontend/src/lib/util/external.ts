import { isTauri as tauriIsTauri } from '@tauri-apps/api/core';
import { openUrl } from '@tauri-apps/plugin-opener';

export { isTauri } from '@tauri-apps/api/core';

export type OpenExternalMethod = 'tauri' | 'browser' | 'none';

export type OpenExternalResult =
	| { ok: true; method: OpenExternalMethod }
	| { ok: false; method: OpenExternalMethod; error: string };

function errorMessage(error: unknown): string {
	return error instanceof Error ? error.message : String(error);
}

export async function openExternal(url: string): Promise<OpenExternalResult> {
	if (!url) return { ok: false, method: 'none', error: 'No URL provided.' };

	if (tauriIsTauri()) {
		try {
			await openUrl(url);
			return { ok: true, method: 'tauri' };
		} catch (error) {
			const message = errorMessage(error);
			console.warn('opener.openUrl failed', error);
			return { ok: false, method: 'tauri', error: message };
		}
	}

	const w = typeof window === 'undefined' ? null : window;
	if (!w) return { ok: false, method: 'none', error: 'No browser window is available.' };

	const opened = w.open(url, '_blank');
	if (!opened) {
		return {
			ok: false,
			method: 'browser',
			error: 'Browser blocked the sign-in window.',
		};
	}

	opened.opener = null;
	return { ok: true, method: 'browser' };
}
