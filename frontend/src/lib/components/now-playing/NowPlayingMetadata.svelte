<script lang="ts">
	import StateBadge from '$lib/components/ui/StateBadge.svelte';
	import type { Track } from '$lib/api/client';
	import { openContextMenu } from '$lib/stores/context_menu';
	import {
		albumRefFromTrack,
		artistRefFromTrack,
		buildMediaMenu,
		mediaHref,
		trackRefFromTrack,
	} from '$lib/player/media_link';

	type Stream = {
		audio_quality?: string | null;
		sample_rate?: number | null;
		bit_depth?: number | null;
	} | null;

	let {
		track,
		eyebrow = 'Listening Instrument',
		nowPlayingAttribution = null,
		stream = null,
		streamDetail = '',
		playerState,
		isScrubbing,
		showStateBadge = true,
		stateBadgeCompact = true,
	}: {
		track: Track | null;
		eyebrow?: string;
		nowPlayingAttribution?: string | null;
		stream?: Stream;
		streamDetail?: string;
		playerState: string;
		isScrubbing: boolean;
		showStateBadge?: boolean;
		stateBadgeCompact?: boolean;
	} = $props();

	const titleRef = $derived(track ? trackRefFromTrack(track) : null);
	const titleHref = $derived(mediaHref(titleRef));
	const artistRef = $derived(track ? artistRefFromTrack(track) : null);
	const artistHref = $derived(mediaHref(artistRef));
	const albumRef = $derived(track ? albumRefFromTrack(track) : null);
	const albumHref = $derived(mediaHref(albumRef));
</script>

<div class="np-info">
	<div class="np-copy">
		<p class="np-eyebrow">{eyebrow}</p>
		{#if track && titleRef && titleHref}
			<a
				class="np-title np-title-link"
				href={titleHref}
				oncontextmenu={(e) => {
					e.preventDefault();
					e.stopPropagation();
					openContextMenu(e, buildMediaMenu(titleRef), titleRef.label);
				}}
			>
				{track.title}
			</a>
		{:else}
			<h2 class="np-title">{track?.title ?? 'Nothing queued'}</h2>
		{/if}
		{#if artistRef && artistHref}
			<a
				class="np-artist np-link"
				href={artistHref}
				oncontextmenu={(e) => {
					e.preventDefault();
					e.stopPropagation();
					openContextMenu(e, buildMediaMenu(artistRef), artistRef.label);
				}}
			>
				{artistRef.label}
			</a>
		{:else}
			<p class="np-artist">{track?.artist_name ?? 'Choose a track to begin playback.'}</p>
		{/if}
		{#if albumRef && albumHref}
			<a
				class="np-album np-link"
				href={albumHref}
				oncontextmenu={(e) => {
					e.preventDefault();
					e.stopPropagation();
					openContextMenu(e, buildMediaMenu(albumRef), albumRef.label);
				}}
			>
				{albumRef.label}
			</a>
		{:else}
			<p class="np-album">{track?.album_title ?? 'Playback controls stay docked here.'}</p>
		{/if}
		{#if nowPlayingAttribution}
			<p class="np-source">{nowPlayingAttribution}</p>
		{/if}
	</div>

	{#if showStateBadge}
		<div class="badge-row">
			<StateBadge label={isScrubbing ? 'Scrubbing' : playerState} tone={track ? 'active' : 'muted'} compact={stateBadgeCompact} />
			{#if streamDetail}
				<span class="stream-micro">{streamDetail}</span>
			{/if}
		</div>
	{/if}
</div>

<style>
	.np-info {
		display: flex;
		flex-direction: column;
		gap: 6px;
		min-width: 0;
	}

	.np-copy {
		display: flex;
		flex-direction: column;
		gap: 6px;
		min-width: 0;
	}

	.np-eyebrow {
		color: var(--signal-text);
		font-size: var(--font-size-2xs);
		letter-spacing: 0.13em;
		text-transform: uppercase;
		font-weight: var(--font-weight-bold);
	}

	.np-title,
	.np-artist,
	.np-album {
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		display: block;
		max-width: 100%;
	}

	.np-title {
		font-size: var(--font-size-xl);
		font-family: var(--font-display);
		line-height: var(--line-height-tight);
		letter-spacing: -0.02em;
	}

	.np-title:hover {
		text-overflow: clip;
		animation: np-title-marquee 9s ease-in-out infinite;
	}

	@keyframes np-title-marquee {
		0%, 15% { transform: translateX(0); }
		50%, 60% { transform: translateX(calc(-1 * (100% - 220px))); }
		95%, 100% { transform: translateX(0); }
	}

	.np-artist {
		color: var(--text-primary);
		font-size: var(--font-size-sm);
	}

	.np-album {
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
	}

	.badge-row {
		display: flex;
		align-items: center;
		gap: 8px;
		flex-wrap: wrap;
	}

	.stream-micro {
		font-size: var(--font-size-2xs);
		color: var(--text-secondary);
		opacity: 0.55;
		font-variant-numeric: tabular-nums;
		letter-spacing: 0.025em;
		white-space: nowrap;
	}

	.np-source {
		font-size: var(--font-size-xs);
		color: var(--text-secondary);
		opacity: 0.75;
		letter-spacing: 0.02em;
		margin-top: 0.1rem;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	a.np-link {
		color: inherit;
		text-decoration: none;
		cursor: pointer;
		transition: color var(--motion-fast);
	}

	a.np-title-link {
		color: inherit;
		text-decoration: none;
		cursor: pointer;
		transition: color var(--motion-fast);
	}

	a.np-link:hover,
	a.np-title-link:hover {
		color: var(--accent-strong, #6366f1);
		text-decoration: underline;
	}
</style>
