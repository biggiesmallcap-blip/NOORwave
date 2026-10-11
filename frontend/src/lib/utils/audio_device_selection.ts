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
