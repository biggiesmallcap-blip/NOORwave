import { clearStoredToken, getStoredToken, setStoredToken } from '$lib/api/client';
import { dataCache } from '$lib/cache/query';

/** Store the API token; switching to a different one drops data cached for the old one. */
export function setSessionToken(token: string): void {
	if (getStoredToken() !== token) dataCache.clear();
	setStoredToken(token);
}

export function clearSessionToken(): void {
	clearStoredToken();
	dataCache.clear();
}
