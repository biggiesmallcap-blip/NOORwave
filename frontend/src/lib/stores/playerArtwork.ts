import { createPersistedStore, oneOf } from './persisted';

export type PlayerArtworkStyle = 'square' | 'banner' | 'slim';

export const playerArtworkStyle = createPersistedStore<PlayerArtworkStyle>('noor-player-artwork-style', 'square', {
	parse: oneOf(['square', 'banner', 'slim'] as const),
});
