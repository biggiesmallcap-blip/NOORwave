<script lang="ts">
	import { onDestroy } from 'svelte';
	import { fade } from 'svelte/transition';
	import type { VideoChannel } from '$lib/api/video_explore_types';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import { buildVideoMenu } from '$lib/player/video_menu';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { LONG_FORM_S, channelNumber, formatClock, formatRuntime, unlockLabel, type LiveSlot } from './clock';
	import type { DialFamily } from './families';
	import { channelHue } from './lineup';
	import { splitTitle } from './lower_third';

	// Every scene channel as one calm index: families in a fixed order, a line
	// per channel (number, name, who is on now). The picture waits until you
	// ask for it: resting on a line (or focusing it) brings up a preview of
	// what is on beside it. Passing over lines shows nothing; the preview never
	// slides after the pointer, it fades away and comes up again where you
	// settle. A click (or Enter) tunes in.
	let { families, slots, onAirId, tuningId, ontune }: {
		families: readonly DialFamily<VideoChannel>[];
		slots: ReadonlyMap<string, LiveSlot>;
		onAirId: string | null;
		tuningId: string | null;
		ontune: (channel: VideoChannel) => void;
	} = $props();

	/** Rest this long on a line before its preview shows. */
	const INTENT_MS = 320;
	const HIDE_MS = 140;
	const CARD_W = 300;
	const CARD_H = 290;
	const GAP = 14;

	let root = $state<HTMLDivElement>();
	let preview = $state<{ id: string; x: number; y: number } | null>(null);
	let timer: ReturnType<typeof setTimeout> | null = null;

	let previewChannel = $derived(preview ? (families.flatMap((family) => family.channels).find((channel) => channel.id === preview!.id) ?? null) : null);
	let previewSlot = $derived(previewChannel ? (slots.get(previewChannel.id) ?? null) : null);

	function clearTimer() {
		if (timer) clearTimeout(timer);
		timer = null;
	}

	/** Beside the line: to the right of its column, or the left when the
	 *  column is the last one; level with the line, inside the index. */
	function place(el: HTMLElement): { x: number; y: number } | null {
		if (!root) return null;
		const box = root.getBoundingClientRect();
		const line = el.getBoundingClientRect();
		let x = line.right - box.left + GAP;
		if (x + CARD_W > box.width) x = line.left - box.left - GAP - CARD_W;
		const y = line.top - box.top + line.height / 2 - CARD_H / 2;
		return { x: Math.max(0, x), y: Math.min(Math.max(0, y), Math.max(0, box.height - CARD_H)) };
	}

	function show(channel: VideoChannel, el: HTMLElement) {
		if (channel.locked) return;
		const at = place(el);
		if (!at) return;
		preview = { id: channel.id, ...at };
	}

	function rest(channel: VideoChannel, el: HTMLElement) {
		clearTimer();
		// Another line: the open preview fades out now, not after the pointer.
		if (preview && preview.id !== channel.id) preview = null;
		timer = setTimeout(() => show(channel, el), INTENT_MS);
	}

	function away() {
		clearTimer();
		timer = setTimeout(() => (preview = null), HIDE_MS);
	}

	function videoMenu(event: MouseEvent, channel: VideoChannel) {
		const video = slots.get(channel.id)?.program?.video;
		if (!video) return;
		event.preventDefault();
		openContextMenu(event, buildVideoMenu(video), video.title);
	}

	onDestroy(clearTimer);
</script>

<div class="index" bind:this={root} onpointerleave={away} role="presentation">
	<div class="columns">
		{#each families as family (family.key)}
			<section class="family" aria-label={family.label}>
				<h4 class="family-name">{family.label}<span class="count">{family.channels.length}</span></h4>
				<ul>
					{#each family.channels as channel (channel.id)}
						{@const video = slots.get(channel.id)?.program?.video ?? null}
						<li>
							<button
								type="button"
								class="entry"
								class:on-air={channel.id === onAirId}
								class:active={preview?.id === channel.id}
								disabled={channel.locked}
								aria-busy={channel.id === tuningId}
								data-dial-no={channel.no > 0 ? channel.no : undefined}
								style:--ch-hue={channelHue(channel)}
								aria-label={`Channel ${channel.no}, ${channel.title}${video ? `: ${video.title}${video.artist_name ? ` by ${video.artist_name}` : ''}` : ''}${channel.locked ? ', coming up' : ''}`}
								onpointerenter={(event) => rest(channel, event.currentTarget)}
								onfocus={(event) => {
									clearTimer();
									show(channel, event.currentTarget);
								}}
								onblur={away}
								onclick={() => ontune(channel)}
								oncontextmenu={(event) => videoMenu(event, channel)}
							>
								<span class="no">{channel.no > 0 ? channelNumber(channel.no) : '--'}</span>
								<span class="text">
									<span class="name">{channel.title}</span>
									{#if channel.locked}
										<span class="now">{unlockLabel(channel.unlock)}</span>
									{:else if video?.artist_name}
										{#key video.artist_name}
											<span class="now" in:fade={{ duration: 400 }}>{video.artist_name}</span>
										{/key}
									{/if}
								</span>
							</button>
						</li>
					{/each}
				</ul>
			</section>
		{/each}
	</div>

	{#if preview && previewChannel}
		{@const program = previewSlot?.program ?? null}
		{@const next = previewSlot?.next ?? null}
		{#key preview.id}
		<div
			class="preview"
			style:--ch-hue={channelHue(previewChannel)}
			style:transform={`translate(${preview.x}px, ${preview.y}px)`}
			out:fade|global={{ duration: 120 }}
			aria-hidden="true"
		>
			<div class="card">
				<div class="art">
					{#if program?.video.artwork_url}
						<ArtworkImage src={program.video.artwork_url} size={640} fallbackText="" decorative fadeIn />
					{:else}
						<span class="art-no">{channelNumber(previewChannel.no)}</span>
					{/if}
					{#if previewSlot && program}
						<span class="bar"><span class="fill" style:transform={`scaleX(${previewSlot.progress})`}></span></span>
					{/if}
				</div>
				<div class="body">
					<p class="ch"><span class="ch-no">{channelNumber(previewChannel.no)}</span>{previewChannel.title}</p>
					{#if program && previewSlot}
						<p class="title">{splitTitle(program.video.title).song}</p>
						<p class="meta">
							{#if program.video.artist_name}<span>{program.video.artist_name}</span>{/if}
							<span class="mono">
								{program.duration_s >= LONG_FORM_S
									? `until ${formatClock(program.start_s + program.duration_s)}`
									: `${formatRuntime(Math.max(0, program.duration_s - previewSlot.offset))} left`}
							</span>
						</p>
					{:else}
						<p class="meta">Off air</p>
					{/if}
					{#if next}
						<p class="next"><span class="mono">{formatClock(next.start_s)}</span>{splitTitle(next.video.title).song}{#if next.video.artist_name}<span class="next-artist"> - {next.video.artist_name}</span>{/if}</p>
					{/if}
				</div>
			</div>
		</div>
		{/key}
	{/if}
</div>

<style>
	/* Three quiet columns of even height: a long family may run on into the
	   next column, but a heading never sits alone at the foot of one, and a
	   line never breaks. The preview sits outside the column flow, over it. */
	.index {
		position: relative;
		container-type: inline-size;
	}
	.columns {
		columns: 3;
		column-gap: clamp(28px, 3vw, 56px);
		padding: 0 12px;
	}
	@container (max-width: 980px) {
		.columns { columns: 2; }
	}
	@container (max-width: 560px) {
		.columns { columns: 1; }
	}

	.family {
		padding-bottom: var(--space-6);
	}
	li {
		break-inside: avoid;
	}
	.family-name {
		break-after: avoid;
		break-inside: avoid;
		display: flex;
		align-items: baseline;
		gap: 8px;
		margin: 0 0 var(--space-2);
		padding-bottom: var(--space-2);
		border-bottom: 1px solid var(--border-subtle);
		color: var(--text-secondary);
		font-size: var(--font-size-label);
		font-weight: var(--font-weight-semibold);
		letter-spacing: 0.12em;
		text-transform: uppercase;
	}
	.count {
		color: var(--text-tertiary);
		font-family: var(--font-mono);
		font-weight: var(--font-weight-medium);
		letter-spacing: 0;
	}
	ul {
		margin: 0;
		padding: 0;
		list-style: none;
	}

	/* A line: the number in the channel's colour, the name, who is on. */
	.entry {
		--ch-color: oklch(72% 0.14 calc(var(--ch-hue) * 1deg));
		position: relative;
		display: grid;
		grid-template-columns: 2.4em minmax(0, 1fr);
		align-items: baseline;
		width: 100%;
		padding: 6px 8px 6px 0;
		border: 0;
		border-radius: var(--radius-xs);
		background: transparent;
		color: inherit;
		font: inherit;
		text-align: left;
		cursor: pointer;
		transition:
			background var(--motion-base),
			color var(--motion-base);
	}
	.no {
		font-family: var(--font-mono);
		font-size: var(--font-size-xs);
		font-variant-numeric: tabular-nums;
		color: var(--ch-color);
		opacity: 0.85;
		transition: opacity var(--motion-base);
	}
	.text {
		min-width: 0;
		line-height: 1.35;
	}
	.name {
		color: var(--text-primary);
		font-size: var(--font-size-md);
		font-weight: var(--font-weight-medium);
	}
	.now {
		margin-left: 10px;
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
		transition: color var(--motion-base);
	}
	/* Resting on a line: a hair of colour under it, nothing else moves. */
	.entry::after {
		content: '';
		position: absolute;
		left: 2.4em;
		right: 8px;
		bottom: 2px;
		height: 1px;
		background: var(--ch-color);
		opacity: 0;
		transition: opacity var(--motion-base);
	}
	.entry:hover::after,
	.entry.active::after,
	.entry:focus-visible::after {
		opacity: 0.7;
	}
	.entry:hover .no,
	.entry.active .no { opacity: 1; }
	.entry:hover .now,
	.entry.active .now { color: var(--text-secondary); }
	.entry:focus-visible {
		outline: none;
		background: var(--bg-hover);
	}
	.entry.on-air .name { color: var(--accent-strong); }
	.entry[aria-busy='true'] { opacity: 0.6; }
	.entry:disabled {
		cursor: default;
	}
	.entry:disabled .name,
	.entry:disabled .no { color: var(--text-tertiary); }
	.entry:disabled .now { font-family: var(--font-mono); }

	/* The preview: what is on, beside the line you rest on. */
	.preview {
		--ch-color: oklch(72% 0.14 calc(var(--ch-hue) * 1deg));
		position: absolute;
		top: 0;
		left: 0;
		z-index: 3;
		width: 300px;
		pointer-events: none;
	}
	/* It comes up softly where it is; it never travels. */
	.card {
		animation: preview-in 220ms var(--ease-standard) both;
		overflow: hidden;
		border-radius: var(--radius-md);
		background: var(--bg-surface-strong, var(--bg-surface));
		box-shadow:
			0 18px 48px rgba(0, 0, 0, 0.45),
			0 0 0 1px var(--border-subtle);
	}
	.art {
		position: relative;
		aspect-ratio: 16 / 9;
		overflow: hidden;
		/* Without artwork: the channel's own colour, never a black box. */
		background:
			radial-gradient(120% 90% at 20% 10%, color-mix(in oklch, var(--ch-color) 45%, transparent), transparent 70%),
			color-mix(in oklch, var(--ch-color) 18%, var(--bg-surface));
	}
	.art :global(img) {
		width: 100%;
		height: 100%;
		object-fit: cover;
	}
	.art-no {
		position: absolute;
		left: 16px;
		bottom: 12px;
		font-family: var(--font-mono);
		font-size: var(--font-size-3xl);
		color: color-mix(in oklch, var(--ch-color) 70%, white);
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
	.body {
		display: grid;
		gap: 4px;
		padding: 12px 14px 14px;
	}
	.body p {
		margin: 0;
		min-width: 0;
	}
	.ch {
		display: flex;
		align-items: baseline;
		gap: 8px;
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
	}
	.ch-no {
		font-family: var(--font-mono);
		color: var(--ch-color);
	}
	.title {
		color: var(--text-primary);
		font-family: var(--font-display);
		font-size: var(--font-size-lg);
		font-style: italic;
		font-weight: var(--font-weight-medium);
		line-height: var(--line-height-snug);
	}
	.meta {
		display: flex;
		flex-wrap: wrap;
		gap: 4px 10px;
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
	}
	.mono {
		font-family: var(--font-mono);
		font-variant-numeric: tabular-nums;
		color: var(--text-tertiary);
	}
	.next {
		display: block;
		margin-top: 4px;
		padding-top: 8px;
		border-top: 1px solid var(--border-subtle);
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
	}
	.next .mono {
		margin-right: 8px;
	}

	@keyframes preview-in {
		from {
			opacity: 0;
			transform: translateY(4px);
		}
		to {
			opacity: 1;
			transform: none;
		}
	}

	@media (prefers-reduced-motion: reduce) {
		.card { animation: none; }
		.fill,
		.entry,
		.entry::after { transition: none; }
	}
	:global(:root[data-motion='reduce']) .card { animation: none; }
	:global(:root[data-motion='reduce']) .fill { transition: none; }
</style>
