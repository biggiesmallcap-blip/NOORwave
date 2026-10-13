import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, test } from 'vitest';

const here = dirname(fileURLToPath(import.meta.url));
const read = (rel: string) => readFileSync(join(here, rel), 'utf8');
const landing = read('../../../routes/videos/+page.svelte');
const stations = read('../../../routes/videos/stations/+page.svelte');
const card = read('ChannelCard.svelte');
const row = read('ChannelRow.svelte');
const stage = read('OnAirStage.svelte');
const guide = read('GuideOverlay.svelte');
const dock = read('../../components/video/VideoDock.svelte');
const player = read('../../components/video/VideoPlayer.svelte');
const store = read('../../stores/video_session.ts');

describe('On Air landing (the dial)', () => {
	test('"For you tonight" first, then every channel in number order', () => {
		expect(landing).toContain('let tonight = $derived(forYouTonight(channels));');
		expect(landing).toContain('let dial = $derived(byNumber(channels));');
		expect(landing.indexOf('For you tonight')).toBeLessThan(landing.indexOf('All channels'));
		expect(landing).toContain('<ChannelCard {channel} {slot}');
		expect(landing).toContain('<ChannelRow {channel} {slot}');
	});

	test('the clock moves every bar from the lineup offset, and a run-out now refetches', () => {
		expect(landing).toContain('liveSlot(channel, elapsed)');
		expect(landing).toContain('($airTick - lineup.fetchedAt) / 1000');
		expect(landing).toContain('stale && now - lastFetch > STALE_REFETCH_MS');
		expect(card).toContain('style:transform={`scaleX(${slot.progress})`}');
		expect(row).toContain('style:transform={`scaleX(${slot.progress})`}');
	});

	test('nothing autoplays: only a pick or a ?channel= link tunes in', () => {
		const mount = landing.slice(landing.indexOf('onMount(() => {'), landing.indexOf('onDestroy('));
		expect(mount).not.toContain('tune(');
		expect(mount).not.toContain('playVideo');
		expect(landing).toContain("const channelId = params.get('channel');");
		expect(landing).toContain("void tune(known ?? { id: channelId, title: 'this channel' });");
	});

	test('the old deep links still land: videoId to the watch page, mixes and playlists play', () => {
		expect(landing).toContain('await goto(watchUrl(videoId, {');
		expect(landing).toContain("await goto('/videos', { replaceState: true, keepFocus: true });");
		expect(landing).toContain("void playVideoCollection('mix', mixId)");
		expect(landing).toContain("void playVideoCollection('playlist', playlistId)");
	});

	test('explore is a segmented control saved on the server', () => {
		expect(landing).toContain('<Segmented options={EXPLORE_OPTIONS} value={$exploreMode} label="Explore"');
		expect(landing).toContain('await setExploreMode(mode)');
	});

	test('building, empty and error states say what is going on', () => {
		expect(landing).toContain("Programming today's channels");
		expect(landing).toContain('<ErrorState title="On Air could not load"');
		expect(landing).toContain('title="No channels yet"');
		expect(landing).toContain('BUILD_POLL_MAX');
	});

	test('locked channels show coming up with an hours bar', () => {
		expect(row).toContain('Coming up');
		expect(row).toContain('unlockProgress(channel.unlock)');
		expect(row).toContain('of {Math.round(channel.unlock.need_hours)} hours');
	});

	test('artwork goes through ArtworkImage at allowed sizes; every reference has its menu', () => {
		expect(card).toContain('size={640}');
		expect(row).toContain('size={320}');
		for (const source of [card, row, stage, guide]) expect(source).toContain('buildVideoMenu(');
		for (const source of [card, row, stage]) expect(source).toContain('buildArtistMenu(');
	});

	test('stations keep working and link back to the dial', () => {
		expect(stations).toContain('<a href="/videos">Back to On Air</a>');
		expect(stations).toContain('playVideoStation(');
	});
});

describe('On Air stage wiring', () => {
	test('tune-in starts the player at the scheduled offset', () => {
		expect(store).toContain('export async function playVideoChannel(');
		expect(store).toContain('tuneStart(tune.offset_s,');
		expect(dock).toContain('startAt={$videoSession.startAtS}');
		expect(player).toContain('await load(src, startAt);');
		expect(player).toContain('void load(src, startAt);');
		expect(player).toContain('startPosition: resumeAt > 0 ? resumeAt : -1');
	});

	test('channel refills carry the nonce and the more/less artists', () => {
		expect(store).toContain('api.getVideoChannelNext(radioChannelId, {');
		expect(store).toContain('session_nonce: radioChannelNonce,');
		expect(store).toContain('more_artist_ids: [...tasteMore],');
		expect(store).toContain('less_artist_ids: [...tasteLess],');
	});

	test('the broadcast layer rides on the dock: watch page and fullscreen, never the corner', () => {
		expect(dock).toContain("let broadcast = $derived($videoChannelOnAir !== null && (place === 'full' || place === 'expanded'));");
		expect(dock).toContain('<OnAirStage');
		expect(dock).toContain('{broadcast}');
		// One player still: the stage is an overlay, not a second <video>.
		expect((dock.match(/<VideoPlayer/g) ?? []).length).toBe(1);
		// Hidden during glides like the rest of the chrome.
		expect(dock).toContain('.up-next-pill, .onair-stage');
	});

	test('on air the player hands its title strip and up-next pill to the lower third', () => {
		expect(player).toContain('{#if !broadcast}');
		expect(player).toContain('upNextVisible && !broadcast');
		expect(player).toContain('class:below-bug={broadcast}');
	});

	test('the remote listens in the capture phase and skips menus and fields', () => {
		expect(stage).toContain("window.addEventListener('keydown', onKeydown, true);");
		expect(stage).toContain("window.removeEventListener('keydown', onKeydown, true);");
		expect(stage).toContain('if (get(contextMenu).open) return;');
		expect(stage).toContain('onAirKeyAction(event,');
		expect(stage).toContain('event.stopPropagation();');
	});

	test('flip shows the next card at once, dips, and tunes once it settles', () => {
		expect(stage).toContain('preview: (target) => showCard(target),');
		expect(stage).toContain('tune: (target) => void tuneTo(target),');
		expect(stage).toContain('animation: dip 160ms linear both;');
		expect(stage).toContain('!get(reduceMotion)');
		expect(stage).toMatch(/@media \(prefers-reduced-motion: reduce\) \{\s*\.dip \{ display: none; \}/);
	});

	test('lower third: rule, ARTIST, "Song", mono metadata, one why chip', () => {
		expect(stage).toContain('lowerThirdPhase(positionS, durationS, startedAtS)');
		expect(stage).toContain('class="lt-rule"');
		expect(stage).toMatch(/\.lt-artist \{[^}]*text-transform: uppercase;/);
		expect(stage).toMatch(/\.lt-song \{[^}]*font-family: var\(--font-display\);[^}]*font-style: italic;/);
		expect(stage).toMatch(/\.lt-meta \{[^}]*font-family: var\(--font-mono\);/);
		expect(stage).toContain('Up next');
	});

	test('the guide is a channels x time grid with a now line, driven by the stage', () => {
		expect(stage).toContain('<GuideOverlay bind:this={guide}');
		expect(stage).toContain('guide?.handleKey(event)');
		expect(guide).toContain('export function handleKey(event: KeyboardEvent): boolean');
		expect(guide).toContain('api.getVideoChannelGuide(channel.id, frame.from, frame.to)');
		expect(guide).toContain('class="now-line"');
	});

	test('the stage stays dark: white ink on its own scrims, no theme text tokens', () => {
		for (const source of [stage, guide]) {
			const css = source.slice(source.indexOf('<style>'));
			expect(css).not.toContain('var(--text-primary)');
			expect(css).not.toContain('var(--bg-');
			expect(css).not.toContain('backdrop-filter');
		}
	});
});
