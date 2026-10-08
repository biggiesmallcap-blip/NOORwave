// Plain-language error copy (audit "States"). Pages used to print the raw
// exception ("Failed to load artist: ApiError: API error: 500"). ErrorState
// shows this sentence instead and keeps the raw text behind "Details".

export interface ErrorDescription {
	/** What happened, in words a listener can act on. */
	message: string;
	/** The raw error text, for the Details disclosure; null when empty. */
	detail: string | null;
}

function statusOf(error: unknown): number | null {
	if (error && typeof error === 'object' && 'status' in error) {
		const status = (error as { status: unknown }).status;
		if (typeof status === 'number') return status;
	}
	const text = String(error ?? '');
	const match = text.match(/API error:?\s*(\d{3})/i) ?? text.match(/\b(4\d\d|5\d\d)\b/);
	return match ? Number(match[1]) : null;
}

export function describeError(error: unknown): ErrorDescription {
	const raw = error instanceof Error ? `${error.name}: ${error.message}` : String(error ?? '').trim();
	const detail = raw || null;
	const status = statusOf(error);
	const text = raw.toLowerCase();
	let message: string;
	if (status === 404) message = 'It may have been removed or moved.';
	else if (status === 401 || status === 403) message = 'NOORwave is not allowed to load this right now. Check the service connection in Settings.';
	else if (status === 503) message = 'The music service is not reachable right now. Try again in a moment.';
	else if (status != null && status >= 500) message = "NOORwave's server hit a problem loading this. Try again.";
	else if (text.includes('failed to fetch') || text.includes('networkerror') || text.includes('network error'))
		message = "Can't reach the NOORwave server. Check that it is running, then try again.";
	else message = 'Something went wrong loading this. Try again.';
	return { message, detail };
}
