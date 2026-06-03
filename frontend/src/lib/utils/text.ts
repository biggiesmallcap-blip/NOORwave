export function initials(value: string): string {
	const parts = value.trim().split(/\s+/).slice(0, 2);
	return parts.map((part) => part[0]?.toUpperCase() ?? '').join('') || '?';
}
