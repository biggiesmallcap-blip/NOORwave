import { createPersistedStore, oneOf } from './persisted';

export type PlayerPlacement = 'right' | 'left' | 'bottom';
export type EffectivePlayerLayout = PlayerPlacement | 'mobile';

export const playerPlacement = createPersistedStore<PlayerPlacement>('noor-player-placement', 'right', {
	parse: oneOf(['right', 'left', 'bottom'] as const),
});

/** Widths are CSS pixels after browser or webview zoom. */
export function resolvePlayerLayout(preferred: PlayerPlacement, width: number): EffectivePlayerLayout {
	if (width < 680) return 'mobile';
	if (width < 1240) return 'bottom';
	return preferred;
}
