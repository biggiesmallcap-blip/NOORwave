/** Returns a debounced version of `fn`. Calling `debounced.cancel()` clears any pending timer. */
export function debounce<T extends (...args: never[]) => void>(
	fn: T,
	ms: number,
): T & { cancel(): void } {
	let timer: ReturnType<typeof setTimeout> | null = null;
	const debounced = (...args: Parameters<T>) => {
		if (timer !== null) clearTimeout(timer);
		timer = setTimeout(() => {
			timer = null;
			fn(...args);
		}, ms);
	};
	debounced.cancel = () => {
		if (timer !== null) {
			clearTimeout(timer);
			timer = null;
		}
	};
	return debounced as T & { cancel(): void };
}
