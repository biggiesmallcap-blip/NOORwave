import { describe, expect, it } from 'vitest';
import { parseReleaseNotes } from './release_notes';

describe('parseReleaseNotes', () => {
	it('splits the curated notes into paragraph, heading and list blocks', () => {
		const notes = [
			'A stability release.',
			'',
			'### TIDAL sign-in',
			'',
			'- Sessions refresh once and retry.',
			'- The status updates on change.',
			'',
			'### Fixes',
			'',
			'- Menus open away from the row.'
		].join('\n');

		expect(parseReleaseNotes(notes)).toEqual([
			{ kind: 'paragraph', text: 'A stability release.' },
			{ kind: 'heading', text: 'TIDAL sign-in' },
			{ kind: 'list', items: ['Sessions refresh once and retry.', 'The status updates on change.'] },
			{ kind: 'heading', text: 'Fixes' },
			{ kind: 'list', items: ['Menus open away from the row.'] }
		]);
	});

	it('joins wrapped bullets and drops inline markdown markers', () => {
		const notes = '- **Bold** start with `code` and a [link](https://x.test)\n  that wraps.\r\n- Next';
		expect(parseReleaseNotes(notes)).toEqual([
			{ kind: 'list', items: ['Bold start with code and a link that wraps.', 'Next'] }
		]);
	});

	it('returns no blocks for blank notes', () => {
		expect(parseReleaseNotes('  \n\n')).toEqual([]);
	});
});
