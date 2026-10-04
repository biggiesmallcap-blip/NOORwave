import { createPersistedStore, oneOf } from './persisted';

export type SurfaceMode = 'dark' | 'light' | 'system';

// Reuse the existing key so dark and light choices survive the new system option.
export const surfaceMode = createPersistedStore<SurfaceMode>('noor-theme', 'light', {
	parse: oneOf(['dark', 'light', 'system'] as const),
});

export function resolveSurfaceMode(mode: SurfaceMode, systemPrefersLight: boolean): 'dark' | 'light' {
	return mode === 'system' ? (systemPrefersLight ? 'light' : 'dark') : mode;
}
