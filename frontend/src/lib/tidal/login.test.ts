import { describe, expect, test } from 'vitest';
import {
	isValidTidalRedirectUrl,
	readTidalRedirectFromClipboard,
	shouldShowLegacyReloginNotice,
} from './login';

describe('TIDAL PKCE login helpers', () => {
	test('accepts TIDAL Android redirect URLs with an auth code', () => {
		expect(
			isValidTidalRedirectUrl('https://tidal.com/android/login/auth?code=abc123&state=na')
		).toBe(true);
	});

	test('rejects blank and unrelated redirect URLs', () => {
		expect(isValidTidalRedirectUrl('')).toBe(false);
		expect(isValidTidalRedirectUrl('https://example.com/android/login/auth?code=abc123')).toBe(false);
		expect(isValidTidalRedirectUrl('https://tidal.com/android/login/auth?state=na')).toBe(false);
	});

	test('reads a valid redirect URL from the clipboard', async () => {
		const result = await readTidalRedirectFromClipboard(async () =>
			'https://tidal.com/android/login/auth?code=abc123&state=na'
		);

		expect(result).toEqual({
			ok: true,
			redirectUrl: 'https://tidal.com/android/login/auth?code=abc123&state=na',
		});
	});

	test('reports invalid clipboard text without returning it as a redirect', async () => {
		const result = await readTidalRedirectFromClipboard(async () => 'not a redirect');

		expect(result).toEqual({
			ok: false,
			error: 'Clipboard does not contain the final TIDAL redirect URL.',
		});
	});

	test('reports clipboard permission failures', async () => {
		const result = await readTidalRedirectFromClipboard(async () => {
			throw new Error('denied');
		});

		expect(result).toEqual({
			ok: false,
			error: 'Clipboard access failed. Paste the URL manually.',
		});
	});
});

describe('legacy PKCE re-login notice', () => {
	test('shows for connected legacy sessions', () => {
		expect(
			shouldShowLegacyReloginNotice(
				{ connected: true, auth_flow: 'legacy' },
				{ dismissedForever: false, dismissedThisSession: false }
			)
		).toBe(true);
	});

	test('hides for PKCE sessions and dismissals', () => {
		expect(
			shouldShowLegacyReloginNotice(
				{ connected: true, auth_flow: 'pkce' },
				{ dismissedForever: false, dismissedThisSession: false }
			)
		).toBe(false);
		expect(
			shouldShowLegacyReloginNotice(
				{ connected: true, auth_flow: 'legacy' },
				{ dismissedForever: false, dismissedThisSession: true }
			)
		).toBe(false);
		expect(
			shouldShowLegacyReloginNotice(
				{ connected: true, auth_flow: 'legacy' },
				{ dismissedForever: true, dismissedThisSession: false }
			)
		).toBe(false);
	});
});
