import { writable } from 'svelte/store';
import { currentTrack } from '$lib/stores/player';
import { wallpaperColorSource, type WallpaperColorSource } from '$lib/stores/wallpaper';
import { upscaleTidalArtwork } from '$lib/utils/artwork';

// Four [r,g,b] colours (each channel 0..1), ordered dark -> bright, extracted
// from the playing track's cover art. `null` whenever there's no art or the
// extraction failed (e.g. the CDN tainted the canvas), so consumers fall back
// to the fixed palette. Feeds the "Album art" wallpaper colour source.
export type ArtRgb = [number, number, number];
export type ArtPalette = [ArtRgb, ArtRgb, ArtRgb, ArtRgb];

export const artPalette = writable<ArtPalette | null>(null);

const SAMPLE = 28; // downscale the cover to this many px per side before sampling

let currentUrl: string | null = null;
let source: WallpaperColorSource = 'palette';
let seq = 0;

function luminance(c: ArtRgb): number {
	return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
}

// Tiny k-means (k=4) over the sampled pixels. Cheap: ~784 points, a handful of
// iterations. Seeded by spreading initial centroids across the sorted-by-luma
// samples so we don't collapse to one cluster on low-contrast covers.
function quantize(pixels: ArtRgb[]): ArtPalette | null {
	if (pixels.length < 4) return null;
	const sorted = [...pixels].sort((a, b) => luminance(a) - luminance(b));
	let centroids: ArtRgb[] = [0, 1, 2, 3].map(
		(i) => sorted[Math.floor(((i + 0.5) / 4) * sorted.length)]
	);

	for (let iter = 0; iter < 8; iter++) {
		const sums: number[][] = [
			[0, 0, 0, 0],
			[0, 0, 0, 0],
			[0, 0, 0, 0],
			[0, 0, 0, 0]
		];
		for (const p of pixels) {
			let best = 0;
			let bestD = Infinity;
			for (let k = 0; k < 4; k++) {
				const c = centroids[k];
				const d = (p[0] - c[0]) ** 2 + (p[1] - c[1]) ** 2 + (p[2] - c[2]) ** 2;
				if (d < bestD) {
					bestD = d;
					best = k;
				}
			}
			sums[best][0] += p[0];
			sums[best][1] += p[1];
			sums[best][2] += p[2];
			sums[best][3] += 1;
		}
		for (let k = 0; k < 4; k++) {
			if (sums[k][3] > 0) {
				centroids[k] = [
					sums[k][0] / sums[k][3],
					sums[k][1] / sums[k][3],
					sums[k][2] / sums[k][3]
				];
			}
		}
	}

	centroids.sort((a, b) => luminance(a) - luminance(b));
	// Lift the brightest accent so dark covers still yield a visible highlight.
	const accent = centroids[3];
	const accentLum = luminance(accent);
	if (accentLum < 0.35 && accentLum > 0) {
		const boost = 0.35 / accentLum;
		centroids[3] = [
			Math.min(1, accent[0] * boost),
			Math.min(1, accent[1] * boost),
			Math.min(1, accent[2] * boost)
		];
	}
	return centroids as ArtPalette;
}

function extract(url: string): void {
	if (typeof document === 'undefined') return;
	const mySeq = ++seq;
	const img = new Image();
	img.crossOrigin = 'anonymous';
	img.decoding = 'async';
	img.onload = () => {
		if (mySeq !== seq) return; // a newer track superseded us
		try {
			const canvas = document.createElement('canvas');
			canvas.width = SAMPLE;
			canvas.height = SAMPLE;
			const ctx = canvas.getContext('2d', { willReadFrequently: true });
			if (!ctx) return;
			ctx.drawImage(img, 0, 0, SAMPLE, SAMPLE);
			const data = ctx.getImageData(0, 0, SAMPLE, SAMPLE).data; // throws if tainted
			const pixels: ArtRgb[] = [];
			for (let i = 0; i < data.length; i += 4) {
				if (data[i + 3] < 128) continue; // skip transparent
				pixels.push([data[i] / 255, data[i + 1] / 255, data[i + 2] / 255]);
			}
			const pal = quantize(pixels);
			if (mySeq === seq) artPalette.set(pal);
		} catch {
			// Tainted canvas (CDN blocked cross-origin reads) or any decode issue:
			// leave consumers on the fixed palette.
			if (mySeq === seq) artPalette.set(null);
		}
	};
	img.onerror = () => {
		if (mySeq === seq) artPalette.set(null);
	};
	// A small cover is plenty for a 28px sample and loads fast.
	img.src = upscaleTidalArtwork(url, 160) ?? url;
}

// Only extract while "Album art" is the chosen colour source, so we don't hit
// the artwork CDN or decode a canvas for users who stay on the fixed palette.
function refresh(): void {
	if (source !== 'art') return;
	if (!currentUrl) {
		seq++;
		artPalette.set(null);
		return;
	}
	extract(currentUrl);
}

if (typeof window !== 'undefined') {
	currentTrack.subscribe((track) => {
		const url = track?.artwork_url ?? null;
		if (url === currentUrl) return;
		currentUrl = url;
		refresh();
	});
	wallpaperColorSource.subscribe((v) => {
		const wasArt = source === 'art';
		source = v;
		if (source === 'art' && !wasArt) refresh();
	});
}
