import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const source = readFileSync(resolve(import.meta.dirname, '../src/routes/videos/+page.svelte'), 'utf8');

describe('video session snapshot behavior', () => {
	test('plain Videos visits do not restore direct video snapshots', () => {
		expect(source).toContain('function snapshotHasRestorableContext');
		expect(source).toContain('snap?.selectedVideo && snapshotHasRestorableContext(snap)');
		expect(source).toContain('clearSessionSnapshot();');
	});

	test('direct video sessions are not persisted without URL context', () => {
		// A direct video (no search query, no active mix) has no restorable browse
		// context, so it is not snapshotted.
		expect(source).toContain('if (selectedVideo && (lastQuery || activeMixId || activePlaylistId)) saveSessionSnapshot();');
		expect(source).toContain('else clearSessionSnapshot();');
	});

	test('clear request resets the video session, snapshot, and URL', () => {
		expect(source).toContain('function clearVideoPageSession');
		// Playback state lives in the dock/store now, so clearing routes through
		// the controller rather than nulling local stream fields.
		expect(source).toContain('clearVideoSession();');
		expect(source).toContain('clearSessionSnapshot();');
		expect(source).toContain("void goto('/videos', { replaceState: true, keepFocus: true });");
		expect(source).toContain('clearVideoPageSession();');
	});

	test('url video ids from search hydrate metadata before generic fallback selection', () => {
		expect(source).toContain('if (q) await runSearch(q, false);');
		expect(source).toContain('const fromContext = findVideoInCurrentContext(videoId);');
		expect(source).toContain('if (fromContext) {');
		expect(source).toContain('void selectVideo(fromContext, false);');
		expect(source.indexOf('const fromContext = findVideoInCurrentContext(videoId);')).toBeLessThan(
			source.indexOf('title: `TIDAL video ${videoId}`')
		);
	});
});
