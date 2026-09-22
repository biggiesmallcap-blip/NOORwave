import { createPersistedStore } from './persisted';

const STORAGE_KEY = 'noor-horizontal-shelf-wheel';

/**
 * Opt-in compatibility mode for people browsing with a conventional mouse
 * wheel. Native mode leaves vertical gestures to the page and lets horizontal
 * trackpad gestures reach the shelf unchanged.
 */
export const horizontalShelfWheel = createPersistedStore<boolean>(STORAGE_KEY, false, {
	parse: (raw) => raw === 'true' ? true : raw === 'false' ? false : undefined,
});
