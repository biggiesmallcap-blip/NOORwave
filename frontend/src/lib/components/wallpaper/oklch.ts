// OKLCH <-> sRGB. Theme tints and light-mode accents move lightness and chroma
// while holding hue, which OKLCH keeps perceptually steady and sRGB does not.

export interface Oklch {
	l: number;
	c: number;
	h: number;
}

function toLinear(v: number): number {
	const c = v / 255;
	return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
}

function fromLinear(v: number): number {
	const c = v <= 0.0031308 ? v * 12.92 : 1.055 * v ** (1 / 2.4) - 0.055;
	return c * 255;
}

export function hexToOklch(hex: string): Oklch {
	const [r, g, b] = [1, 3, 5].map((i) => toLinear(parseInt(hex.slice(i, i + 2), 16)));
	const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
	const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
	const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
	const L = 0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s;
	const A = 1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s;
	const B = 0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s;
	const h = (Math.atan2(B, A) * 180) / Math.PI;
	return { l: L, c: Math.hypot(A, B), h: h < 0 ? h + 360 : h };
}

function oklchToLinear({ l, c, h }: Oklch): [number, number, number] {
	const rad = (h * Math.PI) / 180;
	const A = c * Math.cos(rad);
	const B = c * Math.sin(rad);
	const l_ = (l + 0.3963377774 * A + 0.2158037573 * B) ** 3;
	const m_ = (l - 0.1055613458 * A - 0.0638541728 * B) ** 3;
	const s_ = (l - 0.0894841775 * A - 1.291485548 * B) ** 3;
	return [
		4.0767416621 * l_ - 3.3077115913 * m_ + 0.2309699292 * s_,
		-1.2684380046 * l_ + 2.6097574011 * m_ - 0.3413193965 * s_,
		-0.0041960863 * l_ - 0.7034186147 * m_ + 1.707614701 * s_,
	];
}

function inGamut(rgb: [number, number, number]): boolean {
	return rgb.every((v) => v >= -1e-4 && v <= 1 + 1e-4);
}

/** sRGB channels 0..255. Out-of-gamut colours keep lightness and hue and lose chroma. */
export function oklchToRgb(color: Oklch): [number, number, number] {
	let linear = oklchToLinear(color);
	if (!inGamut(linear)) {
		let lo = 0;
		let hi = color.c;
		for (let i = 0; i < 24; i++) {
			const mid = (lo + hi) / 2;
			if (inGamut(oklchToLinear({ ...color, c: mid }))) lo = mid;
			else hi = mid;
		}
		linear = oklchToLinear({ ...color, c: lo });
	}
	return linear.map((v) => Math.round(Math.min(255, Math.max(0, fromLinear(Math.min(1, Math.max(0, v))))))) as [
		number,
		number,
		number,
	];
}

export function oklchToHex(color: Oklch): string {
	return '#' + oklchToRgb(color).map((v) => v.toString(16).padStart(2, '0')).join('');
}
