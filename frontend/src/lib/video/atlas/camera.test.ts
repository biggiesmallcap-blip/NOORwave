import { describe, expect, test } from 'vitest';
import {
	constrainCamera,
	fitCamera,
	fitScale,
	panBy,
	planFlight,
	screenToWorld,
	worldToScreen,
	zoomAround,
	type Camera,
} from './camera';

const vp = { width: 800, height: 600 };

describe('atlas camera', () => {
	test('screen and world round-trip', () => {
		const cam: Camera = { cx: 500, cy: 400, scale: 2.5 };
		const w = screenToWorld(cam, vp, 123, 456);
		const s = worldToScreen(cam, vp, w.x, w.y);
		expect(s.x).toBeCloseTo(123, 9);
		expect(s.y).toBeCloseTo(456, 9);
		// The camera centre sits at the viewport centre.
		const c = worldToScreen(cam, vp, 500, 400);
		expect(c).toEqual({ x: 400, y: 300 });
	});

	test('zooming keeps the world point under the cursor fixed', () => {
		const cam: Camera = { cx: 500, cy: 500, scale: 1 };
		const before = screenToWorld(cam, vp, 650, 120);
		const next = zoomAround(cam, vp, 650, 120, 3, 0.1, 100);
		expect(next.scale).toBe(3);
		const after = screenToWorld(next, vp, 650, 120);
		expect(after.x).toBeCloseTo(before.x, 9);
		expect(after.y).toBeCloseTo(before.y, 9);
	});

	test('zoom is clamped and still anchored', () => {
		const cam: Camera = { cx: 500, cy: 500, scale: 1 };
		const next = zoomAround(cam, vp, 100, 100, 1000, 0.5, 8);
		expect(next.scale).toBe(8);
		const anchor = screenToWorld(cam, vp, 100, 100);
		const after = screenToWorld(next, vp, 100, 100);
		expect(after.x).toBeCloseTo(anchor.x, 9);
		expect(after.y).toBeCloseTo(anchor.y, 9);
	});

	test('zoomAround may write into the camera it reads', () => {
		const cam: Camera = { cx: 500, cy: 500, scale: 1 };
		const anchor = screenToWorld(cam, vp, 10, 590);
		zoomAround(cam, vp, 10, 590, 2, 0.1, 10, cam);
		const after = screenToWorld(cam, vp, 10, 590);
		expect(after.x).toBeCloseTo(anchor.x, 9);
		expect(after.y).toBeCloseTo(anchor.y, 9);
	});

	test('dragging right moves the map right', () => {
		const cam: Camera = { cx: 500, cy: 500, scale: 2 };
		const next = panBy(cam, 40, -20);
		expect(next.cx).toBe(480);
		expect(next.cy).toBe(510);
	});

	test('fit frames the bounds with padding', () => {
		const bounds = { minX: 0, minY: 0, maxX: 1000, maxY: 1000 };
		expect(fitScale(bounds, vp, 50)).toBeCloseTo(0.5, 9);
		const cam = fitCamera(bounds, vp, 50);
		expect(cam).toEqual({ cx: 500, cy: 500, scale: 0.5 });
	});

	test('a fling cannot lose the map', () => {
		const bounds = { minX: 0, minY: 0, maxX: 1000, maxY: 1000 };
		const cam = constrainCamera({ cx: 99999, cy: -99999, scale: 1 }, bounds, vp);
		expect(cam.cx).toBeLessThanOrEqual(1000 + 400);
		expect(cam.cy).toBeGreaterThanOrEqual(-300);
	});

	test('flights start and end exactly at their cameras', () => {
		const from: Camera = { cx: 100, cy: 100, scale: 0.8 };
		const to: Camera = { cx: 900, cy: 700, scale: 6 };
		const flight = planFlight(from, to, vp);
		const out: Camera = { cx: 0, cy: 0, scale: 0 };
		flight.at(0, out);
		expect(out.cx).toBeCloseTo(100, 6);
		expect(out.cy).toBeCloseTo(100, 6);
		expect(out.scale).toBeCloseTo(0.8, 6);
		flight.at(1, out);
		expect(out).toEqual(to);
		expect(flight.duration).toBeGreaterThanOrEqual(420);
		expect(flight.duration).toBeLessThanOrEqual(1700);
		// A long flight at one zoom pulls out in the middle (van Wijk).
		const across = planFlight({ cx: 0, cy: 0, scale: 4 }, { cx: 1000, cy: 0, scale: 4 }, vp);
		across.at(0.5, out);
		expect(out.scale).toBeLessThan(4);
		expect(out.cx).toBeCloseTo(500, 6);
	});

	test('a zoom-only flight keeps the centre', () => {
		const flight = planFlight({ cx: 300, cy: 300, scale: 1 }, { cx: 300, cy: 300, scale: 4 }, vp);
		const out: Camera = { cx: 0, cy: 0, scale: 0 };
		flight.at(0.5, out);
		expect(out.cx).toBe(300);
		expect(out.scale).toBeGreaterThan(1);
		expect(out.scale).toBeLessThan(4);
	});
});
