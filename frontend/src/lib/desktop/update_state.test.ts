import { describe, expect, test } from 'vitest';
import {
	browserUpdateState,
	loadingDesktopUpdateState,
	unavailableDesktopUpdateState
} from './update_state';

describe('desktop update state', () => {
	test('keeps browser update checks disabled outside Tauri', () => {
		expect(browserUpdateState('0.2.2')).toEqual({
			appVersion: '0.2.2',
			installModeLabel: 'Browser',
			updateStatus: 'Available in the desktop app',
			updateAvailableVersion: null,
			updateError: ''
		});
	});

	test('does not label a detected Tauri shell as Browser while loading', () => {
		expect(loadingDesktopUpdateState('0.2.2')).toMatchObject({
			appVersion: '0.2.2',
			installModeLabel: 'Checking shell',
			updateStatus: 'Checking update channel',
			updateError: ''
		});
	});

	test('preserves a desktop-shell label when Tauri update commands fail', () => {
		const state = unavailableDesktopUpdateState('0.2.2', new Error('updater unavailable'));

		expect(state).toMatchObject({
			appVersion: '0.2.2',
			installModeLabel: 'Desktop shell',
			updateStatus: 'Update status unavailable',
			updateAvailableVersion: null,
			updateError: 'updater unavailable'
		});
	});
});
