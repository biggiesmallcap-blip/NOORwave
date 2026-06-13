import { goto } from '$app/navigation';

// True once the user has navigated at least once *inside* the app this
// session. Until then, a detail page may have been cold-opened (deep link,
// reload, first paint) and there is no in-app entry to pop back to.
let hasInAppHistory = false;

/**
 * Wired from the root layout's `onNavigate`. `hasFrom` is true whenever the
 * navigation we're starting has a previous page, i.e. the history stack has a
 * real in-app entry behind it.
 */
export function markNavigated(hasFrom: boolean) {
	if (hasFrom) hasInAppHistory = true;
}

/**
 * Go back to exactly where the user was, via the WebView/browser history
 * stack. Falls back to `fallback` only when there's no in-app history to pop
 * (cold deep-link or reload landing straight on a detail page).
 */
export function goBack(fallback = '/library') {
	if (hasInAppHistory && typeof history !== 'undefined' && history.length > 1) {
		history.back();
	} else {
		void goto(fallback);
	}
}
