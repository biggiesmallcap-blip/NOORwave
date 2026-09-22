export interface DesktopStartupState {
	supported: boolean;
	enabled: boolean;
	launch_mode: 'normal' | 'autostart';
	unavailable_reason: 'PORTABLE_BUILD' | 'UNSUPPORTED_PLATFORM' | null;
}

export function startupPresentation(state: DesktopStartupState | null, busy = false) {
	if (!state) return { disabled: true, checked: false, message: 'Checking start-at-sign-in…' };
	if (!state.supported) return {
		disabled: true, checked: false,
		message: state.unavailable_reason === 'PORTABLE_BUILD'
			? 'Install NOORwave to enable start-at-sign-in'
			: 'Start-at-sign-in is unavailable on this platform.'
	};
	return { disabled: busy, checked: state.enabled, message: state.enabled ? 'NOORwave starts hidden in the tray.' : 'NOORwave will not start when you sign in.' };
}

export function retainObservedStartupState(previous: DesktopStartupState, error: unknown): DesktopStartupState {
	if (error && typeof error === 'object' && 'state' in error) return (error as { state: DesktopStartupState }).state;
	return previous;
}
