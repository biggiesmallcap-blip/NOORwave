import { beforeEach, describe, expect, test, vi } from 'vitest';
import {
	clearLocalOnboardingComplete,
	hasLocalOnboardingComplete,
	markLocalOnboardingComplete,
} from './status';

describe('local onboarding status', () => {
	beforeEach(() => {
		const values = new Map<string, string>();
		vi.stubGlobal('localStorage', {
			clear: () => values.clear(),
			getItem: (key: string) => values.get(key) ?? null,
			removeItem: (key: string) => values.delete(key),
			setItem: (key: string, value: string) => values.set(key, value),
		});
		localStorage.clear();
	});

	test('stores completion by scope', () => {
		markLocalOnboardingComplete('token-a');

		expect(hasLocalOnboardingComplete('token-a')).toBe(true);
		expect(hasLocalOnboardingComplete('token-b')).toBe(false);
	});

	test('clears only the matching scope', () => {
		markLocalOnboardingComplete('token-a');
		markLocalOnboardingComplete('token-b');

		clearLocalOnboardingComplete('token-a');

		expect(hasLocalOnboardingComplete('token-a')).toBe(false);
		expect(hasLocalOnboardingComplete('token-b')).toBe(true);
	});
});
