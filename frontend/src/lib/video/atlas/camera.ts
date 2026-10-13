// Camera math for the Atlas. The camera is the world point at the centre of
// the viewport plus a scale in CSS pixels per world unit. Every function here
// is pure; the ones called inside the frame loop write into an `out` object
// instead of allocating.

import type { AtlasBounds } from './decode';

export interface Camera {
	/** World point at the centre of the viewport. */
	cx: number;
	cy: number;
	/** CSS pixels per world unit. */
	scale: number;
}

export interface Viewport {
	width: number;
	height: number;
}

export interface Point {
	x: number;
	y: number;
}

export function worldToScreenX(camera: Camera, viewport: Viewport, wx: number): number {
	return (wx - camera.cx) * camera.scale + viewport.width / 2;
}

export function worldToScreenY(camera: Camera, viewport: Viewport, wy: number): number {
	return (wy - camera.cy) * camera.scale + viewport.height / 2;
}

export function worldToScreen(camera: Camera, viewport: Viewport, wx: number, wy: number, out: Point = { x: 0, y: 0 }): Point {
	out.x = worldToScreenX(camera, viewport, wx);
	out.y = worldToScreenY(camera, viewport, wy);
	return out;
}

export function screenToWorld(camera: Camera, viewport: Viewport, sx: number, sy: number, out: Point = { x: 0, y: 0 }): Point {
	out.x = camera.cx + (sx - viewport.width / 2) / camera.scale;
	out.y = camera.cy + (sy - viewport.height / 2) / camera.scale;
	return out;
}

export function copyCamera(from: Camera, out: Camera): Camera {
	out.cx = from.cx;
	out.cy = from.cy;
	out.scale = from.scale;
	return out;
}

export function clamp(value: number, min: number, max: number): number {
	return value < min ? min : value > max ? max : value;
}

/** Scale that fits `bounds` inside the viewport with `padding` CSS px on each side. */
export function fitScale(bounds: AtlasBounds, viewport: Viewport, padding = 24): number {
	const w = Math.max(1, bounds.maxX - bounds.minX);
	const h = Math.max(1, bounds.maxY - bounds.minY);
	const vw = Math.max(1, viewport.width - padding * 2);
	const vh = Math.max(1, viewport.height - padding * 2);
	return Math.min(vw / w, vh / h);
}

export function fitCamera(bounds: AtlasBounds, viewport: Viewport, padding = 24): Camera {
	return {
		cx: (bounds.minX + bounds.maxX) / 2,
		cy: (bounds.minY + bounds.maxY) / 2,
		scale: fitScale(bounds, viewport, padding),
	};
}

/**
 * Zoom by `factor` keeping the world point under screen (sx, sy) fixed, with
 * the scale clamped to [minScale, maxScale]. Writes into `out` (may be the
 * same object as `camera`).
 */
export function zoomAround(
	camera: Camera,
	viewport: Viewport,
	sx: number,
	sy: number,
	factor: number,
	minScale: number,
	maxScale: number,
	out: Camera = { cx: 0, cy: 0, scale: 0 }
): Camera {
	const wx = camera.cx + (sx - viewport.width / 2) / camera.scale;
	const wy = camera.cy + (sy - viewport.height / 2) / camera.scale;
	const scale = clamp(camera.scale * factor, minScale, maxScale);
	out.scale = scale;
	out.cx = wx - (sx - viewport.width / 2) / scale;
	out.cy = wy - (sy - viewport.height / 2) / scale;
	return out;
}

/** Pan by a screen-space delta (drag right moves the map right). */
export function panBy(camera: Camera, dx: number, dy: number, out: Camera = { cx: 0, cy: 0, scale: 0 }): Camera {
	out.scale = camera.scale;
	out.cx = camera.cx - dx / camera.scale;
	out.cy = camera.cy - dy / camera.scale;
	return out;
}

/**
 * Keep some of the map on screen: the camera centre may not leave the map's
 * bounds grown by half a viewport, so a fling can never lose the map.
 */
export function constrainCamera(camera: Camera, bounds: AtlasBounds, viewport: Viewport): Camera {
	const marginX = viewport.width / 2 / camera.scale;
	const marginY = viewport.height / 2 / camera.scale;
	const loX = bounds.minX - marginX * 0.6;
	const hiX = bounds.maxX + marginX * 0.6;
	const loY = bounds.minY - marginY * 0.6;
	const hiY = bounds.maxY + marginY * 0.6;
	camera.cx = loX > hiX ? (bounds.minX + bounds.maxX) / 2 : clamp(camera.cx, loX, hiX);
	camera.cy = loY > hiY ? (bounds.minY + bounds.maxY) / 2 : clamp(camera.cy, loY, hiY);
	return camera;
}

export function easeInOutCubic(t: number): number {
	return t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2;
}

export interface Flight {
	/** Milliseconds. */
	duration: number;
	/** Camera at progress t in [0, 1] (already eased), written into out. */
	at(t: number, out: Camera): Camera;
}

const RHO = Math.SQRT2;
const RHO2 = 2;
const RHO4 = 4;
const EPSILON2 = 1e-12;

/**
 * Smooth and efficient zooming and panning (van Wijk and Nuij 2003, as in
 * d3-interpolate's zoom): the camera pulls out while it travels far and
 * settles in at the destination, so a long flight reads as one gesture.
 * `w` is the viewport's smaller side in world units.
 */
export function planFlight(from: Camera, to: Camera, viewport: Viewport, speed = 1): Flight {
	const side = Math.max(1, Math.min(viewport.width, viewport.height));
	const ux0 = from.cx;
	const uy0 = from.cy;
	const w0 = side / from.scale;
	const ux1 = to.cx;
	const uy1 = to.cy;
	const w1 = side / to.scale;
	const dx = ux1 - ux0;
	const dy = uy1 - uy0;
	const d2 = dx * dx + dy * dy;
	let S: number;
	let at: (t: number, out: Camera) => Camera;

	if (d2 < EPSILON2) {
		S = Math.log(w1 / w0) / RHO;
		at = (t, out) => {
			out.cx = ux0 + t * dx;
			out.cy = uy0 + t * dy;
			out.scale = side / (w0 * Math.exp(RHO * t * S));
			return out;
		};
	} else {
		const d1 = Math.sqrt(d2);
		const b0 = (w1 * w1 - w0 * w0 + RHO4 * d2) / (2 * w0 * RHO2 * d1);
		const b1 = (w1 * w1 - w0 * w0 - RHO4 * d2) / (2 * w1 * RHO2 * d1);
		const r0 = Math.log(Math.sqrt(b0 * b0 + 1) - b0);
		const r1 = Math.log(Math.sqrt(b1 * b1 + 1) - b1);
		S = (r1 - r0) / RHO;
		const coshr0 = Math.cosh(r0);
		const sinhr0 = Math.sinh(r0);
		at = (t, out) => {
			if (t >= 1) {
				out.cx = ux1;
				out.cy = uy1;
				out.scale = to.scale;
				return out;
			}
			const s = t * S;
			const u = (w0 / (RHO2 * d1)) * (coshr0 * Math.tanh(RHO * s + r0) - sinhr0);
			out.cx = ux0 + u * dx;
			out.cy = uy0 + u * dy;
			out.scale = side / ((w0 * coshr0) / Math.cosh(RHO * s + r0));
			return out;
		};
	}
	const raw = Math.abs(S) * 1000 * 0.62 / speed;
	const duration = Number.isFinite(raw) ? clamp(raw, 420, 1700) : 600;
	return { duration, at };
}

/** Frame-rate independent exponential approach: fraction to move this frame. */
export function approach(dtMs: number, halfLifeMs: number): number {
	if (halfLifeMs <= 0) return 1;
	return 1 - Math.pow(0.5, dtMs / halfLifeMs);
}
