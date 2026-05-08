import { describe, it, expect, vi } from 'vitest';
import { rafThrottle } from './throttle';

const FAKE_TIMER_OPTIONS = { toFake: ['requestAnimationFrame', 'setTimeout'] as const };

describe('rafThrottle', () => {
	it('coalesces multiple calls into one rAF tick', async () => {
		vi.useFakeTimers(FAKE_TIMER_OPTIONS);
		let calls = 0;
		const fn = rafThrottle(() => { calls++; });
		fn(); fn(); fn();
		expect(calls).toBe(0);
		await vi.advanceTimersByTimeAsync(20);
		expect(calls).toBe(1);
		fn();
		await vi.advanceTimersByTimeAsync(20);
		expect(calls).toBe(2);
		vi.useRealTimers();
	});

	it('passes the latest args', async () => {
		vi.useFakeTimers(FAKE_TIMER_OPTIONS);
		let last: number | undefined;
		const fn = rafThrottle((n: number) => { last = n; });
		fn(1); fn(2); fn(3);
		await vi.advanceTimersByTimeAsync(20);
		expect(last).toBe(3);
		vi.useRealTimers();
	});

	it('does not drop the final call after quiet period', async () => {
		vi.useFakeTimers(FAKE_TIMER_OPTIONS);
		let calls = 0;
		const fn = rafThrottle(() => { calls++; });
		fn();
		await vi.advanceTimersByTimeAsync(20);
		fn();
		await vi.advanceTimersByTimeAsync(20);
		expect(calls).toBe(2);
		vi.useRealTimers();
	});
});
