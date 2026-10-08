// Work grouping for long albums (audit "Album page: liner notes"). Classical
// and similar albums name each movement "Work: Movement" ("Suite No. 1 in G
// Major, BWV 1007: I. Prelude"). Consecutive tracks that share the part
// before the colon form a work: the album shows a work header and each row
// shows only the movement. Anything that does not fit falls back to a flat
// list, so ordinary albums are untouched.

export interface WorkGroup {
	/** The shared prefix, e.g. "Suite No. 1 in G Major, BWV 1007". */
	work: string;
	/** Index of the first and last track in the group (inclusive). */
	start: number;
	end: number;
}

export interface WorkLayout {
	groups: WorkGroup[];
	/** Title to show per track: the movement inside a group, else the title. */
	displayTitles: string[];
}

const MIN_GROUP = 2;

function split(title: string): { work: string; part: string } | null {
	// The last colon: titles often lead with the composer ("J.S. Bach: Suite
	// No. 1 ..., BWV 1007: I. Prelude"), and the movement is always last.
	const index = title.lastIndexOf(': ');
	if (index <= 0) return null;
	const work = title.slice(0, index).trim();
	const part = title.slice(index + 2).trim();
	if (!work || !part) return null;
	return { work, part };
}

export function groupWorks(titles: readonly string[]): WorkLayout {
	const displayTitles = [...titles];
	const groups: WorkGroup[] = [];
	let i = 0;
	while (i < titles.length) {
		const head = split(titles[i]);
		if (!head) {
			i += 1;
			continue;
		}
		let j = i + 1;
		while (j < titles.length && split(titles[j])?.work === head.work) j += 1;
		if (j - i >= MIN_GROUP) {
			groups.push({ work: head.work, start: i, end: j - 1 });
			for (let k = i; k < j; k += 1) displayTitles[k] = split(titles[k])!.part;
		}
		i = j;
	}
	return { groups, displayTitles };
}
