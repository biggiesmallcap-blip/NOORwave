import { afterEach, describe, expect, test, vi } from 'vitest';
import { cornerForKey, loadCorner, miniSize, nearestCorner, placeMini, saveCorner, PILL_SIZE } from './mini_dock';

// The workspace: right of a 200 px sidebar, left of a 340 px side panel, above
// a 100 px bottom bar on a 1600 x 1000 window.
const workspace = { left: 200, top: 0, right: 1260, bottom: 900 };

afterEach(() => vi.unstubAllGlobals());

describe('mini video window placement', () => {
	test('stays inside the workspace in every corner', () => {
		const size = miniSize(1600, false);
		expect(size).toEqual({ width: 340, height: 191 });
		expect(placeMini(workspace, 'br', size)).toEqual({ left: 1260 - 16 - 340, top: 900 - 16 - 191 });
		expect(placeMini(workspace, 'tl', size)).toEqual({ left: 216, top: 16 });
		const br = placeMini(workspace, 'br', size);
		expect(br.left + size.width).toBeLessThanOrEqual(workspace.right);
		expect(br.top + size.height).toBeLessThanOrEqual(workspace.bottom);
	});

	test('a narrow workspace pins the window to its left and top', () => {
		const tight = { left: 200, top: 0, right: 400, bottom: 120 };
		expect(placeMini(tight, 'br', miniSize(1600, false))).toEqual({ left: 216, top: 16 });
	});

	test('the minimised pill uses its own size', () => {
		expect(miniSize(1600, true)).toEqual(PILL_SIZE);
		expect(miniSize(800, false).width).toBe(248);
	});

	test('a drag snaps to the corner of the quadrant it ends in', () => {
		expect(nearestCorner(workspace, 300, 100)).toBe('tl');
		expect(nearestCorner(workspace, 1200, 100)).toBe('tr');
		expect(nearestCorner(workspace, 300, 800)).toBe('bl');
		expect(nearestCorner(workspace, 1200, 800)).toBe('br');
	});

	test('arrow keys move between corners', () => {
		expect(cornerForKey('br', 'ArrowLeft')).toBe('bl');
		expect(cornerForKey('bl', 'ArrowUp')).toBe('tl');
		expect(cornerForKey('tl', 'ArrowRight')).toBe('tr');
		expect(cornerForKey('tr', 'ArrowDown')).toBe('br');
		expect(cornerForKey('tr', 'Enter')).toBe('tr');
	});

	test('the corner is remembered, and blocked storage falls back to bottom right', () => {
		const store = new Map<string, string>();
		vi.stubGlobal('localStorage', {
			getItem: (key: string) => store.get(key) ?? null,
			setItem: (key: string, value: string) => void store.set(key, value),
		});
		expect(loadCorner()).toBe('br');
		saveCorner('tl');
		expect(loadCorner()).toBe('tl');
		vi.stubGlobal('localStorage', {
			getItem: () => { throw new Error('blocked'); },
			setItem: () => { throw new Error('QuotaExceededError'); },
		});
		expect(() => saveCorner('bl')).not.toThrow();
		expect(loadCorner()).toBe('br');
	});
});
