import { savedDateMillis } from '$lib/utils/format';

// Watch history in days, the way a diary reads: Today, Yesterday, then the
// weekday for this week, then the date (with the year once it is another
// year). Local days, newest first, entries keep their order inside a day.

export interface HistoryDay<T> {
	key: string;
	label: string;
	entries: T[];
}

function dayKey(date: Date): string {
	return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}-${String(date.getDate()).padStart(2, '0')}`;
}

/** "Today", "Yesterday", "Monday", "14 October", "14 October 2025". */
export function dayLabel(date: Date, now: Date = new Date()): string {
	const start = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
	const days = Math.round((start(now) - start(date)) / 86_400_000);
	if (days <= 0) return 'Today';
	if (days === 1) return 'Yesterday';
	if (days < 7) return date.toLocaleDateString('en-GB', { weekday: 'long' });
	const sameYear = date.getFullYear() === now.getFullYear();
	return date.toLocaleDateString('en-GB', sameYear ? { day: 'numeric', month: 'long' } : { day: 'numeric', month: 'long', year: 'numeric' });
}

export function historyDays<T extends { watched_at: string }>(entries: readonly T[], now: Date = new Date()): HistoryDay<T>[] {
	const days: HistoryDay<T>[] = [];
	for (const entry of entries) {
		const at = savedDateMillis(entry.watched_at);
		const date = at === null ? null : new Date(at);
		const key = date ? dayKey(date) : 'unknown';
		const last = days[days.length - 1];
		if (last && last.key === key) last.entries.push(entry);
		else days.push({ key, label: date ? dayLabel(date, now) : 'Earlier', entries: [entry] });
	}
	return days;
}

/** "watched twice", "watched 3 times"; nothing for once. */
export function playsLine(plays: number): string {
	if (plays <= 1) return '';
	return plays === 2 ? 'watched twice' : `watched ${plays} times`;
}
