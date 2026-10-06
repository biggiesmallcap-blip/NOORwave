import assert from 'node:assert/strict';
import { chromium } from 'playwright';

const baseUrl = process.env.DJ_STORY_TEST_URL ?? 'http://127.0.0.1:17703';
const browser = await chromium.launch({ channel: 'chrome', headless: true });
const page = await browser.newPage();
const pageErrors = [];
const requests = [];
let snapshot;
page.on('pageerror', error => pageErrors.push(error.message));

try {
	await page.route(new URL('/cockpit-test', baseUrl).href, route => route.fulfill({
		contentType: 'text/html', body: '<html><body><div id="cockpit"></div></body></html>'
	}));
	await page.route(url => url.pathname.startsWith('/api/'), async route => {
		const path = new URL(route.request().url()).pathname;
		requests.push({ path, at: Date.now() });
		const body = path === '/api/dj/status' ? snapshot
			: path === '/api/dj/enabled' ? { enabled: true }
			: path === '/api/dj/policy' ? { mix_intent: 'balanced', transition_speed_bias: 'neutral', preferred_strategy: 'adaptive' }
			: { features: null };
		await route.fulfill({ contentType: 'application/json', body: JSON.stringify(body) });
	});
	await page.goto(new URL('/cockpit-test', baseUrl).href);
	snapshot = await page.evaluate(async () => {
		const { Cockpit, mount, tick, unmount } = await import('/src/lib/components/dj-cockpit/fixtures/TransitionStoryHarness.svelte');
		const player = await import('/src/lib/stores/player.ts');
		const deck = id => ({ media_ref_kind: 'library_track', media_ref_id: String(id), title: `Track ${id}`,
			beat_markers_ms: [], downbeat_markers_ms: [], phrase_markers_ms: [], drop_markers_ms: [], manual_drop_markers_ms: [],
			profile_ready: true, profile_status: 'ready', waveform_status: 'ready', waveform_peaks: [], safe_crossfade_only: false });
		player.currentTrack.set({ id: 1, tidal_id: null, title: 'Track 1' });
		player.position.set(97000);
		player.isPlaying.set(true);
		window.mountCockpit = () => { window.cockpit = mount(Cockpit, { target: document.querySelector('#cockpit') }); };
		window.stopCockpit = async () => { player.isPlaying.set(false); await unmount(window.cockpit); };
		window.pauseFixture = async () => { player.isPlaying.set(false); await tick(); };
		window.promoteFixture = async () => { player.currentTrack.set({ id: 2, tidal_id: null, title: 'Track 2' }); player.position.set(2000); await tick(); };
		return { enabled: true, current: deck(1), next: deck(2), planning_status: 'armed', timing_status: 'armed',
			last_transition_event_id: 7, playback_position_ms: 97000, planned_start_ms: 100000,
			recent_timing_events: [], rejected_alternatives: [], drop_preview: { status: 'skipped' } };
	});
	await page.evaluate(() => window.mountCockpit());
	await page.waitForTimeout(1750);
	const statusPolls = requests.filter(request => request.path === '/api/dj/status');
	const enabledPolls = requests.filter(request => request.path === '/api/dj/enabled');
	const policyPolls = requests.filter(request => request.path === '/api/dj/policy');
	assert.ok(statusPolls.length >= 3, `Expected near-fire status polling, got ${statusPolls.length}`);
	assert.equal(enabledPolls.length, 1);
	assert.equal(policyPolls.length, 1);
	await page.evaluate(() => window.pauseFixture());
	const pollsBeforePause = statusPolls.length;
	await page.waitForTimeout(800);
	assert.equal(requests.filter(request => request.path === '/api/dj/status').length, pollsBeforePause);
	snapshot = { ...snapshot, current: snapshot.next, next: undefined, playback_position_ms: 2000,
		recent_timing_events: [{ event_id: 7, from_title: 'Track 1', to_title: 'Track 2',
			planned_template: 'SlamCut', renderer_template: 'SlamCut', actual_start_ms: 100010,
			timing_status: 'fired', timing_quality: 'tight', timing_direction: 'on_time',
			runtime_rendered_dj_mixer: true, runtime_renderer_status: 'rendered_handoff', rejected_alternatives: [] }] };
	await page.evaluate(() => window.promoteFixture());
	await page.getByText('Cut fired · Track 1 → Track 2', { exact: true }).waitFor();
	assert.equal(await page.locator('.cursor').count(), 0);
	await page.evaluate(() => window.stopCockpit());
	assert.deepEqual(pageErrors, []);
	console.log(JSON.stringify({ nearFireStatusPolls: statusPolls.length, enabledPolls: enabledPolls.length,
		policyPolls: policyPolls.length, pausedPollingSlowed: true, confirmedCutVisible: true, pageErrors }));
} finally {
	await browser.close();
}
