import { describe, expect, test } from 'vitest';
import { resolveReduceMotion, parseMotionPreference } from './motion';

describe('motion preference', () => {
	test('follow system reduces only when the OS asks', () => {
		expect(resolveReduceMotion('system', false)).toBe(false);
		expect(resolveReduceMotion('system', true)).toBe(true);
	});

	test('always reduces regardless of the OS', () => {
		expect(resolveReduceMotion('reduce', false)).toBe(true);
	});

	test('unknown stored values fall back to follow system', () => {
		expect(parseMotionPreference('reduce')).toBe('reduce');
		expect(parseMotionPreference('off')).toBe('system');
		expect(parseMotionPreference(null)).toBe('system');
	});
});
