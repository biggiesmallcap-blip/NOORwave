<script lang="ts">
	import type { VideoChannel } from '$lib/api/video_explore_types';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import { buildArtistMenu } from '$lib/player/artist_menu';
	import { buildVideoMenu } from '$lib/player/video_menu';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { LONG_FORM_S, channelNumber, formatClock, formatRuntime, unlockProgress, type LiveSlot } from './clock';
	import { channelHue } from './lineup';
	import { splitTitle } from './lower_third';

	// One line of the dial: number, name, what is on now (with a live bar) and
	// the two that follow. The row is the action: anything on it tunes in.
	// Locked channels show how close they are to opening instead.
	let { channel, slot, onAir = false, busy = false, rise = 0, ontune }: {
		channel: VideoChannel;
		slot: LiveSlot;
		onAir?: boolean;
		busy?: boolean;
		rise?: number;
		ontune: (channel: VideoChannel) => void;
	} = $props();

	let video = $derived(slot.program?.video ?? null);
	let upcoming = $derived([slot.next, slot.then].filter((program) => program != null));
	let left = $derived(slot.program ? Math.max(0, slot.program.duration_s - slot.offset) : 0);
	// Long form says when it ends: "until 15:03" reads apart from a song.
	let longForm = $derived(slot.program != null && slot.program.duration_s >= LONG_FORM_S);

	function tune() {
		if (!channel.locked) ontune(channel);
	}

	function videoMenu(event: MouseEvent, target: typeof video) {
		if (!target) return;
		event.preventDefault();
		openContextMenu(event, buildVideoMenu(target), target.title);
	}

	function artistMenu(event: MouseEvent) {
		if (video?.artist_id == null) return;
		event.preventDefault();
		openContextMenu(event, buildArtistMenu({ tidal_id: video.artist_id, name: video.artist_name ?? '', in_library: false }, { isLocal: false }), video.artist_name ?? '');
	}
</script>

<article
	class="row rise-in-card"
	class:on-air={onAir}
	class:locked={channel.locked}
	style:--ch-hue={channelHue(channel)}
	style:--rise-index={rise}
	aria-busy={busy}
	aria-label={`Channel ${channel.no}, ${channel.title}${channel.locked ? ', coming up' : ''}`}
>
	<span class="no" aria-hidden="true">{channelNumber(channel.no)}</span>

	<div class="ident">
		<button type="button" class="name" disabled={channel.locked} onclick={tune}>{channel.title}</button>
		<span class="subtitle" title={channel.subtitle}>{channel.subtitle}</span>
	</div>

	{#if channel.locked}
		<div class="coming">
			<span class="coming-label">Coming up</span>
			<span class="unlock" aria-hidden="true"><span class="unlock-fill" style:transform={`scaleX(${unlockProgress(channel.unlock)})`}></span></span>
			{#if channel.unlock}
				<span class="unlock-meta">{Math.floor(channel.unlock.hours)} of {Math.round(channel.unlock.need_hours)} hours</span>
			{/if}
		</div>
	{:else}
		<div class="now">
			<button
				type="button"
				class="thumb"
				onclick={tune}
				oncontextmenu={(event) => videoMenu(event, video)}
				aria-label={video ? `Tune in to ${channel.title}: ${video.title}` : `Tune in to ${channel.title}`}
			>
				<ArtworkImage src={video?.artwork_url ?? null} size={320} fallbackText="" decorative fadeIn />
				<span class="bar" aria-hidden="true"><span class="fill" style:transform={`scaleX(${slot.progress})`}></span></span>
			</button>
			<div class="now-text">
				{#if video}
					<button type="button" class="song" onclick={tune} oncontextmenu={(event) => videoMenu(event, video)} tabindex="-1">{splitTitle(video.title).song}</button>
					<span class="sub">
						{#if video.artist_id != null}
							<a class="artist" href={`/tidal/artists/${video.artist_id}`} oncontextmenu={artistMenu}>{video.artist_name}</a>
						{:else if video.artist_name}
							<span class="artist">{video.artist_name}</span>
						{/if}
						{#if longForm && slot.program}
							<span class="left">until {formatClock(slot.program.start_s + slot.program.duration_s)}</span>
						{:else if left > 0}<span class="left">{formatRuntime(left)} left</span>{/if}
					</span>
				{:else}
					<span class="quiet">Off air</span>
				{/if}
			</div>
		</div>

		{#each [0, 1] as n (n)}
			{@const next = upcoming[n] ?? null}
			<button
				type="button"
				class="next"
				class:then={n === 1}
				onclick={tune}
				oncontextmenu={(event) => videoMenu(event, next?.video ?? null)}
				tabindex="-1"
				disabled={!next}
			>
				{#if next}
					<span class="at">{formatClock(next.start_s)}</span>
					<span class="next-text">
						<span class="next-song">{splitTitle(next.video.title).song}</span>
						{#if next.video.artist_name}<span class="next-artist">{next.video.artist_name}</span>{/if}
					</span>
				{/if}
			</button>
		{/each}

		<button type="button" class="play" aria-label={`Tune in to ${channel.title}`} onclick={tune}>
			<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M8 5.5v13l11-6.5z" /></svg>
		</button>
	{/if}
</article>

<style>
	.row {
		--ch-color: oklch(70% 0.15 calc(var(--ch-hue) * 1deg));
		position: relative;
		display: grid;
		/* Now, then the two after it, close together: no empty middle. */
		grid-template-columns: 48px minmax(150px, 220px) minmax(0, 1.5fr) minmax(0, 1fr) minmax(0, 1fr) 40px;
		align-items: center;
		gap: 18px;
		padding: 8px 12px;
		border-radius: var(--radius-sm);
		transition: background var(--motion-fast);
	}
	.row:hover,
	.row:focus-within {
		background: var(--bg-hover);
	}
	.row[aria-busy='true'] { opacity: 0.6; }
	.row.on-air { box-shadow: inset 3px 0 0 var(--accent); }

	/* The number in mono with the channel's colour as a short tab above it,
	   like the colour keys of a printed guide. */
	.no {
		position: relative;
		align-self: center;
		padding-top: 7px;
		font-family: var(--font-mono);
		font-size: var(--font-size-lg);
		font-weight: var(--font-weight-medium);
		line-height: 1;
		font-variant-numeric: tabular-nums;
		color: var(--text-secondary);
	}
	.no::before {
		content: '';
		position: absolute;
		top: 0;
		left: 0;
		width: 18px;
		height: 3px;
		border-radius: 2px;
		background: var(--ch-color);
	}
	.on-air .no { color: var(--text-primary); }

	.ident {
		display: grid;
		gap: 2px;
		min-width: 0;
	}
	.name {
		justify-self: start;
		max-width: 100%;
		padding: 0;
		border: 0;
		background: transparent;
		color: var(--text-primary);
		font: inherit;
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		text-align: left;
		cursor: pointer;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.name:hover:not(:disabled) { text-decoration: underline; text-underline-offset: 3px; }
	.name:disabled { cursor: default; }
	.subtitle {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
	}

	.now {
		display: grid;
		grid-template-columns: 96px minmax(0, 1fr);
		align-items: center;
		gap: 12px;
		min-width: 0;
	}
	.thumb {
		position: relative;
		width: 96px;
		aspect-ratio: 16 / 9;
		padding: 0;
		border: 0;
		border-radius: var(--radius-xs);
		overflow: hidden;
		background: var(--bg-raised);
		cursor: pointer;
	}
	.thumb :global(img) {
		width: 100%;
		height: 100%;
		object-fit: cover;
		filter: var(--art-wall-filter);
		transition: filter var(--motion-fast);
	}
	.row:hover .thumb :global(img),
	.row:focus-within .thumb :global(img) { filter: none; }
	.thumb:focus-visible {
		outline: 2px solid var(--accent-strong);
		outline-offset: 2px;
	}
	.bar {
		position: absolute;
		left: 0;
		right: 0;
		bottom: 0;
		height: 3px;
		background: rgba(0, 0, 0, 0.45);
	}
	.fill {
		position: absolute;
		inset: 0;
		background: var(--ch-color);
		transform-origin: 0 50%;
		transition: transform 1s linear;
	}

	.now-text {
		display: grid;
		gap: 2px;
		min-width: 0;
	}
	.song {
		justify-self: start;
		max-width: 100%;
		padding: 0;
		border: 0;
		background: transparent;
		color: var(--text-primary);
		font-family: var(--font-display);
		font-size: var(--font-size-md);
		font-style: italic;
		font-weight: var(--font-weight-medium);
		line-height: var(--line-height-snug);
		text-align: left;
		cursor: pointer;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.sub {
		display: flex;
		align-items: baseline;
		gap: 10px;
		min-width: 0;
		font-size: var(--font-size-xs);
	}
	.artist {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		color: var(--text-secondary);
		text-decoration: none;
	}
	a.artist:hover { color: var(--text-primary); text-decoration: underline; text-underline-offset: 3px; }
	a.artist:focus-visible {
		outline: 2px solid var(--accent-strong);
		outline-offset: 2px;
	}
	.left {
		flex: 0 0 auto;
		font-family: var(--font-mono);
		font-variant-numeric: tabular-nums;
		color: var(--text-tertiary);
	}
	.quiet {
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
	}

	.next {
		display: grid;
		grid-template-columns: auto minmax(0, 1fr);
		align-items: baseline;
		gap: 10px;
		min-width: 0;
		padding: 0;
		border: 0;
		background: transparent;
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
		text-align: left;
		cursor: pointer;
	}
	.next:disabled { cursor: default; }
	.at {
		font-family: var(--font-mono);
		font-variant-numeric: tabular-nums;
	}
	.next-text {
		display: grid;
		min-width: 0;
	}
	.next-song,
	.next-artist {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.next-song { color: var(--text-secondary); }
	.next:hover:not(:disabled) .next-song { color: var(--text-primary); }

	.play {
		display: grid;
		place-items: center;
		width: 36px;
		height: 36px;
		padding: 0;
		border: 0;
		border-radius: 50%;
		background: var(--accent);
		color: var(--text-on-accent);
		cursor: pointer;
		opacity: 0;
		transition: opacity var(--motion-fast);
	}
	.play svg { width: 16px; height: 16px; fill: currentColor; }
	.row:hover .play,
	.row:focus-within .play { opacity: 1; }
	.play:focus-visible,
	.name:focus-visible {
		outline: 2px solid var(--accent-strong);
		outline-offset: 2px;
	}

	/* Locked: the same row, quieter, with the hours it still needs. */
	.locked .no,
	.locked .name { color: var(--text-tertiary); }
	.coming {
		grid-column: 3 / -1;
		display: grid;
		grid-template-columns: auto minmax(80px, 220px) auto;
		align-items: center;
		gap: 12px;
		min-width: 0;
	}
	.coming-label {
		font-size: var(--font-size-label);
		font-weight: var(--font-weight-semibold);
		text-transform: uppercase;
		letter-spacing: 0.12em;
		color: var(--text-tertiary);
	}
	.unlock {
		position: relative;
		height: 3px;
		border-radius: 2px;
		overflow: hidden;
		background: var(--border-subtle);
	}
	.unlock-fill {
		position: absolute;
		inset: 0;
		background: var(--ch-color);
		opacity: 0.7;
		transform-origin: 0 50%;
	}
	.unlock-meta {
		font-family: var(--font-mono);
		font-size: var(--font-size-xs);
		font-variant-numeric: tabular-nums;
		color: var(--text-tertiary);
	}

	@media (max-width: 1180px) {
		.row {
			grid-template-columns: 48px minmax(150px, 200px) minmax(0, 1.4fr) minmax(0, 1fr) 40px;
		}
		.next.then { display: none; }
	}

	@media (max-width: 860px) {
		.row {
			grid-template-columns: 40px minmax(0, 1fr);
			row-gap: 8px;
		}
		.now,
		.coming { grid-column: 2; }
		.next,
		.play { display: none; }
	}

	@media (prefers-reduced-motion: reduce) {
		.fill { transition: none; }
	}
	:global(:root[data-motion='reduce']) .fill { transition: none; }
</style>
