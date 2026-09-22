import { describe, expect, it } from 'vitest';
import { retainObservedStartupState, startupPresentation, type DesktopStartupState } from './startup';

const installed: DesktopStartupState = { supported: true, enabled: false, launch_mode: 'normal', unavailable_reason: null };

describe('startup setting presentation', () => {
	it('uses actual returned state and explains portable builds', () => {
		expect(startupPresentation({ ...installed, supported: false, unavailable_reason: 'PORTABLE_BUILD' })).toEqual({
			disabled: true, checked: false, message: 'Install NOORwave to enable start-at-sign-in'
		});
		expect(startupPresentation({ ...installed, enabled: true }).checked).toBe(true);
	});

	it('retains the observed state from a failed transition', () => {
		const observed = { ...installed, enabled: true };
		expect(retainObservedStartupState(installed, { code: 'AUTOSTART_DISABLE_FAILED', state: observed })).toBe(observed);
	});
});
