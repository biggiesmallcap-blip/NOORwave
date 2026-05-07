<script lang="ts">
	import StateBadge from '$lib/components/ui/StateBadge.svelte';
	import type { Track } from '$lib/api/client';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { buildArtistMenu } from '$lib/player/artist_menu';
	import { buildAlbumMenu } from '$lib/player/album_menu';

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
</script>

<div class="np-info">
	<div class="np-copy">
		<p class="np-eyebrow">{eyebrow}</p>
		<h2 class="np-title">{track?.title ?? 'Nothing queued'}</h2>
		{#if track?.artist_id && track.artist_id > 0}
			<a
				class="np-artist np-link"
				href="/artists/{track.artist_id}"
				oncontextmenu={(e) => {
					e.preventDefault();
					e.stopPropagation();
					openContextMenu(e, buildArtistMenu({ id: track.artist_id, name: track.artist_name ?? '' }, { isLocal: true }), track.artist_name ?? undefined);
				}}
			>
				{track.artist_name ?? 'Unknown artist'}
			</a>
		{:else if track?.artist_tidal_id}
			<a
				class="np-artist np-link"
				href="/tidal/artists/{track.artist_tidal_id}"
				oncontextmenu={(e) => {
					e.preventDefault();
					e.stopPropagation();
					openContextMenu(e, buildArtistMenu({ tidal_id: track.artist_tidal_id, name: track.artist_name ?? '' }, { isLocal: false }), track.artist_name ?? undefined);
				}}
			>
				{track.artist_name ?? 'Unknown artist'}
			</a>
		{:else}
			<p class="np-artist">{track?.artist_name ?? 'Choose a track to begin playback.'}</p>
		{/if}
		{#if track?.album_id}
			<a
				class="np-album np-link"
				href="/albums/{track.album_id}"
				oncontextmenu={(e) => {
					e.preventDefault();
					e.stopPropagation();
					openContextMenu(e, buildAlbumMenu({
						id: track.album_id,
						title: track.album_title ?? '',
						artist_id: track.artist_id,
						artist_name: track.artist_name,
					}, { isLocal: true }), track.album_title ?? undefined);
				}}
			>
				{track.album_title ?? 'Unknown album'}
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
		font-size: 0.66rem;
		letter-spacing: 0.13em;
		text-transform: uppercase;
		font-weight: 700;
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
		font-size: 1.35rem;
		font-family: var(--font-display);
		line-height: 1.1;
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
		font-size: 0.9rem;
	}

	.np-album {
		color: var(--text-secondary);
		font-size: 0.8rem;
	}

	.badge-row {
		display: flex;
		align-items: center;
		gap: 8px;
		flex-wrap: wrap;
	}

	.stream-micro {
		font-size: 0.68rem;
		color: var(--text-secondary);
		opacity: 0.55;
		font-variant-numeric: tabular-nums;
		letter-spacing: 0.025em;
		white-space: nowrap;
	}

	.np-source {
		font-size: 0.72rem;
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

	a.np-link:hover {
		color: var(--accent-strong, #6366f1);
		text-decoration: underline;
	}
</style>
