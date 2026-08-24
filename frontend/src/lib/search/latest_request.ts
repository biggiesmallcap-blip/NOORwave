export interface LatestRequest {
	token: number;
	signal: AbortSignal;
}

export interface LatestRequestGate {
	begin(): LatestRequest;
	invalidate(): void;
	isCurrent(token: number): boolean;
}

export function createLatestRequestGate(): LatestRequestGate {
	let sequence = 0;
	let controller: AbortController | null = null;

	return {
		begin() {
			controller?.abort();
			controller = new AbortController();
			return { token: ++sequence, signal: controller.signal };
		},
		invalidate() {
			sequence += 1;
			controller?.abort();
			controller = null;
		},
		isCurrent(token: number) {
			return sequence === token && controller?.signal.aborted === false;
		},
	};
}
