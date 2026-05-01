import { browser } from '$app/environment';

export function getCmdOrCtrlLabel(): '⌘' | 'Ctrl' {
	if (!browser) return 'Ctrl';
	const platform =
		(navigator as Navigator & { userAgentData?: { platform?: string } }).userAgentData?.platform
		?? navigator.platform
		?? '';
	return platform.toLowerCase().includes('mac') ? '⌘' : 'Ctrl';
}
