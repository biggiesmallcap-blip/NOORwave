// The watch page's related grid: two full rows in focus, then an endless
// extension that sits out of focus below the screen and sharpens as it is
// scrolled into view. Pure helpers; the page owns fetching and layout.

/** Rows that stay sharp at the top of the grid. */
export const FOCUS_ROWS = 2;
/** Softest level; rows further down stay at this one. */
export const MAX_FOCUS_LEVEL = 3;
const EXCLUDE_CAP = 256;
const RECENT_ARTISTS = 24;

type FeedVideo = { tidal_id: number; artist_id: number | null };

/** Depth of field for one grid row. The first FOCUS_ROWS rows are always
 *  sharp; a row below them is sharp once its top has scrolled above the
 *  focus line, and softer the further below it sits (0..MAX_FOCUS_LEVEL). */
export function focusLevel(row: number, rowTop: number, focusLine: number, rowHeight: number): number {
	if (row < FOCUS_ROWS || rowTop <= focusLine) return 0;
	return Math.min(MAX_FOCUS_LEVEL, 1 + Math.floor((rowTop - focusLine) / Math.max(1, rowHeight)));
}

/** Column count from getComputedStyle(grid).gridTemplateColumns. */
export function gridColumns(template: string): number {
	const tracks = template.trim().split(/\s+/).filter((track) => track && track !== 'none');
	return Math.max(1, tracks.length);
}

/** The sharp rows plus one row of extension should be on the page. */
export function needsMore(count: number, columns: number): boolean {
	return count < (FOCUS_ROWS + 1) * columns;
}

/** Placeholder tiles that finish the last row while a batch loads. */
export function placeholderCount(count: number, columns: number, loading: boolean): number {
	if (!loading) return 0;
	const rest = count % columns;
	return rest === 0 ? columns : columns - rest;
}

/** A feed that has ended keeps whole rows only. The sharp rows are never
 *  cut: if the feed ran dry inside them, show what there is. */
export function trimToRows<T>(items: T[], columns: number): T[] {
	if (items.length <= FOCUS_ROWS * columns) return items;
	return items.slice(0, items.length - (items.length % columns));
}

/** Append a batch, skipping the playing video and repeats. */
export function mergeFeed<T extends FeedVideo>(shown: T[], batch: T[], playingId: number): T[] {
	const seen = new Set([playingId, ...shown.map((video) => video.tidal_id)]);
	const added = batch.filter((video) => {
		if (seen.has(video.tidal_id)) return false;
		seen.add(video.tidal_id);
		return true;
	});
	return added.length > 0 ? [...shown, ...added] : shown;
}

/** What the radio request should skip: the playing video, then everything
 *  shown, newest first (the server keeps the first 256). Recent artists let
 *  it rotate outward instead of circling the same few. */
export function radioExclusions(shown: FeedVideo[], playingId: number) {
	const newest = [...shown].reverse();
	const exclude = [playingId, ...newest.map((video) => video.tidal_id)].slice(0, EXCLUDE_CAP);
	const recentArtists: number[] = [];
	for (const video of newest.slice(0, RECENT_ARTISTS)) {
		if (video.artist_id != null && !recentArtists.includes(video.artist_id)) recentArtists.push(video.artist_id);
	}
	return { exclude, recentArtists };
}
