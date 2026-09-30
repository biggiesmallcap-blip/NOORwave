import { createPersistedStore, oneOf } from './persisted';

export type QualityDisplay = 'off' | 'icon' | 'details' | 'both';

const parseQualityDisplay = oneOf(['off', 'icon', 'details', 'both'] as const);

export const sideQualityDisplay = createPersistedStore<QualityDisplay>('noor-side-quality-display', 'details', {
	parse: parseQualityDisplay,
});

export const bottomQualityDisplay = createPersistedStore<QualityDisplay>('noor-bottom-quality-display', 'icon', {
	parse: parseQualityDisplay,
});
