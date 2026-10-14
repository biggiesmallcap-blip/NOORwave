<script lang="ts">
	import { onDestroy, onMount } from 'svelte';
	import { get } from 'svelte/store';
	import type { VideoChannel } from '$lib/api/video_explore_types';
	import { buildArtistMenu } from '$lib/player/artist_menu';
	import { buildVideoMenu } from '$lib/player/video_menu';
	import { contextMenu, openContextMenu } from '$lib/stores/context_menu';
	import { reduceMotion } from '$lib/stores/motion';
	import { steerChannel, steerMessage, type VideoSessionItem } from '$lib/stores/video_session';
	import { dispatchVideoControl } from '$lib/video/controls';
	import { channelNumber, liveSlot, secondsSinceMidnight } from './clock';
	import { createFlipper, DIGIT_WAIT_MS, digitVerdict } from './flip';
	import GuideOverlay from './GuideOverlay.svelte';
	import { onAirKeyAction, type OnAirAction } from './keymap';
	import { airTick, channelHue, channelLineup, ensureLineup } from './lineup';
	import { creditFor, lowerThirdPhase, splitTitle, type CreditSource } from './lower_third';
	import { remotePresses } from './remote';
	import { tuneChannel } from './tune';

	// The broadcast layer over the stage while a channel is on air, on the
	// watch page and in fullscreen: the channel bug, the lower third credits,
	// the flip card, typed channel numbers, the guide, and the remote's keys.
	// It never covers the picture with chrome: everything is text over the
	// video on a scrim, white in every theme.
	let { channel, current, upNext, positionS, durationS, startedAtS, fullscreen = false }: {
		channel: VideoChannel;
		current: VideoSessionItem | null;
		upNext: VideoSessionItem | null;
		positionS: number;
		durationS: number;
		/** Where this video started for us (a tune-in lands mid-video). */
		startedAtS: number;
		fullscreen?: boolean;
	} = $props();

	const FLIP_HOLD_MS = 1400;
	const NOTE_MS = 1800;

	let guideOpen = $state(false);
	let guide = $state<ReturnType<typeof GuideOverlay> | null>(null);
	let digits = $state('');
	let flipCard = $state<VideoChannel | null>(null);
	let dipKey = $state(0);
	let note = $state<{ text: string; sign: '+' | '-' | null; key: number } | null>(null);
	let digitTimer: ReturnType<typeof setTimeout> | null = null;
	let flipTimer: ReturnType<typeof setTimeout> | null = null;
	let noteTimer: ReturnType<typeof setTimeout> | null = null;

	let lineup = $derived($channelLineup);
	let channels = $derived<readonly VideoChannel[]>(lineup?.data.channels.length ? lineup.data.channels : [channel]);
	let elapsed = $derived(lineup ? Math.max(0, ($airTick - lineup.fetchedAt) / 1000) : 0);
	let nowS = $derived(secondsSinceMidnight(new Date($airTick)));
	let hue = $derived(channelHue(channel));

	let phase = $derived(lowerThirdPhase(positionS, durationS, startedAtS));
	let introCredit = $derived(current ? creditFor(current as CreditSource) : null);
	let outroCredit = $derived(upNext ? creditFor(upNext as CreditSource) : null);
	let credit = $derived(phase === 'intro' ? introCredit : phase === 'outro' ? outroCredit : null);
	let showCredit = $derived(Boolean(credit) && !guideOpen && !flipCard);

	let flipSlot = $derived(flipCard ? liveSlot(flipCard, elapsed) : null);
	let flipVideo = $derived(flipSlot?.program?.video ?? null);

	const flipper = createFlipper<VideoChannel>({
		channels: () => channels,
		currentNo: () => channel.no,
		preview: (target) => showCard(target),
		tune: (target) => void tuneTo(target),
	});

	function showCard(target: VideoChannel) {
		if (flipTimer) clearTimeout(flipTimer);
		flipTimer = null;
		const changing = target.id !== flipCard?.id;
		flipCard = target;
		if (changing && target.id !== channel.id && !get(reduceMotion)) dipKey += 1;
	}

	function holdCardThenHide(target: VideoChannel) {
		if (flipTimer) clearTimeout(flipTimer);
		flipTimer = setTimeout(() => {
			flipTimer = null;
			if (flipCard?.id === target.id) flipCard = null;
		}, FLIP_HOLD_MS);
	}

	async function tuneTo(target: VideoChannel) {
		guideOpen = false;
		if (target.id === channel.id) {
			holdCardThenHide(target);
			return;
		}
		await tuneChannel(target);
		holdCardThenHide(target);
	}

	function say(text: string, sign: '+' | '-' | null = null) {
		if (noteTimer) clearTimeout(noteTimer);
		note = { text, sign, key: (note?.key ?? 0) + 1 };
		noteTimer = setTimeout(() => {
			noteTimer = null;
			note = null;
		}, NOTE_MS);
	}

	function commitDigits() {
		if (digitTimer) clearTimeout(digitTimer);
		digitTimer = null;
		const typed = digits;
		digits = '';
		if (!typed) return;
		const target = channels.find((candidate) => candidate.no === Number(typed));
		if (!target) {
			say(`No channel ${channelNumber(Number(typed))}`);
			return;
		}
		if (target.locked) {
			say(`${channelNumber(target.no)} ${target.title} is coming up`);
			return;
		}
		flipper.go(target);
	}

	function typeDigit(digit: number) {
		if (digitTimer) clearTimeout(digitTimer);
		digitTimer = null;
		digits = `${digits}${digit}`.slice(-3);
		const verdict = digitVerdict(digits, channels.map((candidate) => candidate.no));
		if (verdict === 'commit') commitDigits();
		else if (verdict === 'invalid') {
			say(`No channel ${digits}`);
			digits = '';
		} else digitTimer = setTimeout(commitDigits, DIGIT_WAIT_MS);
	}

	function teach(more: boolean) {
		const artistId = current?.artist_id ?? null;
		const name = current?.artist_name ?? 'this artist';
		if (artistId == null) {
			say('Nothing to learn from this one');
			return;
		}
		void steerChannel(artistId, current?.artist_name ?? null, more).then((result) =>
			say(steerMessage(name, more, result), more ? '+' : '-')
		);
	}

	function run(action: OnAirAction) {
		switch (action.kind) {
			case 'flip':
				flipper.press(action.step);
				break;
			case 'digit':
				typeDigit(action.digit);
				break;
			case 'guide':
				guideOpen = !guideOpen;
				if (guideOpen) void ensureLineup();
				break;
			case 'taste':
				teach(action.more);
				break;
			case 'volume':
				dispatchVideoControl(action.up ? 'volume-up' : 'volume-down');
				break;
			case 'close':
				if (guideOpen) guideOpen = false;
				else if (digits) {
					digits = '';
					if (digitTimer) clearTimeout(digitTimer);
					digitTimer = null;
				}
				break;
		}
	}

	// Capture phase on window: the remote answers before the player's and the
	// app's own shortcuts, and only takes the keys it uses.
	function onKeydown(event: KeyboardEvent) {
		if (get(contextMenu).open) return;
		// A button outside the guide (a pill, the player's controls) keeps its
		// own Enter and Space.
		const target = event.target instanceof Element ? event.target : null;
		const ownKeys = (event.key === 'Enter' || event.key === ' ') && target?.closest('button, a, [role="button"]') && !target.closest('.guide');
		if (guideOpen && !ownKeys && guide?.handleKey(event)) {
			event.preventDefault();
			event.stopPropagation();
			return;
		}
		const action = onAirKeyAction(event, { guideOpen, overlayOpen: guideOpen || digits.length > 0 });
		if (!action) return;
		event.preventDefault();
		event.stopPropagation();
		run(action);
	}

	function artistMenu(event: MouseEvent, item: VideoSessionItem | null) {
		if (item?.artist_id == null) return;
		event.preventDefault();
		openContextMenu(event, buildArtistMenu({ tidal_id: item.artist_id, name: item.artist_name ?? '', in_library: false }, { isLocal: false }), item.artist_name ?? '');
	}

	function videoMenu(event: MouseEvent, item: VideoSessionItem | null) {
		if (!item) return;
		event.preventDefault();
		openContextMenu(event, buildVideoMenu(item), item.title);
	}

	// The on-screen remote (the fullscreen channel pill) presses the same
	// actions as the keys. A press made before this stage mounted is old.
	let handledPress = get(remotePresses)?.seq ?? 0;
	const unsubscribePresses = remotePresses.subscribe((press) => {
		if (!press || press.seq === handledPress) return;
		handledPress = press.seq;
		run(press.action);
	});

	onMount(() => {
		void ensureLineup();
		window.addEventListener('keydown', onKeydown, true);
	});

	onDestroy(() => {
		if (typeof window !== 'undefined') window.removeEventListener('keydown', onKeydown, true);
		unsubscribePresses();
		flipper.cancel();
		for (const timer of [digitTimer, flipTimer, noteTimer]) if (timer) clearTimeout(timer);
	});
</script>

<div class="onair-stage" class:fullscreen style:--ch-hue={hue}>
	<!-- The bug: number and name, top right, quiet like a broadcaster's mark.
	     While digits are typed it shows them instead. -->
	<div class="bug" class:typing={digits.length > 0} class:hidden={guideOpen} aria-live="polite">
		{#if digits}
			<span class="bug-no" aria-label={`Channel ${digits}`}>{digits.padEnd(2, '_')}</span>
		{:else}
			<span class="bug-no">{channelNumber(channel.no)}</span>
			<span class="bug-name">{channel.title}</span>
		{/if}
	</div>

	{#if showCredit && credit}
		{#key `${phase}:${phase === 'intro' ? current?.tidal_id : upNext?.tidal_id}`}
			{@const item = phase === 'intro' ? current : upNext}
			<div class="lower-third" class:outro={phase === 'outro'} aria-live="polite">
				<span class="lt-rule" aria-hidden="true"></span>
				{#if phase === 'outro'}<span class="lt-kicker">Up next</span>{/if}
				<span class="lt-artist" role="presentation" oncontextmenu={(event) => artistMenu(event, item)}>{credit.artist}</span>
				<span class="lt-song" role="presentation" oncontextmenu={(event) => videoMenu(event, item)}>{credit.song}</span>
				<span class="lt-meta">
					{#if credit.year}<span>{credit.year}</span>{/if}
					{#if credit.tag}<span>{credit.tag}</span>{:else if credit.cut}<span>{credit.cut}</span>{/if}
					{#if credit.runtime}<span>{credit.runtime}</span>{/if}
				</span>
				{#if credit.why && phase === 'intro'}<span class="lt-why">{credit.why}</span>{/if}
			</div>
		{/key}
	{/if}

	{#if flipCard}
		<div class="flip-card" style:--ch-hue={channelHue(flipCard)} aria-live="assertive">
			<span class="fc-no">{channelNumber(flipCard.no)}</span>
			<div class="fc-text">
				<span class="fc-name">{flipCard.title}</span>
				{#if flipVideo}
					<span class="fc-now">
						<span class="fc-song">{splitTitle(flipVideo.title).song}</span>
						{#if flipVideo.artist_name}<span class="fc-artist">{flipVideo.artist_name}</span>{/if}
					</span>
				{:else if flipCard.subtitle}
					<span class="fc-sub">{flipCard.subtitle}</span>
				{/if}
				{#if flipSlot?.program}
					<span class="fc-bar" aria-hidden="true"><span style:transform={`scaleX(${flipSlot.progress})`}></span></span>
				{/if}
			</div>
		</div>
	{/if}

	{#if note}
		{#key note.key}
			<div class="note" role="status">
				{#if note.sign}<span class="note-sign">{note.sign}</span>{/if}
				{note.text}
			</div>
		{/key}
	{/if}

	{#key dipKey}
		{#if dipKey > 0}<span class="dip" aria-hidden="true"></span>{/if}
	{/key}

	{#if guideOpen}
		<GuideOverlay bind:this={guide} {channels} currentId={channel.id} {nowS} ontune={(target) => flipper.go(target)} onclose={() => (guideOpen = false)} clearBottom={fullscreen ? 104 : 64} />
	{/if}
</div>

<style>
	/* Over the player, under nothing: pointer events pass through to the
	   video except on the few things that take a right-click or a click. */
	.onair-stage {
		--ch-ink: oklch(78% 0.14 calc(var(--ch-hue) * 1deg));
		position: absolute;
		inset: 0;
		z-index: 5;
		overflow: hidden;
		pointer-events: none;
		color: #fff;
		border-radius: inherit;
	}

	/* ---- Bug ---- */
	.bug {
		position: absolute;
		top: 16px;
		right: 18px;
		display: grid;
		justify-items: end;
		gap: 2px;
		opacity: 0.82;
		text-shadow: 0 1px 10px rgba(0, 0, 0, 0.7);
		transition: opacity var(--motion-base);
	}
	.bug.typing { opacity: 1; }
	.bug.hidden { opacity: 0; }
	.bug-no {
		font-family: var(--font-mono);
		font-size: var(--font-size-lg);
		font-weight: var(--font-weight-semibold);
		line-height: 1;
		font-variant-numeric: tabular-nums;
		color: var(--ch-ink);
	}
	/* Typed digits take the bug's whole height (number and name), so the
	   fullscreen pills under it never move. */
	.bug.typing .bug-no {
		font-size: var(--font-size-2xl);
		color: #fff;
	}
	/* Long names wrap to a second line, right aligned, instead of losing
	   their end. */
	.bug-name {
		max-width: 30ch;
		text-align: right;
		text-wrap: balance;
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-semibold);
		text-transform: uppercase;
		letter-spacing: 0.12em;
		color: rgba(255, 255, 255, 0.86);
	}
	.fullscreen .bug { top: 22px; right: 28px; }
	.fullscreen .bug-no { font-size: var(--font-size-xl); }

	/* ---- Lower third: the music-TV credit ---- */
	.lower-third {
		position: absolute;
		left: clamp(16px, 4%, 64px);
		bottom: clamp(70px, 15%, 132px);
		display: grid;
		justify-items: start;
		gap: 4px;
		max-width: min(72%, 760px);
		padding: 14px 40px 14px 0;
		/* Readable on any picture: a soft shadow under the type, no box. */
		text-shadow: 0 1px 2px rgba(0, 0, 0, 0.55), 0 2px 24px rgba(0, 0, 0, 0.6);
	}
	.lower-third::before {
		content: '';
		position: absolute;
		inset: -70px -160px -80px -140px;
		z-index: -1;
		/* closest-side: the shade is gone before the box edge, so it never
		   shows a seam on the picture. */
		background: radial-gradient(closest-side, rgba(0, 0, 0, 0.55), rgba(0, 0, 0, 0.22) 60%, rgba(0, 0, 0, 0));
		animation: lt-fade var(--motion-slow) both;
	}
	.lt-rule {
		width: 44px;
		height: 3px;
		margin-bottom: 6px;
		border-radius: 2px;
		background: var(--ch-ink);
		transform-origin: 0 50%;
		animation: lt-rule var(--motion-slow) both;
	}
	.lt-kicker {
		font-family: var(--font-mono);
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-medium);
		text-transform: uppercase;
		letter-spacing: 0.16em;
		color: var(--ch-ink);
		animation: lt-in var(--motion-slow) 40ms both;
	}
	.lt-artist {
		max-width: 100%;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-family: var(--font-body);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-bold);
		text-transform: uppercase;
		letter-spacing: 0.14em;
		pointer-events: auto;
		animation: lt-in var(--motion-slow) 80ms both;
	}
	/* The title wraps (two lines at most, balanced) rather than cutting off
	   mid-word. */
	.lt-song {
		max-width: 100%;
		display: -webkit-box;
		-webkit-line-clamp: 2;
		line-clamp: 2;
		-webkit-box-orient: vertical;
		overflow: hidden;
		text-wrap: balance;
		font-family: var(--font-display);
		font-size: var(--font-size-2xl);
		font-style: italic;
		font-weight: var(--font-weight-medium);
		line-height: var(--line-height-tight);
		pointer-events: auto;
		animation: lt-in var(--motion-slow) 140ms both;
	}
	.lt-song::before { content: '\201C'; }
	.lt-song::after { content: '\201D'; }
	.lt-meta {
		display: flex;
		flex-wrap: wrap;
		align-items: baseline;
		margin-top: 2px;
		font-family: var(--font-mono);
		font-size: var(--font-size-xs);
		font-variant-numeric: tabular-nums;
		color: rgba(255, 255, 255, 0.78);
		animation: lt-in var(--motion-slow) 200ms both;
	}
	.lt-meta span + span::before {
		content: '/';
		margin: 0 10px;
		color: rgba(255, 255, 255, 0.4);
	}
	.lt-why {
		margin-top: 6px;
		padding: 4px 10px;
		border: 1px solid color-mix(in oklch, var(--ch-ink) 55%, transparent);
		border-radius: 999px;
		background: rgba(0, 0, 0, 0.35);
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-semibold);
		color: rgba(255, 255, 255, 0.9);
		text-shadow: none;
		animation: lt-in var(--motion-slow) 260ms both;
	}
	.fullscreen .lower-third { bottom: clamp(96px, 13%, 168px); }
	.fullscreen .lt-artist { font-size: var(--font-size-md); }
	.fullscreen .lt-song { font-size: var(--font-size-3xl); }
	.fullscreen .lt-meta { font-size: var(--font-size-sm); }
	.fullscreen .lt-why { font-size: var(--font-size-xs); }

	@keyframes lt-rule {
		from { transform: scaleX(0); }
		to { transform: scaleX(1); }
	}
	@keyframes lt-in {
		from { opacity: 0; transform: translateX(-18px); }
		to { opacity: 1; transform: none; }
	}
	@keyframes lt-fade {
		from { opacity: 0; }
		to { opacity: 1; }
	}

	/* ---- Flip card: where you are going, shown at once ---- */
	.flip-card {
		--fc-ink: oklch(78% 0.14 calc(var(--ch-hue) * 1deg));
		position: absolute;
		left: clamp(16px, 4%, 64px);
		top: clamp(16px, 7%, 72px);
		display: grid;
		grid-template-columns: auto minmax(0, 1fr);
		align-items: start;
		gap: 18px;
		width: min(560px, calc(100% - 32px));
		padding: 18px 22px 18px 18px;
		border-top: 3px solid var(--fc-ink);
		border-radius: 0 0 var(--radius-sm) var(--radius-sm);
		background: rgba(8, 8, 12, 0.88);
		box-shadow: 0 18px 50px rgba(0, 0, 0, 0.5);
		animation: card-in var(--motion-fast) both;
	}
	@keyframes card-in {
		from { opacity: 0; transform: translateX(-10px); }
		to { opacity: 1; transform: none; }
	}
	.fc-no {
		font-family: var(--font-mono);
		font-size: var(--font-size-4xl);
		font-weight: var(--font-weight-semibold);
		line-height: 1;
		font-variant-numeric: tabular-nums;
		letter-spacing: -0.03em;
		color: var(--fc-ink);
	}
	.fc-text {
		display: grid;
		gap: 4px;
		min-width: 0;
		padding-top: 2px;
	}
	.fc-name {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-bold);
		text-transform: uppercase;
		letter-spacing: 0.14em;
	}
	.fc-now {
		display: grid;
		min-width: 0;
	}
	.fc-song,
	.fc-artist,
	.fc-sub {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.fc-song {
		font-family: var(--font-display);
		font-size: var(--font-size-xl);
		font-style: italic;
		font-weight: var(--font-weight-medium);
		line-height: var(--line-height-snug);
	}
	.fc-artist,
	.fc-sub {
		font-size: var(--font-size-xs);
		color: rgba(255, 255, 255, 0.7);
	}
	.fc-bar {
		position: relative;
		height: 3px;
		margin-top: 8px;
		border-radius: 2px;
		overflow: hidden;
		background: rgba(255, 255, 255, 0.14);
	}
	.fc-bar span {
		position: absolute;
		inset: 0;
		background: var(--fc-ink);
		transform-origin: 0 50%;
	}

	/* ---- +/- confirmation ---- */
	.note {
		position: absolute;
		top: 18px;
		left: 50%;
		display: inline-flex;
		align-items: center;
		gap: 8px;
		max-width: min(520px, calc(100% - 200px));
		padding: 8px 16px 8px 10px;
		border-radius: 999px;
		background: rgba(8, 8, 12, 0.86);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		transform: translateX(-50%);
		animation: note-in var(--motion-base) both;
	}
	.note-sign {
		display: grid;
		place-items: center;
		width: 20px;
		height: 20px;
		border-radius: 50%;
		background: var(--ch-ink);
		color: #0b0b0e;
		font-family: var(--font-mono);
		font-weight: var(--font-weight-bold);
		line-height: 1;
	}
	@keyframes note-in {
		from { opacity: 0; transform: translate(-50%, -8px); }
		to { opacity: 1; transform: translate(-50%, 0); }
	}

	/* ---- The flip's luminance dip: 160 ms down and back up ---- */
	.dip {
		position: absolute;
		inset: 0;
		background: #000;
		opacity: 0;
		animation: dip 160ms linear both;
	}
	@keyframes dip {
		0% { opacity: 0; }
		45% { opacity: 0.88; }
		100% { opacity: 0; }
	}

	/* Reduced motion: no dip, no slides; things simply appear. */
	@media (prefers-reduced-motion: reduce) {
		.dip { display: none; }
		.lt-rule,
		.lt-kicker,
		.lt-artist,
		.lt-song,
		.lt-meta,
		.lt-why,
		.lower-third::before,
		.flip-card,
		.note { animation: none; }
	}
	:global(:root[data-motion='reduce']) .dip { display: none; }
	:global(:root[data-motion='reduce']) :is(.lt-rule, .lt-kicker, .lt-artist, .lt-song, .lt-meta, .lt-why, .flip-card, .note) {
		animation: none;
	}
	:global(:root[data-motion='reduce']) .lower-third::before { animation: none; }
</style>
