// Release notes arrive as the markdown subset docs/releases/*.md uses. The
// dialog renders blocks as plain text nodes, so nothing here needs escaping.
export type ReleaseNoteBlock =
	| { kind: 'heading'; text: string }
	| { kind: 'paragraph'; text: string }
	| { kind: 'list'; items: string[] };

const HEADING = /^#{1,6}\s+(.*)$/;
const BULLET = /^[-*+]\s+(.*)$/;

function stripInline(text: string): string {
	return text
		.replace(/\[([^\]]+)\]\([^)]*\)/g, '$1')
		.replace(/(\*\*|__)(.+?)\1/g, '$2')
		.replace(/`([^`]+)`/g, '$1')
		.trim();
}

export function parseReleaseNotes(notes: string): ReleaseNoteBlock[] {
	const blocks: ReleaseNoteBlock[] = [];
	let paragraph: string[] = [];
	let list: string[] | null = null;

	const flushParagraph = () => {
		if (paragraph.length) blocks.push({ kind: 'paragraph', text: stripInline(paragraph.join(' ')) });
		paragraph = [];
	};
	const flushList = () => {
		if (list?.length) blocks.push({ kind: 'list', items: list });
		list = null;
	};

	for (const raw of notes.replace(/\r\n?/g, '\n').split('\n')) {
		const line = raw.trim();
		if (!line) {
			flushParagraph();
			flushList();
			continue;
		}
		const heading = HEADING.exec(line);
		if (heading) {
			flushParagraph();
			flushList();
			blocks.push({ kind: 'heading', text: stripInline(heading[1]) });
			continue;
		}
		const bullet = BULLET.exec(line);
		if (bullet) {
			flushParagraph();
			(list ??= []).push(stripInline(bullet[1]));
			continue;
		}
		if (list?.length) {
			// A wrapped bullet continues the previous item.
			list[list.length - 1] = `${list[list.length - 1]} ${stripInline(line)}`;
			continue;
		}
		paragraph.push(line);
	}
	flushParagraph();
	flushList();
	return blocks;
}
