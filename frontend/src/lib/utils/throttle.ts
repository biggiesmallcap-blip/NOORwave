export function rafThrottle<A extends unknown[]>(fn: (...args: A) => void): (...args: A) => void {
	let scheduled = false;
	let lastArgs: A | null = null;
	const tick =
		typeof requestAnimationFrame === 'function'
			? requestAnimationFrame
			: (cb: FrameRequestCallback) => setTimeout(() => cb(performance.now()), 16) as unknown as number;
	return (...args: A) => {
		lastArgs = args;
		if (scheduled) return;
		scheduled = true;
		tick(() => {
			scheduled = false;
			if (lastArgs) fn(...lastArgs);
			lastArgs = null;
		});
	};
}
