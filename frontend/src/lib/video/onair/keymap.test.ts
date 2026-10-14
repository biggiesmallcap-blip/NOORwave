import { describe, expect, test } from 'vitest';
import { isTypingTarget, onAirKeyAction, type KeyLike } from './keymap';

const idle = { guideOpen: false, overlayOpen: false };
const key = (k: string, extra: Partial<KeyLike> = {}): KeyLike => ({ key: k, ...extra });

describe('the remote key map', () => {
	test('Up and Down flip channels; Shift keeps them as volume', () => {
		expect(onAirKeyAction(key('ArrowUp'), idle)).toEqual({ kind: 'flip', step: 1 });
		expect(onAirKeyAction(key('ArrowDown'), idle)).toEqual({ kind: 'flip', step: -1 });
		expect(onAirKeyAction(key('ArrowUp', { shiftKey: true }), idle)).toEqual({ kind: 'volume', up: true });
		expect(onAirKeyAction(key('ArrowDown', { shiftKey: true }), idle)).toEqual({ kind: 'volume', up: false });
		expect(onAirKeyAction(key('PageUp'), idle)).toEqual({ kind: 'flip', step: 1 });
		expect(onAirKeyAction(key('ChannelDown'), idle)).toEqual({ kind: 'flip', step: -1 });
	});

	test('digits dial, G opens the guide, +/- teach the channel', () => {
		expect(onAirKeyAction(key('7'), idle)).toEqual({ kind: 'digit', digit: 7 });
		expect(onAirKeyAction(key('0'), idle)).toEqual({ kind: 'digit', digit: 0 });
		expect(onAirKeyAction(key('g'), idle)).toEqual({ kind: 'guide' });
		expect(onAirKeyAction(key('G', { shiftKey: true }), idle)).toEqual({ kind: 'guide' });
		expect(onAirKeyAction(key('+', { shiftKey: true }), idle)).toEqual({ kind: 'taste', more: true });
		expect(onAirKeyAction(key('='), idle)).toEqual({ kind: 'taste', more: true });
		expect(onAirKeyAction(key('-'), idle)).toEqual({ kind: 'taste', more: false });
	});

	test('leaves the player shortcuts alone', () => {
		for (const other of [' ', 'ArrowLeft', 'ArrowRight', 'f', 'm', 'q', 'Enter', 'MediaTrackNext', 'F5']) {
			expect(onAirKeyAction(key(other), idle), other).toBeNull();
		}
	});

	test('modifiers belong to the app (Ctrl +/- is zoom)', () => {
		expect(onAirKeyAction(key('+', { ctrlKey: true }), idle)).toBeNull();
		expect(onAirKeyAction(key('-', { metaKey: true }), idle)).toBeNull();
		expect(onAirKeyAction(key('ArrowUp', { altKey: true }), idle)).toBeNull();
		expect(onAirKeyAction(key('ArrowUp', { defaultPrevented: true }), idle)).toBeNull();
	});

	test('never fires while typing in a field', () => {
		for (const tagName of ['INPUT', 'textarea', 'SELECT']) {
			expect(onAirKeyAction(key('5', { target: { tagName } as unknown as EventTarget }), idle)).toBeNull();
		}
		const editable = { tagName: 'DIV', isContentEditable: true } as unknown as EventTarget;
		expect(onAirKeyAction(key('g', { target: editable }), idle)).toBeNull();
		expect(isTypingTarget({ tagName: 'BUTTON' } as unknown as EventTarget)).toBe(false);
		expect(isTypingTarget(null)).toBe(false);
	});

	test('Esc closes an open overlay first, otherwise it is left to fullscreen', () => {
		expect(onAirKeyAction(key('Escape'), { guideOpen: true, overlayOpen: true })).toEqual({ kind: 'close' });
		expect(onAirKeyAction(key('Escape'), { guideOpen: false, overlayOpen: true })).toEqual({ kind: 'close' });
		expect(onAirKeyAction(key('Escape'), idle)).toBeNull();
	});

	test('with the guide open, arrows and digits are the guide\'s; G still closes it', () => {
		const guide = { guideOpen: true, overlayOpen: true };
		expect(onAirKeyAction(key('ArrowUp'), guide)).toBeNull();
		expect(onAirKeyAction(key('4'), guide)).toBeNull();
		expect(onAirKeyAction(key('g'), guide)).toEqual({ kind: 'guide' });
	});
});
