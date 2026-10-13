import type { ChannelProgram, VideoChannel } from '$lib/api/video_explore_types';

// The guide overlay's grid maths: which slice of the day it shows, where a
// program sits on it, and how the keyboard moves through it. Pure, so the
// component stays about drawing.

// Music videos run about four minutes, so a TV guide's three hours would
// shrink every program to a sliver. Forty-five minutes keeps a block wide
// enough to read: a little of what just played, mostly what is coming.
export const GUIDE_SPAN_S = 45 * 60;
const BACK_S = 8 * 60;
const ALIGN_S = 5 * 60;
const TICK_S = 10 * 60;
const DAY_S = 86_400;

/** The guide's window: from a few minutes before now (on a five minute
 *  mark), 45 minutes long, inside today. */
export function guideWindow(nowS: number, spanS = GUIDE_SPAN_S): { from: number; to: number } {
	const from = Math.max(0, Math.floor((nowS - BACK_S) / ALIGN_S) * ALIGN_S);
	return { from, to: Math.min(DAY_S, from + spanS) };
}

/** Ten minute marks inside the window. */
export function guideTicks(from: number, to: number): number[] {
	const ticks: number[] = [];
	for (let t = Math.ceil(from / TICK_S) * TICK_S; t < to; t += TICK_S) ticks.push(t);
	return ticks;
}

/** A program's box on the track, in 0..1 of the window, clipped to it.
 *  Null when it falls outside. */
export function placeProgram(program: Pick<ChannelProgram, 'start_s' | 'duration_s'>, from: number, to: number): { left: number; width: number } | null {
	const span = to - from;
	if (span <= 0) return null;
	const start = Math.max(from, program.start_s);
	const end = Math.min(to, program.start_s + Math.max(0, program.duration_s));
	if (end <= start) return null;
	return { left: (start - from) / span, width: (end - start) / span };
}

/** What the guide knows before its fetches land: each channel's now and next
 *  from the lineup. */
export function lineupPrograms(channel: Pick<VideoChannel, 'now' | 'next'>): ChannelProgram[] {
	const programs: ChannelProgram[] = [];
	if (channel.now) {
		const now = channel.now;
		programs.push({ slot: now.slot, start_s: now.start_s, duration_s: now.duration_s, video: now.video, why: now.why });
	}
	if (channel.next) programs.push(channel.next);
	return programs;
}

/** The program in a row at `timeS`, else the first one after it, else the
 *  last one. -1 for an empty row. */
export function programAt(programs: readonly Pick<ChannelProgram, 'start_s' | 'duration_s'>[], timeS: number): number {
	if (programs.length === 0) return -1;
	const covering = programs.findIndex((p) => p.start_s <= timeS && timeS < p.start_s + p.duration_s);
	if (covering >= 0) return covering;
	const after = programs.findIndex((p) => p.start_s > timeS);
	return after >= 0 ? after : programs.length - 1;
}

export interface GuideFocus {
	row: number;
	/** The time the cursor sits at; moving between rows keeps it. */
	time: number;
}

/** Arrow keys over the grid. Up/Down change channel and keep the time;
 *  Left/Right step to the program before or after. Null for other keys. */
export function moveGuideFocus(
	focus: GuideFocus,
	key: string,
	rows: readonly (readonly Pick<ChannelProgram, 'start_s' | 'duration_s'>[])[],
	window: { from: number; to: number },
): GuideFocus | null {
	const last = rows.length - 1;
	if (last < 0) return null;
	switch (key) {
		case 'ArrowUp':
			return { ...focus, row: Math.max(0, focus.row - 1) };
		case 'ArrowDown':
			return { ...focus, row: Math.min(last, focus.row + 1) };
		case 'PageUp':
			return { ...focus, row: Math.max(0, focus.row - 6) };
		case 'PageDown':
			return { ...focus, row: Math.min(last, focus.row + 6) };
		case 'Home':
			return { ...focus, row: 0 };
		case 'End':
			return { ...focus, row: last };
		case 'ArrowLeft':
		case 'ArrowRight': {
			const programs = rows[focus.row] ?? [];
			const index = programAt(programs, focus.time);
			if (index < 0) return focus;
			const target = programs[index + (key === 'ArrowRight' ? 1 : -1)];
			if (!target) return focus;
			const start = Math.max(window.from, target.start_s);
			if (start >= window.to) return focus;
			return { ...focus, time: start + 1 };
		}
	}
	return null;
}
