export type DesktopUpdateState = {
	appVersion: string;
	installModeLabel: string;
	updateStatus: string;
	updateAvailableVersion: string | null;
	updateError: string;
};

export function browserUpdateState(appVersion: string): DesktopUpdateState {
	return {
		appVersion,
		installModeLabel: 'Browser',
		updateStatus: 'Available in the desktop app',
		updateAvailableVersion: null,
		updateError: ''
	};
}

export function loadingDesktopUpdateState(appVersion: string): DesktopUpdateState {
	return {
		appVersion,
		installModeLabel: 'Checking shell',
		updateStatus: 'Checking update channel',
		updateAvailableVersion: null,
		updateError: ''
	};
}

export function unavailableDesktopUpdateState(appVersion: string, error: unknown): DesktopUpdateState {
	return {
		appVersion,
		installModeLabel: 'Desktop shell',
		updateStatus: 'Update status unavailable',
		updateAvailableVersion: null,
		updateError: error instanceof Error ? error.message : String(error)
	};
}
