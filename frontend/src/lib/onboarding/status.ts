export const ONBOARDING_COMPLETE_KEY = 'noor.onboarding.complete';

function scopedKey(scope?: string | null): string {
	return scope ? `${ONBOARDING_COMPLETE_KEY}.${encodeURIComponent(scope)}` : ONBOARDING_COMPLETE_KEY;
}

export function hasLocalOnboardingComplete(scope?: string | null): boolean {
	if (typeof localStorage === 'undefined') return false;
	return localStorage.getItem(scopedKey(scope)) === '1';
}

export function markLocalOnboardingComplete(scope?: string | null): void {
	if (typeof localStorage === 'undefined') return;
	localStorage.setItem(scopedKey(scope), '1');
}

export function clearLocalOnboardingComplete(scope?: string | null): void {
	if (typeof localStorage === 'undefined') return;
	localStorage.removeItem(scopedKey(scope));
}
