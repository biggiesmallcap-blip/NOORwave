export function normalizeRemoteSearchQuery(input: string): string {
	return input.trim().replace(/\s+/g, ' ');
}

export function shouldRunRemoteSearch(input: string): boolean {
	return normalizeRemoteSearchQuery(input).length >= 2;
}
