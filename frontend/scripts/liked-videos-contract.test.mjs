import { readFileSync } from 'node:fs';
import { describe, expect, test } from 'vitest';

const PAGE = 'src/routes/videos/liked/+page.svelte';
const VIDEOS_PAGE = 'src/routes/videos/+page.svelte';
const CLIENT = 'src/lib/api/client.ts';

describe('liked videos contract', () => {
	test('the two video pages link to each other from mirrored places', () => {
		// A separate route rather than a third mode inside /videos: that page is
		// already a browse / search / player / snapshot-restore state machine.
		// Each page offers the other from its own leading slot, so getting back
		// is the same gesture in both directions.
		const videos = readFileSync(VIDEOS_PAGE, 'utf8');
		expect(videos).toContain('class="tools-lead"');
		expect(videos).toContain('href="/videos/liked"');
		expect(videos).toContain('href="/tidal/videos"');

		const page = readFileSync(PAGE, 'utf8');
		expect(page).toContain('class="back-link" href="/videos"');
	});

	test('a version shows what tells it apart from its siblings', () => {
		// Four videos all titled "Jamming" are separated by nothing on the card
		// but runtime; year and resolution ride along with the payload already
		// being fetched.
		const page = readFileSync(PAGE, 'utf8');
		expect(page).toContain('function versionMeta(version: LikedVideoVersion): string');
		expect(page).toContain("version.release_year ? String(version.release_year) : ''");
		expect(page).toContain('{versionMeta(version)}');
	});

	test('client talks to the three liked-video endpoints', () => {
		const client = readFileSync(CLIENT, 'utf8');
		expect(client).toContain("fetchApi<LikedVideosResponse>('/api/videos/liked')");
		expect(client).toContain("'/api/videos/liked/refresh'");
		expect(client).toContain("'/api/videos/liked/hide'");
		// The card's whole set of liked rows: a song favorited twice draws one
		// card, and suppressing half of it would just redraw from the other half.
		expect(client).toContain('track_ids: trackIds, tidal_video_id: tidalVideoId');
	});

	test('a card is a song, and its versions hang off it', () => {
		// Grouping is the server's call (one definition of "same song"), so the
		// page keys on song_key and never re-derives identity.
		const page = readFileSync(PAGE, 'utf8');
		expect(page).toContain('{#each filtered as video, index (video.song_key)}');
		expect(page).toContain('function face(video: LikedVideo): LikedVideoVersion');
		expect(page).toContain('return video.versions[0];');
	});

	test('the versions chip is its own control, so tapping the card still plays', () => {
		const page = readFileSync(PAGE, 'utf8');
		expect(page).toContain('{#if video.versions.length > 1}');
		expect(page).toContain('{video.versions.length} versions');
		expect(page).toContain('onclick={() => void playFrom(index)}');
		// One popout open at a time.
		expect(page).toContain('openVersions = openVersions === video.song_key ? null : video.song_key');
	});

	test('play all queues one video per song, not every version', () => {
		// Six cuts of the same song back to back is nobody's idea of playing the
		// wall; a specific version is picked from the popout instead.
		const page = readFileSync(PAGE, 'utf8');
		expect(page).toContain('return filtered.map((v) => toQueueItem(v, face(v)));');
		expect(page).toContain('async function playVersion(');
	});

	test('play all, shuffle and card clicks all go through playVideo with a queue', () => {
		const page = readFileSync(PAGE, 'utf8');
		expect(page).toContain("import { playVideo } from '$lib/stores/video_session'");
		// One shared entry point, so no surface can drift into its own playback path.
		expect(page).toContain("await playVideo(queue[index], { queue, source: 'search', sourceLabel })");
		expect(page).toContain('async function playAll()');
		expect(page).toContain('async function shuffle()');
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

	test('cards and version rows both wire the menu plus the wrong-match action', () => {
		const page = readFileSync(PAGE, 'utf8');
		expect(page).toContain("import { buildVideoMenu } from '$lib/player/video_menu'");
		expect(page).toContain('oncontextmenu={(event) => menu(event, video, face(video))}');
		expect(page).toContain('oncontextmenu={(event) => menu(event, video, version)}');
		expect(page).toContain("label: 'Wrong match - hide this'");
		expect(page).toContain('api.hideLikedVideo(video.track_ids, version.tidal_video_id)');
	});

	test('hiding drops one version, and the card survives on its others', () => {
		const page = readFileSync(PAGE, 'utf8');
		expect(page).toContain('const previous = videos;');
		expect(page).toContain('ver.tidal_video_id !== version.tidal_video_id');
		// Only a song with nothing left leaves the wall.
		expect(page).toContain('.filter((v) => v.versions.length > 0)');
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
