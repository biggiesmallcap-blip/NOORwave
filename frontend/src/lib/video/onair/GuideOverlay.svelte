<script lang="ts" module>
	import type { ChannelProgram } from '$lib/api/video_explore_types';
	// Guide rows already fetched, per channel and window start, so reopening
	// the guide (G, G) draws at once.
	const guideCache = new Map<string, { at: number; programs: ChannelProgram[] }>();
	const GUIDE_TTL_MS = 5 * 60_000;
</script>

<script lang="ts">
	import { onDestroy, onMount, tick, untrack } from 'svelte';
	import { api } from '$lib/api/client';
	import type { VideoChannel } from '$lib/api/video_explore_types';
	import { buildVideoMenu } from '$lib/player/video_menu';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { channelNumber, formatClock, progressOf } from './clock';
	import { dialOrder } from './flip';
	import { guideTicks, guideWindow, lineupPrograms, moveGuideFocus, placeProgram, programAt, type GuideFocus } from './guide';
	import { channelHue } from './lineup';
	import { splitTitle } from './lower_third';

	// The guide on the stage (G): channels down, time across, each program a
	// block in its channel's colour, a line at now. Arrows move, Enter tunes
	// the focused channel. It answers keys through handleKey, which the stage
	// calls before anything else sees them, so it works wherever focus is.
	let { channels, currentId, nowS, ontune, onclose, clearBottom = 0 }: {
		channels: readonly VideoChannel[];
		currentId: string | null;
		/** Live seconds since local midnight. */
		nowS: number;
		ontune: (channel: VideoChannel) => void;
		/** The close button, or a click on the margin around the grid. */
		onclose: () => void;
		/** Px left clear at the bottom: the player's controls (and in
		 *  fullscreen the heart, Queue and channel pills) stay usable. */
		clearBottom?: number;
	} = $props();

	const CONCURRENCY = 4;

	let rows = $derived(dialOrder(channels));
	// The window is fixed while the guide is open; the now line moves.
	const frame = untrack(() => guideWindow(nowS));
	const ticks = guideTicks(frame.from, frame.to);
	let fetched = $state<Record<string, ChannelProgram[]>>({});
	let programsByRow = $derived(rows.map((channel) => fetched[channel.id] ?? lineupPrograms(channel)));
	let focus = $state<GuideFocus>({ row: 0, time: untrack(() => nowS) });
	let nowFrac = $derived(Math.min(1, Math.max(0, (nowS - frame.from) / (frame.to - frame.from))));
	let body = $state<HTMLDivElement | null>(null);
	let live = true;

	function cacheKey(id: string): string {
		return `${id}@${frame.from}`;
	}

	/** Fetch rows nearest the focused one first, a few at a time. */
	async function fetchRows() {
		const order = rows
			.map((channel, index) => ({ channel, distance: Math.abs(index - focus.row) }))
			.sort((a, b) => a.distance - b.distance)
			.map(({ channel }) => channel);
		const pending = order.filter((channel) => {
			const hit = guideCache.get(cacheKey(channel.id));
			if (hit && Date.now() - hit.at < GUIDE_TTL_MS) {
				fetched[channel.id] = hit.programs;
				return false;
			}
			return true;
		});
		const worker = async () => {
			while (live && pending.length > 0) {
				const channel = pending.shift()!;
				try {
					const guide = await api.getVideoChannelGuide(channel.id, frame.from, frame.to);
					if (!live) return;
					const programs = [...guide.programs].sort((a, b) => a.start_s - b.start_s);
					guideCache.set(cacheKey(channel.id), { at: Date.now(), programs });
					fetched[channel.id] = programs;
				} catch {
					// The row keeps its now and next from the lineup.
				}
			}
		};
		await Promise.all(Array.from({ length: CONCURRENCY }, worker));
	}

	function focusedIndex(row: number): number {
		return programAt(programsByRow[row] ?? [], focus.time);
	}

	async function revealFocus() {
		await tick();
		const el = body?.querySelector<HTMLElement>('[data-focused="true"]');
		el?.focus({ preventScroll: true });
		el?.scrollIntoView({ block: 'nearest', inline: 'nearest' });
	}

	/** Keys for the grid. True when it took the key. */
	export function handleKey(event: KeyboardEvent): boolean {
		if (event.ctrlKey || event.metaKey || event.altKey) return false;
		if (event.key === 'Enter' || event.key === ' ') {
			const channel = rows[focus.row];
			if (channel) ontune(channel);
			return true;
		}
		const next = moveGuideFocus(focus, event.key, programsByRow, frame);
		if (!next) return false;
		focus = next;
		void revealFocus();
		return true;
	}

	function pick(row: number, program: ChannelProgram) {
		focus = { row, time: Math.max(frame.from, program.start_s) + 1 };
		const channel = rows[row];
		if (channel) ontune(channel);
	}

	function programMenu(event: MouseEvent, program: ChannelProgram) {
		event.preventDefault();
		openContextMenu(event, buildVideoMenu(program.video), program.video.title);
	}

	onMount(() => {
		const current = rows.findIndex((channel) => channel.id === currentId);
		focus = { row: Math.max(0, current), time: nowS };
		void revealFocus();
		void fetchRows();
	});

	onDestroy(() => {
		live = false;
	});
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<div
	class="guide"
	role="dialog"
	aria-label="Channel guide"
	style:bottom={`${clearBottom}px`}
	tabindex="-1"
	onclick={(event) => {
		// The margin around the grid closes it; anything inside does its own thing.
		if (event.target === event.currentTarget) onclose();
	}}
>
	<header class="head">
		<span class="title">Guide</span>
		<span class="keys">
			<kbd>&uarr;</kbd><kbd>&darr;</kbd> channels <kbd>&larr;</kbd><kbd>&rarr;</kbd> time <kbd>Enter</kbd> tune <kbd>Esc</kbd> close
		</span>
		<button type="button" class="close" aria-label="Close the guide" title="Close (Esc)" onclick={onclose}>
			<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M6 6l12 12M18 6L6 18" /></svg>
		</button>
	</header>

	<div class="ruler" aria-hidden="true">
		<span class="corner">{formatClock(nowS)}</span>
		<div class="ticks">
			{#each ticks as t (t)}
				<span class="tick" style:left={`${((t - frame.from) / (frame.to - frame.from)) * 100}%`}>{formatClock(t)}</span>
			{/each}
		</div>
	</div>

	<div class="tracks" style:--now={nowFrac}>
	<div class="body" bind:this={body}>
		{#each rows as channel, row (channel.id)}
			{@const programs = programsByRow[row]}
			{@const focusIndex = row === focus.row ? focusedIndex(row) : -1}
			<div class="row" class:current={channel.id === currentId} class:focus-row={row === focus.row} style:--ch-hue={channelHue(channel)}>
				<div class="ident">
					<span class="no">{channelNumber(channel.no)}</span>
					<span class="name">{channel.title}</span>
				</div>
				<div class="track">
					{#each programs as program, index (program.slot + ':' + program.start_s)}
						{@const box = placeProgram(program, frame.from, frame.to)}
						{#if box}
							{@const ended = program.start_s + program.duration_s <= nowS}
							{@const airing = program.start_s <= nowS && !ended}
							<button
								type="button"
								class="block"
								class:ended
								class:airing
								data-focused={index === focusIndex}
								tabindex={index === focusIndex ? 0 : -1}
								style:left={`${box.left * 100}%`}
								style:width={`${box.width * 100}%`}
								aria-label={`${channel.title}, ${formatClock(program.start_s)}: ${program.video.title}${program.video.artist_name ? ` by ${program.video.artist_name}` : ''}`}
								onclick={() => pick(row, program)}
								oncontextmenu={(event) => programMenu(event, program)}
							>
								<span class="b-song">{splitTitle(program.video.title).song}</span>
								{#if program.video.artist_name}<span class="b-artist">{program.video.artist_name}</span>{/if}
								{#if airing}
									<span class="b-bar" aria-hidden="true"><span style:transform={`scaleX(${progressOf(nowS - program.start_s, program.duration_s)})`}></span></span>
								{/if}
							</button>
						{/if}
					{/each}
				</div>
			</div>
		{/each}
	</div>
	<span class="now-line" aria-hidden="true"></span>
	</div>
</div>

<style>
	/* The stage is dark in every theme, so the guide carries its own ink. */
	.guide {
		--ident-w: clamp(150px, 18%, 220px);
		position: absolute;
		inset: 0;
		display: grid;
		grid-template-rows: auto auto minmax(0, 1fr);
		padding: clamp(14px, 2.4%, 28px) clamp(14px, 3%, 40px);
		background: rgba(5, 5, 8, 0.93);
		color: rgba(255, 255, 255, 0.94);
		pointer-events: auto;
		animation: guide-in var(--motion-base) both;
	}
	@keyframes guide-in {
		from { opacity: 0; }
		to { opacity: 1; }
	}

	.head {
		display: flex;
		align-items: center;
		gap: var(--space-3);
		padding-bottom: var(--space-3);
	}
	.title {
		font-size: var(--font-size-label);
		font-weight: var(--font-weight-semibold);
		text-transform: uppercase;
		letter-spacing: 0.12em;
		color: rgba(255, 255, 255, 0.72);
	}
	.keys {
		margin-left: auto;
		font-size: var(--font-size-2xs);
		color: rgba(255, 255, 255, 0.5);
		white-space: nowrap;
	}
	/* Big enough to hit without aiming. */
	.close {
		display: grid;
		place-items: center;
		width: 36px;
		height: 36px;
		margin: -6px -6px -6px 0;
		padding: 0;
		border: 1px solid rgba(255, 255, 255, 0.16);
		border-radius: 50%;
		background: rgba(255, 255, 255, 0.08);
		color: rgba(255, 255, 255, 0.84);
		cursor: pointer;
		transition: background var(--motion-fast), color var(--motion-fast);
	}
	.close:hover {
		background: rgba(255, 255, 255, 0.18);
		color: #fff;
	}
	.close:focus-visible {
		outline: 2px solid #fff;
		outline-offset: 2px;
	}
	.close svg {
		width: 16px;
		height: 16px;
		fill: none;
		stroke: currentColor;
		stroke-width: 2;
		stroke-linecap: round;
	}
	kbd {
		display: inline-block;
		min-width: 18px;
		margin: 0 2px;
		padding: 1px 5px;
		border: 1px solid rgba(255, 255, 255, 0.2);
		border-radius: 4px;
		font-family: var(--font-mono);
		font-size: var(--font-size-2xs);
		text-align: center;
		color: rgba(255, 255, 255, 0.78);
	}

	.ruler {
		display: grid;
		grid-template-columns: var(--ident-w) minmax(0, 1fr);
		height: 24px;
		border-bottom: 1px solid rgba(255, 255, 255, 0.1);
	}
	.corner {
		font-family: var(--font-mono);
		font-size: var(--font-size-xs);
		font-variant-numeric: tabular-nums;
		color: rgba(255, 255, 255, 0.9);
	}
	.ticks { position: relative; }
	.tick {
		position: absolute;
		top: 0;
		padding-left: 6px;
		border-left: 1px solid rgba(255, 255, 255, 0.16);
		font-family: var(--font-mono);
		font-size: var(--font-size-2xs);
		font-variant-numeric: tabular-nums;
		color: rgba(255, 255, 255, 0.56);
		line-height: 1;
		padding-top: 6px;
		height: 24px;
	}

	.tracks {
		position: relative;
		min-height: 0;
	}
	.body {
		height: 100%;
		overflow-y: auto;
		overscroll-behavior: contain;
		/* No scrollbar: the tracks must stay as wide as the ruler above them. */
		scrollbar-width: none;
	}
	.row {
		--ch-ink: oklch(76% 0.14 calc(var(--ch-hue) * 1deg));
		--ch-fill: oklch(30% 0.06 calc(var(--ch-hue) * 1deg));
		display: grid;
		grid-template-columns: var(--ident-w) minmax(0, 1fr);
		height: 54px;
		border-bottom: 1px solid rgba(255, 255, 255, 0.06);
	}
	.ident {
		display: flex;
		align-items: center;
		gap: 10px;
		min-width: 0;
		padding-right: 12px;
	}
	.no {
		flex: 0 0 auto;
		font-family: var(--font-mono);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-medium);
		font-variant-numeric: tabular-nums;
		color: var(--ch-ink);
	}
	.name {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		color: rgba(255, 255, 255, 0.82);
	}
	.current .name { color: #fff; }
	.current .ident { box-shadow: inset 3px 0 0 var(--ch-ink); padding-left: 10px; }

	.track {
		position: relative;
		min-width: 0;
	}
	.block {
		position: absolute;
		top: 5px;
		bottom: 5px;
		display: grid;
		align-content: center;
		gap: 1px;
		min-width: 0;
		padding: 0 10px;
		overflow: hidden;
		border: 0;
		border-left: 2px solid var(--ch-ink);
		border-radius: 0 var(--radius-xs) var(--radius-xs) 0;
		background: var(--ch-fill);
		color: rgba(255, 255, 255, 0.92);
		text-align: left;
		cursor: pointer;
		transition: background var(--motion-fast);
	}
	/* A hair of ground between back-to-back blocks. */
	.block { box-shadow: inset -2px 0 0 rgba(5, 5, 8, 0.93); }
	.block:hover { background: oklch(38% 0.08 calc(var(--ch-hue) * 1deg)); }
	.block.ended { opacity: 0.45; }
	.block.airing { background: oklch(36% 0.08 calc(var(--ch-hue) * 1deg)); }
	.block[data-focused='true'] {
		background: oklch(44% 0.1 calc(var(--ch-hue) * 1deg));
		outline: 2px solid #fff;
		outline-offset: -2px;
	}
	.block:focus-visible {
		outline: 2px solid #fff;
		outline-offset: -2px;
	}
	.b-song,
	.b-artist {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		line-height: var(--line-height-snug);
	}
	.b-song {
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
	}
	.b-artist {
		font-size: var(--font-size-2xs);
		color: rgba(255, 255, 255, 0.66);
	}
	.b-bar {
		position: absolute;
		left: 0;
		right: 0;
		bottom: 0;
		height: 2px;
		background: rgba(255, 255, 255, 0.12);
	}
	.b-bar span {
		position: absolute;
		inset: 0;
		background: var(--ch-ink);
		transform-origin: 0 50%;
	}

	/* Now: one line down the whole grid, past the channel names. */
	.now-line {
		position: absolute;
		top: 0;
		bottom: 0;
		left: calc(var(--ident-w) + (100% - var(--ident-w)) * var(--now));
		width: 2px;
		margin-left: -1px;
		background: #fff;
		box-shadow: 0 0 8px rgba(255, 255, 255, 0.6);
		pointer-events: none;
	}

	@media (max-width: 720px) {
		.keys { display: none; }
	}
	@media (prefers-reduced-motion: reduce) {
		.guide { animation: none; }
	}
</style>
