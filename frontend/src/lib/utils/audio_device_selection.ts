import type { AudioDevice } from '$lib/api/types';

/** Resolve saved display names only when they identify one endpoint. */
export function audioDeviceSelectionValue(
	pref: string | null,
	devices: Pick<AudioDevice, 'id' | 'name'>[],
): string {
	if (pref === null || pref === 'default') return '__default__';
	if (devices.some((device) => device.id === pref)) return pref;
	const matches = devices.filter((device) => device.name === pref);
	return matches.length === 1 ? matches[0].id : pref;
}

export type AudioDeviceSelectionStatus = 'default' | 'available' | 'ambiguous' | 'missing';

/** How a saved preference maps onto the devices connected right now. */
export function audioDeviceSelectionStatus(
	pref: string | null,
	devices: Pick<AudioDevice, 'id' | 'name'>[],
): AudioDeviceSelectionStatus {
	if (pref === null || pref === 'default') return 'default';
	if (devices.some((device) => device.id === pref)) return 'available';
	const matches = devices.filter((device) => device.name === pref).length;
	if (matches === 1) return 'available';
	return matches > 1 ? 'ambiguous' : 'missing';
}

/** Saved endpoint ids are opaque GUIDs; only a legacy saved name reads well. */
export function savedOutputLabel(pref: string): string {
	return pref.includes('{') ? 'Saved output' : pref;
}
