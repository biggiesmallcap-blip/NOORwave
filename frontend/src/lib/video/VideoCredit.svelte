<script lang="ts">
	import { buildArtistMenu } from '$lib/player/artist_menu';
	import { buildVideoMenu } from '$lib/player/video_menu';
	import { openContextMenu } from '$lib/stores/context_menu';
	import type { VideoSessionItem } from '$lib/stores/video_session';
	import { creditFor, lowerThirdPhase, type CreditSource } from './onair/lower_third';

	// The music-TV credit (the lower third): ARTIST, "Song", year, cut and
	// runtime, and one why chip, in for the first seconds of a video and back
	// for the last ones with what is up next. On a channel it sits in the
	// stage under the bug, in the channel's colour; with titles on for all
	// videos the player shows it the same way, in the accent.
	let { current, upNext, positionS, durationS, startedAtS, hidden = false, fullscreen = false }: {
		current: VideoSessionItem | null;
		upNext: VideoSessionItem | null;
		positionS: number;
		durationS: number;
		/** Where this video started for us (a tune-in lands mid-video). */
		startedAtS: number;
		/** Something else holds the picture (the guide, a flip, the curtain). */
		hidden?: boolean;
		fullscreen?: boolean;
	} = $props();

	let phase = $derived(lowerThirdPhase(positionS, durationS, startedAtS));
	let item = $derived(phase === 'intro' ? current : phase === 'outro' ? upNext : null);
	let credit = $derived(item ? creditFor(item as CreditSource) : null);

	function artistMenu(event: MouseEvent, target: VideoSessionItem | null) {
		if (target?.artist_id == null) return;
		event.preventDefault();
		openContextMenu(event, buildArtistMenu({ tidal_id: target.artist_id, name: target.artist_name ?? '', in_library: false }, { isLocal: false }), target.artist_name ?? '');
	}

	function videoMenu(event: MouseEvent, target: VideoSessionItem | null) {
		if (!target) return;
		event.preventDefault();
		openContextMenu(event, buildVideoMenu(target), target.title);
	}
</script>

{#if credit && !hidden}
	{#key `${phase}:${item?.tidal_id}`}
		<div class="lower-third" class:outro={phase === 'outro'} class:fullscreen aria-live="polite">
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

<style>
	/* In the channel's colour on air (--ch-ink from the stage), else the
	   accent. White type over the picture in every theme. */
	.lower-third {
		--lt-ink: var(--ch-ink, var(--accent-strong));
		position: absolute;
		left: clamp(16px, 4%, 64px);
		bottom: clamp(70px, 15%, 132px);
		display: grid;
		justify-items: start;
		gap: 4px;
		max-width: min(72%, 760px);
		padding: 14px 40px 14px 0;
		color: #fff;
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
		background: var(--lt-ink);
		transform-origin: 0 50%;
		animation: lt-rule var(--motion-slow) both;
	}
	.lt-kicker {
		font-family: var(--font-mono);
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-medium);
		text-transform: uppercase;
		letter-spacing: 0.16em;
		color: var(--lt-ink);
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
		border: 1px solid color-mix(in oklch, var(--lt-ink) 55%, transparent);
		border-radius: 999px;
		background: rgba(0, 0, 0, 0.35);
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-semibold);
		color: rgba(255, 255, 255, 0.9);
		text-shadow: none;
		animation: lt-in var(--motion-slow) 260ms both;
	}
	.lower-third.fullscreen { bottom: clamp(96px, 13%, 168px); }
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

	@media (prefers-reduced-motion: reduce) {
		.lt-rule,
		.lt-kicker,
		.lt-artist,
		.lt-song,
		.lt-meta,
		.lt-why,
		.lower-third::before { animation: none; }
	}
	:global(:root[data-motion='reduce']) :is(.lt-rule, .lt-kicker, .lt-artist, .lt-song, .lt-meta, .lt-why) {
		animation: none;
	}
	:global(:root[data-motion='reduce']) .lower-third::before { animation: none; }
</style>
