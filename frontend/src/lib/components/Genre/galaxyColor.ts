// Small colour helpers for the galaxy canvas. Pure functions, no DOM.

export type Rgb255 = [number, number, number];

export function hexToRgb255(hex: string): Rgb255 | null {
	const normalized = hex.replace('#', '');
	if (!/^[0-9a-fA-F]{6}$/.test(normalized)) return null;
	return [
		Number.parseInt(normalized.slice(0, 2), 16),
		Number.parseInt(normalized.slice(2, 4), 16),
		Number.parseInt(normalized.slice(4, 6), 16)
	];
}

export function rgb255ToHex([red, green, blue]: Rgb255): string {
	const part = (value: number) =>
		Math.round(Math.max(0, Math.min(255, value)))
			.toString(16)
			.padStart(2, '0');
	return `#${part(red)}${part(green)}${part(blue)}`;
}

/** [hue 0..360, saturation 0..1, lightness 0..1] */
export function rgbToHsl([red, green, blue]: Rgb255): [number, number, number] {
	const r = red / 255;
	const g = green / 255;
	const b = blue / 255;
	const max = Math.max(r, g, b);
	const min = Math.min(r, g, b);
	const lightness = (max + min) / 2;
	if (max === min) return [0, 0, lightness];
	const delta = max - min;
	const saturation = lightness > 0.5 ? delta / (2 - max - min) : delta / (max + min);
	let hue: number;
	if (max === r) hue = (g - b) / delta + (g < b ? 6 : 0);
	else if (max === g) hue = (b - r) / delta + 2;
	else hue = (r - g) / delta + 4;
	return [hue * 60, saturation, lightness];
}

export function hslToRgb([hue, saturation, lightness]: [number, number, number]): Rgb255 {
	const h = (((hue % 360) + 360) % 360) / 360;
	if (saturation === 0) {
		const value = lightness * 255;
		return [value, value, value];
	}
	const q = lightness < 0.5 ? lightness * (1 + saturation) : lightness + saturation - lightness * saturation;
	const p = 2 * lightness - q;
	const channel = (t: number) => {
		let x = t;
		if (x < 0) x += 1;
		if (x > 1) x -= 1;
		if (x < 1 / 6) return p + (q - p) * 6 * x;
		if (x < 1 / 2) return q;
		if (x < 2 / 3) return p + (q - p) * (2 / 3 - x) * 6;
		return p;
	};
	return [channel(h + 1 / 3) * 255, channel(h) * 255, channel(h - 1 / 3) * 255];
}

/** Max hue drift (degrees) of a sub-genre away from its family color. */
export const HUE_SPREAD = 10;

/**
 * Sub-genres take a small, stable step away from their family color so a
 * cluster reads as related-but-distinct instead of N identical copies. Deeper
 * levels get slightly lighter. Roots (depth 0) keep the family color exactly.
 */
export function varyFamilyColor(hex: string, seed: number, depth: number): string {
	const rgb = hexToRgb255(hex);
	if (!rgb || depth <= 0) return hex;
	const [hue, saturation, lightness] = rgbToHsl(rgb);
	const unit = (((seed * 2654435761) >>> 0) % 1000) / 999; // 0..1, stable per seed
	const nextHue = hue + (unit * 2 - 1) * HUE_SPREAD;
	const nextLightness = Math.min(0.86, lightness + depth * 0.035 + (unit - 0.5) * 0.06);
	return rgb255ToHex(hslToRgb([nextHue, saturation, nextLightness]));
}
