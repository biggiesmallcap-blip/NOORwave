import { describe, expect, test } from 'vitest';

import {
	SILENT_SOURCE_LABELS,
	SOURCE_LEGEND,
	formatQueueSource,
	queueSourceSlug,
} from './queue_source';

describe('formatQueueSource', () => {
	test('names every source the backend actually writes', () => {
		const cases: Record<string, string> = {
			library: 'Library',
			local: 'Library',
			playlist: 'Playlist',
			genre: 'Genre',
			automix: 'Automix',
			'automix-new': 'Automix',
			engine: 'Automix',
			radio: 'Song radio',
			radio_pending: 'Song radio',
			lastfm: 'Last.fm',
			lastfm_similar: 'Last.fm',
			lastfm_api: 'Last.fm',
			tidal: 'TIDAL',
			tidal_similar: 'TIDAL',
			tidal_new_release: 'TIDAL',
			spotify: 'Spotify',
			dj: 'DJ',
			dj_gain_program: 'DJ',
			blend: 'Discover',
			discover: 'Discover',
			external: 'Outside library',
			user: 'Manual',
			user_queue: 'Manual',
			user_play_next: 'Manual',
			manual_drop_cue: 'Manual',
		};

		for (const [source, label] of Object.entries(cases)) {
			expect(formatQueueSource(source), source).toBe(label);
		}
	});

	test('never echoes a raw slug for a source it has not met', () => {
		// The regression this guards: `radio_pending` used to be printed verbatim
		// under the album line in the now-playing panel.
		expect(formatQueueSource('some_new_backend_source')).toBe('Some new backend source');
		expect(formatQueueSource('BEAT-SYNC')).toBe('Beat sync');
		expect(formatQueueSource('')).toBe('Queued');
		expect(formatQueueSource('   ')).toBe('Queued');
	});

	test('hand-queued sources resolve to the label the now-playing card suppresses', () => {
		for (const source of ['user', 'user_queue', 'user_play_next', 'manual']) {
			expect(SILENT_SOURCE_LABELS.has(formatQueueSource(source))).toBe(true);
		}
		expect(SILENT_SOURCE_LABELS.has(formatQueueSource('radio'))).toBe(false);
	});
});

describe('queueSourceSlug', () => {
	test('produces css-safe slugs and keeps discover-automix distinguishable', () => {
		expect(queueSourceSlug('automix-new')).toBe('automix-new');
		expect(queueSourceSlug('automix')).toBe('automix');
		expect(queueSourceSlug('radio_pending')).toBe('song-radio');
		expect(queueSourceSlug('lastfm_similar')).toBe('last-fm');
		expect(queueSourceSlug('external')).toBe('outside-library');
		expect(queueSourceSlug('some_new_backend_source')).toMatch(/^[a-z0-9-]+$/);
	});

	test('every legend slug is one a real source can produce', () => {
		const producible = new Set([
			queueSourceSlug('library'),
			queueSourceSlug('playlist'),
			queueSourceSlug('genre'),
			queueSourceSlug('automix'),
			queueSourceSlug('automix-new'),
			queueSourceSlug('discover'),
			queueSourceSlug('radio'),
			queueSourceSlug('lastfm'),
			queueSourceSlug('tidal'),
			queueSourceSlug('dj'),
			queueSourceSlug('user_queue'),
		]);

		for (const entry of SOURCE_LEGEND) {
			expect(producible.has(entry.slug), entry.slug).toBe(true);
		}
	});
});
