import { render } from 'svelte/server';
import { afterEach, describe, expect, test, vi } from 'vitest';
import type { TidalSearchVideo } from '$lib/api/client';
import { clearVideoSession, playVideo, setVideoBrowseMode } from '$lib/stores/video_session';
import VideoBackLink from './VideoBackLink.svelte';

vi.mock('$lib/api/client', () => ({
	api: {
		getVideoRadioNext: vi.fn(),
		getTidalVideoStream: vi.fn(),
		recordVideoHistory: vi.fn().mockResolvedValue({ ok: true }),
		finishVideoHistory: vi.fn().mockResolvedValue({ ok: true }),
	},
}));

const video: TidalSearchVideo = {
	tidal_id: 1, title: 'Never Been To Spain', duration_ms: 180_000, artist_id: 10,
	artist_name: 'Elvis Presley', album_tidal_id: null, artwork_url: null,
	quality: null, explicit: null, type: 'Music Video',
};

async function playing() {
	await playVideo(video, { queue: [video], source: 'direct', sourceLabel: null }, {
		preloaded: { url: 'https://example.test/stream.m3u8', expiresAt: null },
	});
}

afterEach(() => clearVideoSession());

describe('video back button', () => {
	test('is absent when nothing is playing: the video pages are tabs, not a stack', () => {
		const { body } = render(VideoBackLink, { props: { current: 'stations' } });
		expect(body).not.toContain('back-link');
	});

	test('is the shared back button and names where it goes', async () => {
		await playing();
		const { body } = render(VideoBackLink, { props: { current: 'stations' } });
		expect(body).toContain('class="back-link');
		expect(body).toContain('href="/videos"');
		expect(body).toContain('aria-label="Back to the player"');
		expect(body).toMatch(/>Back<\/a>/);
	});

	test('on /videos it steps between the player and the picks', async () => {
		await playing();
		setVideoBrowseMode(false);
		const picks = render(VideoBackLink, { props: { current: 'videos', canBrowse: true } }).body;
		expect(picks).toContain('aria-label="Back to picks"');
		expect(picks).toMatch(/<button[^>]*class="back-link/);
		setVideoBrowseMode(true);
		const player = render(VideoBackLink, { props: { current: 'videos', canBrowse: true } }).body;
		expect(player).toContain('aria-label="Back to the player"');
		setVideoBrowseMode(false);
		const nothingToBrowse = render(VideoBackLink, { props: { current: 'videos', canBrowse: false } }).body;
		expect(nothingToBrowse).not.toContain('back-link');
	});
});
