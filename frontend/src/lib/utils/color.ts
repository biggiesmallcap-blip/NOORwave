/**
 * Hash a string into one of seven brand-friendly colours, used as a fallback
 * background for missing album / artist / playlist artwork.
 *
 * The palette is deliberately small and bright so adjacent fallbacks read as
 * a colour-coded set rather than a noisy gradient. Determinism (same name →
 * same colour) keeps the UI calm across renders.
 *
 *   letterColor('Pink Floyd')    → '#9b5de5'
 *   letterColor('Daft Punk')     → '#457b9d'
 *
 * Note: routes/search/+page.svelte intentionally uses a different muted-HSL
 * variant for its results pane. That divergence is documented in STYLING.md
 * and not consolidated here.
 */
const LETTER_COLORS = [
	'#e63946',
	'#457b9d',
	'#2a9d8f',
	'#e9c46a',
	'#f4a261',
	'#9b5de5',
	'#00b4d8',
] as const;

export function letterColor(seed: string): string {
	let h = 0;
	for (let i = 0; i < seed.length; i++) {
		h = (h * 31 + seed.charCodeAt(i)) & 0xffffffff;
	}
	return LETTER_COLORS[Math.abs(h) % LETTER_COLORS.length];
}
