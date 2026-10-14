import { describe, expect, test } from 'vitest';
import { GUIDE_SPAN_S, guideTicks, guideWindow, lineupPrograms, moveGuideFocus, placeProgram, programAt } from './guide';
import { FIXTURE_NOW_S, fixtureChannels, fixtureGuide } from './fixtures';

describe('guide grid', () => {
	test('the window starts a few minutes back on a five minute mark and runs 45 minutes', () => {
		// 21:10 -> 21:00 to 21:45.
		expect(guideWindow(FIXTURE_NOW_S + 600)).toEqual({ from: FIXTURE_NOW_S, to: FIXTURE_NOW_S + GUIDE_SPAN_S });
		expect(GUIDE_SPAN_S).toBe(45 * 60);
		// 21:13 -> 21:05.
		expect(guideWindow(FIXTURE_NOW_S + 780).from).toBe(FIXTURE_NOW_S + 300);
		// Early morning clamps to midnight; late night stops at midnight.
		expect(guideWindow(120).from).toBe(0);
		expect(guideWindow(23.9 * 3600).to).toBe(86_400);
		expect(guideTicks(0, 2700)).toEqual([0, 600, 1200, 1800, 2400]);
	});

	test('programs are placed in window fractions and clipped', () => {
		const from = FIXTURE_NOW_S;
		const to = FIXTURE_NOW_S + 3600;
		expect(placeProgram({ start_s: from + 360, duration_s: 360 }, from, to)).toEqual({ left: 0.1, width: 0.1 });
		const clipped = placeProgram({ start_s: from - 100, duration_s: 460 }, from, to);
		expect(clipped?.left).toBe(0);
		expect(clipped?.width).toBeCloseTo(0.1);
		expect(placeProgram({ start_s: from - 500, duration_s: 100 }, from, to)).toBeNull();
	});

	test('rows fall back to the lineup now and next', () => {
		const [bay] = fixtureChannels.channels;
		const programs = lineupPrograms(bay);
		expect(programs.map((p) => p.slot)).toEqual([40, 41]);
		expect(programs[0]).not.toHaveProperty('offset_s');
	});

	test('keyboard: rows keep the time, left and right step programs', () => {
		const rows = [fixtureGuide.programs, fixtureGuide.programs.slice(1), []];
		const frame = guideWindow(FIXTURE_NOW_S);
		const start = { row: 0, time: FIXTURE_NOW_S };
		expect(programAt(rows[0], FIXTURE_NOW_S)).toBe(1);
		const right = moveGuideFocus(start, 'ArrowRight', rows, frame)!;
		expect(programAt(rows[0], right.time)).toBe(2);
		const left = moveGuideFocus(start, 'ArrowLeft', rows, frame)!;
		expect(programAt(rows[0], left.time)).toBe(0);
		const down = moveGuideFocus(start, 'ArrowDown', rows, frame)!;
		expect(down).toEqual({ row: 1, time: FIXTURE_NOW_S });
		expect(moveGuideFocus(start, 'ArrowUp', rows, frame)).toEqual(start);
		expect(moveGuideFocus(start, 'End', rows, frame)?.row).toBe(2);
		expect(moveGuideFocus({ row: 2, time: 0 }, 'ArrowRight', rows, frame)).toEqual({ row: 2, time: 0 });
		expect(moveGuideFocus(start, 'x', rows, frame)).toBeNull();
	});
});
