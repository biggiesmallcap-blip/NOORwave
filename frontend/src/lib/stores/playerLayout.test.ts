import { describe, expect, test } from 'vitest';
import { resolvePlayerLayout } from './playerLayout';

describe('resolvePlayerLayout', () => {
	test('keeps all explicit positions when a side layout has room', () => {
		expect(resolvePlayerLayout('right', 1240)).toBe('right');
		expect(resolvePlayerLayout('left', 1920)).toBe('left');
		expect(resolvePlayerLayout('bottom', 2560)).toBe('bottom');
	});

	test('protects the workspace at narrow and mobile widths', () => {
		expect(resolvePlayerLayout('left', 1239)).toBe('bottom');
		expect(resolvePlayerLayout('right', 680)).toBe('bottom');
		expect(resolvePlayerLayout('bottom', 679)).toBe('mobile');
	});
});
