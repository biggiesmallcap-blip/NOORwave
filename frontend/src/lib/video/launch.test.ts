import { describe, expect, test } from 'vitest';
import { launchBox, noteVideoLaunch, takeVideoLaunch } from './launch';

const element = (rect: { top: number; left: number; width: number; height: number }) =>
	({ getBoundingClientRect: () => rect }) as unknown as Element;

describe('tune-in launch', () => {
	test('a picture launches as itself; a text line as a 16:9 box at its left', () => {
		expect(launchBox({ top: 10, left: 20, width: 160, height: 90 })).toEqual({ top: 10, left: 20, width: 160, height: 90 });
		const line = launchBox({ top: 100, left: 300, width: 380, height: 36 });
		expect(line).toEqual({ top: 100, left: 300, width: 64, height: 36 });
	});

	test('it is taken once, and only while fresh', () => {
		noteVideoLaunch(element({ top: 0, left: 0, width: 160, height: 90 }), 1000);
		expect(takeVideoLaunch(2000)).not.toBeNull();
		expect(takeVideoLaunch(2000)).toBeNull();
		noteVideoLaunch(element({ top: 0, left: 0, width: 160, height: 90 }), 1000);
		expect(takeVideoLaunch(1000 + 9000)).toBeNull();
		noteVideoLaunch(element({ top: 0, left: 0, width: 0, height: 0 }), 1000);
		expect(takeVideoLaunch(1000)).toBeNull();
	});
});
