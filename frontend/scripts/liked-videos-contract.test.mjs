import { readFileSync } from 'node:fs';
import { describe, expect, test } from 'vitest';

const PAGE = 'src/routes/videos/liked/+page.svelte';
const VIDEOS_PAGE = 'src/routes/videos/+page.svelte';
const CLIENT = 'src/lib/api/client.ts';

describe('liked videos contract', () => {
	test('the wall lives on its own route, reachable from /videos', () => {
		// A separate route rather than a third mode inside /videos: that page is
		// already a browse / search / player / snapshot-restore state machine.
		const videos = readFileSync(VIDEOS_PAGE, 'utf8');
		expect(videos).toContain('href="/videos/liked"');
		expect(videos).toContain('href="/tidal/videos"');
	});

	test('client talks to the three liked-video endpoints', () => {
		const client = readFileSync(CLIENT, 'utf8');
		expect(client).toContain("fetchApi<LikedVideosResponse>('/api/videos/liked')");
		expect(client).toContain("'/api/videos/liked/refresh'");
		expect(client).toContain("'/api/videos/liked/hide'");
		expect(client).toContain('track_id: trackId, tidal_video_id: tidalVideoId');
	});

	test('cards key on the (track, video) pair so duplicates survive', () => {
		// Live takes, covers and alternates are kept on purpose. Keying on
		// track_id alone would collapse them back into one card.
		const page = readFileSync(PAGE, 'utf8');
		expect(page).toContain('`${video.track_id}:${video.tidal_video_id}`');
		expect(page).toContain('{#each filtered as video, index (cardKey(video))}');
	});

	test('play all, shuffle and card clicks all go through playVideo with a queue', () => {
		const page = readFileSync(PAGE, 'utf8');
		expect(page).toContain("import { playVideo } from '$lib/stores/video_session'");
		// One shared entry point, so no surface can drift into its own playback path.
		expect(page).toContain("await playVideo(queue[index], { queue, source: 'search', sourceLabel })");
		expect(page).toContain('async function playAll()');
		expect(page).toContain('async function shuffle()');
		expect(page).toContain('onclick={() => void playFrom(index)}');
	});

	test('the ticked genre names the queue, which is what "play genre" means here', () => {
		const page = readFileSync(PAGE, 'utf8');
		expect(page).toContain('activeGenre ? `Liked ${activeGenre} videos` : \'Liked videos\'');
	});

	test('genre and year pills filter, and a year-less card drops out under a year pill', () => {
		const page = readFileSync(PAGE, 'utf8');
		expect(page).toContain("rows = rows.filter((v) => v.genre === activeGenre)");
		expect(page).toContain('rows = rows.filter((v) => v.album_year === activeYear)');
		// Pills are built from the data, so a library with no years shows none.
		expect(page).toContain('videos.map((v) => v.album_year)');
		expect(page).toContain('videos.map((v) => v.genre)');
	});

	test('artwork goes through the shared component at a legal TIDAL size', () => {
		const page = readFileSync(PAGE, 'utf8');
		expect(page).toContain('<ArtworkImage');
		// 320 is the documented size for grid tiles; the backend emits 640.
		expect(page).toContain('size={320}');
		expect(page).toContain('fallbackText="VID"');
	});

	test('every card wires the shared context menu plus the wrong-match action', () => {
		const page = readFileSync(PAGE, 'utf8');
		expect(page).toContain("import { buildVideoMenu } from '$lib/player/video_menu'");
		expect(page).toContain('oncontextmenu={(event) => menu(event, video)}');
		expect(page).toContain("label: 'Wrong match - hide this'");
		expect(page).toContain('api.hideLikedVideo(video.track_id, video.tidal_video_id)');
	});

	test('hiding is optimistic and rolls back when the write fails', () => {
		const page = readFileSync(PAGE, 'utf8');
		expect(page).toContain('const previous = videos;');
		expect(page).toContain('videos = videos.filter((v) => cardKey(v) !== key);');
		expect(page).toContain('videos = previous;');
	});

	test('polling stops once the resolve has nothing left to do', () => {
		// A finished library should settle into no timer at all rather than
		// re-fetching the whole wall every few seconds forever.
		const page = readFileSync(PAGE, 'utf8');
		expect(page).toContain('const pending = running || (totalArtists > 0 && scannedArtists < totalArtists);');
		expect(page).toContain('if (!pending) return;');
	});

	test('a logged-out or still-scanning library gets its own empty state', () => {
		const page = readFileSync(PAGE, 'utf8');
		expect(page).toContain('{:else if !tidalConnected}');
		expect(page).toContain('Connect TIDAL to find videos');
		expect(page).toContain('scanPending');
	});
});
