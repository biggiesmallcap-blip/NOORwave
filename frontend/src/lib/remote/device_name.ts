// Default name a phone sends when it pairs, so Settings > Phone remote lists
// "iPhone - Home Screen" instead of a column of identical "Phone remote" rows.
// Best effort from the user agent; the listener can rename it on the computer.

export interface DeviceNameHints {
	userAgent: string;
	maxTouchPoints?: number;
	standalone?: boolean;
}

function platform(ua: string, touchPoints: number): string | null {
	if (/iPhone/.test(ua)) return 'iPhone';
	if (/iPad/.test(ua)) return 'iPad';
	// iPadOS 13+ reports a desktop Mac user agent; touch support gives it away.
	if (/Macintosh/.test(ua) && touchPoints > 1) return 'iPad';
	if (/Android/.test(ua)) return /Mobile/.test(ua) ? 'Android phone' : 'Android tablet';
	if (/Macintosh|Mac OS X/.test(ua)) return 'Mac';
	if (/Windows/.test(ua)) return 'Windows PC';
	if (/CrOS/.test(ua)) return 'Chromebook';
	if (/Linux/.test(ua)) return 'Linux';
	return null;
}

function browser(ua: string): string | null {
	if (/EdgA?\//.test(ua)) return 'Edge';
	if (/SamsungBrowser\//.test(ua)) return 'Samsung Internet';
	if (/OPR\/|Opera/.test(ua)) return 'Opera';
	if (/FxiOS\/|Firefox\//.test(ua)) return 'Firefox';
	if (/CriOS\/|Chrome\//.test(ua)) return 'Chrome';
	if (/Safari\//.test(ua)) return 'Safari';
	return null;
}

export function suggestDeviceName({ userAgent, maxTouchPoints = 0, standalone = false }: DeviceNameHints): string {
	const device = platform(userAgent, maxTouchPoints) ?? 'Phone remote';
	const via = standalone ? 'Home Screen' : browser(userAgent);
	return via ? `${device} - ${via}` : device;
}

export function currentDeviceName(): string | undefined {
	if (typeof navigator === 'undefined') return undefined;
	const standalone = typeof window !== 'undefined'
		&& (window.matchMedia?.('(display-mode: standalone)').matches
			|| (navigator as Navigator & { standalone?: boolean }).standalone === true);
	return suggestDeviceName({ userAgent: navigator.userAgent, maxTouchPoints: navigator.maxTouchPoints, standalone });
}
