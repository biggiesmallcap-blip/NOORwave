export const TIDAL_PKCE_RELOGIN_DISMISSED_KEY = 'noor-tidal-pkce-relogin-dismissed';

const TIDAL_REDIRECT_ORIGIN = 'https://tidal.com';
const TIDAL_REDIRECT_PATH = '/android/login/auth';

export interface ClipboardRedirectResult {
	ok: boolean;
	redirectUrl?: string;
	error?: string;
}

export interface TidalNoticeStatus {
	connected?: boolean;
	auth_flow?: string | null;
}

export interface TidalNoticeDismissal {
	dismissedForever: boolean;
	dismissedThisSession: boolean;
}

export function isValidTidalRedirectUrl(value: string): boolean {
	const trimmed = value.trim();
	if (!trimmed) return false;
	try {
		const url = new URL(trimmed);
		return (
			url.origin === TIDAL_REDIRECT_ORIGIN &&
			url.pathname === TIDAL_REDIRECT_PATH &&
			Boolean(url.searchParams.get('code')?.trim())
		);
	} catch {
		return false;
	}
}

export async function readTidalRedirectFromClipboard(
	readText: () => Promise<string> = () => navigator.clipboard.readText()
): Promise<ClipboardRedirectResult> {
	try {
		const text = (await readText()).trim();
		if (!isValidTidalRedirectUrl(text)) {
			return {
				ok: false,
				error: 'Clipboard does not contain the final TIDAL redirect URL.',
			};
		}
		return { ok: true, redirectUrl: text };
	} catch {
		return {
			ok: false,
			error: 'Clipboard access failed. Paste the URL manually.',
		};
	}
}

export function shouldShowLegacyReloginNotice(
	status: TidalNoticeStatus,
	dismissal: TidalNoticeDismissal
): boolean {
	return Boolean(
		status.connected &&
			status.auth_flow !== 'pkce' &&
			!dismissal.dismissedForever &&
			!dismissal.dismissedThisSession
	);
}
