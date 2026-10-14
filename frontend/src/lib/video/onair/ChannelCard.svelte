<script lang="ts">
	import type { VideoChannel } from '$lib/api/video_explore_types';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import { noteVideoLaunch } from '$lib/video/launch';
	import { buildArtistMenu } from '$lib/player/artist_menu';
	import { buildVideoMenu } from '$lib/player/video_menu';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { channelNumber, formatClock, type LiveSlot } from './clock';
	import { channelHue } from './lineup';
	import { splitTitle } from './lower_third';

	// A big channel in "For you tonight": what is on now, full bleed, with the
	// channel's number and name over it and a live bar along the bottom. The
	// picture is the button: it tunes in where the channel is now.
	let { channel, slot, onAir = false, busy = false, rise = 0, ontune }: {
		channel: VideoChannel;
		slot: LiveSlot;
		onAir?: boolean;
		busy?: boolean;
		rise?: number;
		ontune: (channel: VideoChannel) => void;
	} = $props();

	let program = $derived(slot.program);
	let video = $derived(program?.video ?? null);
	let song = $derived(video ? splitTitle(video.title).song : '');
	let next = $derived(slot.next);

	/** The picture is one button; a right-click on the artist line inside it
	 *  opens the artist's menu, anywhere else the video's. */
	function screenMenu(event: MouseEvent) {
		if (!video) return;
		event.preventDefault();
		const onArtist = event.target instanceof Element && event.target.closest('.by') != null;
		if (onArtist && video.artist_id != null) {
			openContextMenu(event, buildArtistMenu({ tidal_id: video.artist_id, name: video.artist_name ?? '', in_library: false }, { isLocal: false }), video.artist_name ?? '');
			return;
		}
		openContextMenu(event, buildVideoMenu(video), video.title);
	}

	function nextMenu(event: MouseEvent) {
		if (!next) return;
		event.preventDefault();
		openContextMenu(event, buildVideoMenu(next.video), next.video.title);
	}
</script>

<article
	class="card rise-in-card"
	class:on-air={onAir}
	style:--ch-hue={channelHue(channel)}
	style:--rise-index={rise}
	aria-busy={busy}
	aria-label={`Channel ${channel.no}, ${channel.title}`}
>
	<button
		type="button"
		class="screen"
		onclick={(event) => {
			noteVideoLaunch(event.currentTarget);
			ontune(channel);
		}}
		oncontextmenu={screenMenu}
		aria-label={video ? `Tune in to ${channel.title}: ${video.title}${video.artist_name ? ` by ${video.artist_name}` : ''}` : `Tune in to ${channel.title}`}
	>
		<ArtworkImage src={video?.artwork_url ?? null} size={640} fallbackText={channel.title.slice(0, 1)} tint decorative fadeIn loading="eager" fetchPriority="high" />
		<span class="scrim" aria-hidden="true"></span>
		<span class="ident" aria-hidden="true">
			<span class="no">{channelNumber(channel.no)}</span>
			<span class="name">{channel.title}</span>
		</span>
		<!-- Every channel is live; only the one you are on (or tuning) says so. -->
		{#if onAir || busy}<span class="live" aria-hidden="true">{onAir ? 'On air' : 'Tuning'}</span>{/if}
		{#if video}
			<span class="now" aria-hidden="true">
				<span class="song">{song}</span>
				{#if video.artist_name}<span class="by">{video.artist_name}</span>{/if}
			</span>
		{/if}
		<span class="tune" aria-hidden="true">
			<svg viewBox="0 0 24 24"><path d="M8 5.5v13l11-6.5z" /></svg>
			Tune in
		</span>
		<span class="bar" aria-hidden="true"><span class="fill" style:transform={`scaleX(${slot.progress})`}></span></span>
	</button>
	<div class="under">
		<span class="subtitle">{channel.subtitle}</span>
		{#if next}
			<button type="button" class="next" onclick={(event) => {
				noteVideoLaunch(event.currentTarget.closest('.card')?.querySelector('.screen'));
				ontune(channel);
			}} oncontextmenu={nextMenu} title={`Next on ${channel.title}`}>
				<span class="at">{formatClock(next.start_s)}</span>
				<span class="next-title">{splitTitle(next.video.title).song}{#if next.video.artist_name}<span class="next-artist">{` - ${next.video.artist_name}`}</span>{/if}</span>
			</button>
		{/if}
	</div>
</article>

<style>
	.card {
		--ch-color: oklch(76% 0.15 calc(var(--ch-hue) * 1deg));
		display: grid;
		gap: var(--space-2);
		min-width: 0;
	}
	.card[aria-busy='true'] .screen { opacity: 0.75; }

	.screen {
		position: relative;
		display: block;
		width: 100%;
		aspect-ratio: 16 / 9;
		padding: 0;
		border: 0;
		border-radius: var(--radius-md);
		overflow: hidden;
		background: #050507;
		color: #fff;
		text-align: left;
		cursor: pointer;
		isolation: isolate;
		transition: transform var(--motion-press);
	}
	.screen:active { transform: scale(0.985); }
	.screen :global(img) {
		width: 100%;
		height: 100%;
		object-fit: cover;
		transition: transform var(--motion-base);
	}
	.screen:hover :global(img),
	.screen:focus-visible :global(img) {
		transform: scale(1.03);
	}
	.screen:focus-visible {
		outline: 2px solid var(--accent-strong);
		outline-offset: 2px;
	}
	/* On air: a ring in the channel's own colour. */
	.on-air .screen {
		box-shadow: 0 0 0 2px var(--ch-color);
	}

	/* Text over artwork is white on a scrim in every theme: dark at the top
	   for the ident, deeper at the bottom for the title. */
	.scrim {
		position: absolute;
		inset: 0;
		background:
			linear-gradient(180deg, rgba(0, 0, 0, 0.62) 0%, rgba(0, 0, 0, 0) 34%),
			linear-gradient(0deg, rgba(0, 0, 0, 0.86) 0%, rgba(0, 0, 0, 0.34) 42%, rgba(0, 0, 0, 0) 64%);
	}

	.ident {
		position: absolute;
		top: 12px;
		left: 14px;
		right: 14px;
		display: flex;
		align-items: baseline;
		gap: 10px;
		min-width: 0;
	}
	/* Room for the On air tag only when it shows. */
	.on-air .ident,
	.card[aria-busy='true'] .ident { right: 96px; }
	.no {
		position: relative;
		padding-left: 10px;
		font-family: var(--font-mono);
		font-size: var(--font-size-xl);
		font-weight: var(--font-weight-semibold);
		line-height: 1;
		font-variant-numeric: tabular-nums;
		letter-spacing: -0.02em;
	}
	/* The channel's colour as a short vertical rule before its number. */
	.no::before {
		content: '';
		position: absolute;
		left: 0;
		top: 2px;
		bottom: 2px;
		width: 3px;
		border-radius: 2px;
		background: var(--ch-color);
	}
	.name {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-size: var(--font-size-label);
		font-weight: var(--font-weight-semibold);
		text-transform: uppercase;
		letter-spacing: 0.12em;
		color: rgba(255, 255, 255, 0.86);
	}

	.live {
		position: absolute;
		top: 14px;
		right: 14px;
		display: inline-flex;
		align-items: center;
		gap: 6px;
		font-family: var(--font-mono);
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-medium);
		text-transform: uppercase;
		letter-spacing: 0.12em;
		color: rgba(255, 255, 255, 0.82);
	}
	.live::before {
		content: '';
		width: 6px;
		height: 6px;
		border-radius: 50%;
		background: var(--ch-color);
		box-shadow: 0 0 10px var(--ch-color);
	}

	.now {
		position: absolute;
		left: 14px;
		right: 14px;
		bottom: 16px;
		display: grid;
		gap: 2px;
		min-width: 0;
		transition: opacity var(--motion-fast);
	}
	.song {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-family: var(--font-display);
		font-size: var(--font-size-xl);
		font-style: italic;
		font-weight: var(--font-weight-medium);
		line-height: var(--line-height-snug);
	}
	.by {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		text-transform: uppercase;
		letter-spacing: 0.12em;
		color: rgba(255, 255, 255, 0.78);
	}

	.tune {
		position: absolute;
		top: 50%;
		left: 50%;
		display: inline-flex;
		align-items: center;
		gap: 6px;
		height: 36px;
		padding: 0 16px 0 12px;
		border-radius: 999px;
		background: rgba(255, 255, 255, 0.94);
		color: #0b0b0e;
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-bold);
		transform: translate(-50%, -50%);
		opacity: 0;
		transition: opacity var(--motion-fast);
	}
	.tune svg { width: 14px; height: 14px; fill: currentColor; }
	.screen:hover .tune,
	.screen:focus-visible .tune { opacity: 1; }

	/* The live bar: how far into what is on now. It moves once a second;
	   the linear transition makes that a glide. */
	.bar {
		position: absolute;
		left: 0;
		right: 0;
		bottom: 0;
		height: 3px;
		background: rgba(255, 255, 255, 0.16);
	}
	.fill {
		position: absolute;
		inset: 0;
		background: var(--ch-color);
		transform-origin: 0 50%;
		transition: transform 1s linear;
	}

	.under {
		display: grid;
		gap: 2px;
		min-width: 0;
		padding: 0 2px;
	}
	.subtitle,
	.next {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.subtitle {
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
	}
	.next {
		display: flex;
		align-items: baseline;
		gap: 8px;
		max-width: 100%;
		padding: 0;
		border: 0;
		background: transparent;
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
		text-align: left;
		cursor: pointer;
	}
	.next:hover .next-title { color: var(--text-secondary); }
	.at {
		flex: 0 0 auto;
		font-family: var(--font-mono);
		font-variant-numeric: tabular-nums;
		color: var(--text-tertiary);
	}
	.next-title {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		color: var(--text-tertiary);
	}
	.next:focus-visible {
		outline: 2px solid var(--accent-strong);
		outline-offset: 2px;
	}

	@media (prefers-reduced-motion: reduce) {
		.fill { transition: none; }
		.screen:hover :global(img),
		.screen:focus-visible :global(img) { transform: none; }
	}
	:global(:root[data-motion='reduce']) .fill { transition: none; }
</style>
