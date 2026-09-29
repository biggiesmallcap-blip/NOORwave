<script lang="ts">
	import StateBadge from '$lib/components/ui/StateBadge.svelte';
	import type { Track } from '$lib/api/client';
	import type { QualityDisplay } from '$lib/stores/playerInformation';
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
		eyebrow = null,
		nowPlayingAttribution = null,
		stream = null,
		streamDetail = '',
		qualityLabel = '',
		qualityClass = '',
		qualityDisplay = 'details',
		playerState,
		isScrubbing,
		showStateBadge = true,
		stateBadgeCompact = true,
	}: {
		track: Track | null;
		/** Off by default: the desktop panel is self-evidently the now-playing
		 * surface, so only callers that need a label (quiet mode) pass one. */
		eyebrow?: string | null;
		nowPlayingAttribution?: string | null;
		stream?: Stream;
		streamDetail?: string;
		/** Single quality statement for the surface (e.g. "Lossless"). The
		 * artwork carries no badges of its own. */
		qualityLabel?: string;
		qualityClass?: string;
		qualityDisplay?: QualityDisplay;
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
		{#if eyebrow}
			<p class="np-eyebrow">{eyebrow}</p>
		{/if}
		{#if track && titleRef && titleHref}
			<a
				class="np-title np-title-link"
				href={titleHref}
				title={track.title}
				oncontextmenu={(e) => {
					e.preventDefault();
					e.stopPropagation();
					openContextMenu(e, buildMediaMenu(titleRef), titleRef.label);
				}}
			>
				<span class="np-title-text">{track.title}</span>
			</a>
		{:else}
			<h2 class="np-title" title={track?.title ?? 'Nothing queued'}>
				<span class="np-title-text">{track?.title ?? 'Nothing queued'}</span>
			</h2>
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
		{:else if track?.artist_name}
			<p class="np-artist">{track.artist_name}</p>
		{:else if !track}
			<p class="np-artist">Choose a track to begin playback.</p>
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
		{:else if track?.album_title}
			<p class="np-album">{track.album_title}</p>
		{/if}
		{#if nowPlayingAttribution}
			<p class="np-source">{nowPlayingAttribution}</p>
		{/if}
	</div>
	{#if showStateBadge}
		<div class="badge-row">
			<StateBadge label={isScrubbing ? 'Scrubbing' : playerState} tone={track ? 'active' : 'muted'} compact={stateBadgeCompact} />
			{#if qualityLabel && (qualityDisplay === 'icon' || qualityDisplay === 'both')}
				<span class={`quality-badge quality-icon ${qualityClass}`} role="img" aria-label={`${qualityLabel}${streamDetail ? `, ${streamDetail}` : ''}`} title={`${qualityLabel}${streamDetail ? ` · ${streamDetail}` : ''}`}>
					<svg viewBox="0 0 16 16" aria-hidden="true" focusable="false"><path d="M1.5 8h2.2l1.4-3.6 2.5 7.2 2-5.4 1.4 1.8h3.5" /></svg>
				</span>
			{/if}
			{#if qualityLabel && (qualityDisplay === 'details' || qualityDisplay === 'both')}
				<span class={`quality-badge np-quality-chip ${qualityClass}`}>{qualityLabel}</span>
			{/if}
			{#if streamDetail && (qualityDisplay === 'details' || qualityDisplay === 'both')}
				<span class="stream-micro" title={streamDetail}>{streamDetail}</span>
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
		width: fit-content;
	}

	.np-title-text {
		display: inline-block;
		max-width: 100%;
		overflow: hidden;
		text-overflow: ellipsis;
		vertical-align: bottom;
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
		flex-wrap: nowrap;
		min-width: 0;
	}

	.badge-row :global(.state-badge) {
		flex: 0 0 auto;
		justify-content: flex-start;
	}

	.np-quality-chip {
		flex: 0 0 auto;
	}

	.quality-icon {
		flex: 0 0 auto;
		justify-content: center;
		width: 22px;
		height: 22px;
		padding: 0;
	}

	.quality-icon svg {
		width: 14px;
		height: 14px;
		fill: none;
		stroke: currentColor;
		stroke-width: 1.5;
		stroke-linecap: round;
		stroke-linejoin: round;
	}

	.stream-micro {
		font-size: var(--font-size-2xs);
		color: var(--text-secondary);
		opacity: 0.55;
		font-variant-numeric: tabular-nums;
		letter-spacing: 0.025em;
		white-space: nowrap;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
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
