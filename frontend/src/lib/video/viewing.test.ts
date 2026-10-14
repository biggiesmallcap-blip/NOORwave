import { describe, expect, test } from 'vitest';
import { scopeCovers } from './viewing';

describe('viewing scopes', () => {
	test('off covers nothing, On Air covers channels, all covers every video', () => {
		expect(scopeCovers('off', true)).toBe(false);
		expect(scopeCovers('off', false)).toBe(false);
		expect(scopeCovers('onair', true)).toBe(true);
		expect(scopeCovers('onair', false)).toBe(false);
		expect(scopeCovers('all', true)).toBe(true);
		expect(scopeCovers('all', false)).toBe(true);
	});
});
