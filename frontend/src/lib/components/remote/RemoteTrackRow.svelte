<script lang="ts">
	import type { TidalDiscographyTrack, TidalPlayable, Track } from '$lib/api/client';
	import {
		currentTrack,
		playAlbum,
		playArtist,
		playTidalTrackNow,
		playTrackNow
	} from '$lib/stores/player';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import { formatTrackDuration } from '$lib/utils/format';
	import {
		buildTidalTrackMenu,
		buildTrackMenu,
		type MenuTrack
	} from '$lib/player/track_menu';
	import { openActionSheet } from '$lib/remote/action_sheet';
	import { hapticTap } from '$lib/remote/haptics';
	import { longPress } from '$lib/remote/long_press';

	// Single non-discriminated prop type so Svelte 5's $props() proxy never has
	// to deal with union narrowing. Variant is a hint only; actual TIDAL-vs-
	// library dispatch is done by inspecting the track shape at runtime.
	let {
		variant = 'library',
		track,
		albumIdForPlay = null,
		artistIdForPlay = null,
		index = null,
		hideDuration = false
	}: {
		variant?: 'library' | 'tidal';
		track: Track | TidalPlayable | TidalDiscographyTrack;
		albumIdForPlay?: number | null;
		artistIdForPlay?: number | null;
		index?: number | null;
		hideDuration?: boolean;
	} = $props();

	// Treat as library only when the row has a positive library `id`. A
	// TidalPlayable / TidalDiscographyTrack will fail this check because
	// neither shape carries `id`. This is more robust than trusting the
	// variant flag alone.
	let isLibraryTrack = $derived.by(() => {
		if (variant === 'tidal') return false;
		const t = track as Track;
		return typeof t.id === 'number' && t.id > 0;
	});

	let title = $derived(track.title);
	let artist = $derived(track.artist_name ?? 'Unknown artist');
	let duration = $derived(track.duration_ms ?? 0);

	// Highlight the row when it matches the currently playing track. For library
	// rows we compare the local id; for TIDAL rows we fall back to the tidal_id
	// so a TIDAL-ephemeral playback still lights up the right row.
	let isCurrent = $derived.by(() => {
		const current = $currentTrack;
		if (!current) return false;
		if (isLibraryTrack) {
			return (track as Track).id === current.id;
		}
		const t = track as TidalPlayable | TidalDiscographyTrack;
		if (current.tidal_id && t.tidal_id) return current.tidal_id === t.tidal_id;
		return false;
	});

	function asMenuTrack(t: Track): MenuTrack {
		return {
			id: t.id,
			title: t.title,
			artist_id: t.artist_id ?? null,
			artist_name: t.artist_name ?? null,
			album_id: t.album_id ?? null,
			album_title: t.album_title ?? null,
			is_favorite: t.is_favorite ?? false
		};
	}

	function tidalShape(t: TidalPlayable | TidalDiscographyTrack): TidalPlayable {
		// TidalDiscographyTrack and TidalPlayable share enough structure that a
		// cast is safe at runtime — but `playTidalTrackNow` reads fields that
		// might be missing/undefined on the discography shape, so explicitly
		// normalise to TidalPlayable.
		return {
			tidal_id: t.tidal_id,
			title: t.title,
			artist_name: t.artist_name ?? null,
			album_title: t.album_title ?? null,
			artwork_url: t.artwork_url ?? null,
			duration_ms: t.duration_ms ?? null,
			artist_tidal_id: t.artist_tidal_id ?? null,
			album_tidal_id: t.album_tidal_id ?? null
		};
	}

	async function onTap() {
		hapticTap();
		if (!isLibraryTrack) {
			await playTidalTrackNow(tidalShape(track as TidalPlayable | TidalDiscographyTrack));
			return;
		}
		const t = track as Track;
		if (albumIdForPlay && albumIdForPlay > 0) {
			await playAlbum(albumIdForPlay, t.id);
			return;
		}
		if (artistIdForPlay && artistIdForPlay > 0) {
			await playArtist(artistIdForPlay, t.id);
			return;
		}
		await playTrackNow(t.id);
	}

	function onLongPress() {
		if (!isLibraryTrack) {
			const t = tidalShape(track as TidalPlayable | TidalDiscographyTrack);
			openActionSheet({
				title: t.title,
				subtitle: t.artist_name,
				items: buildTidalTrackMenu(t, { remoteRoutes: true })
			});
			return;
		}
		const t = track as Track;
		openActionSheet({
			title: t.title,
			subtitle: t.artist_name,
			items: buildTrackMenu(asMenuTrack(t), { remoteRoutes: true })
		});
	}
</script>

<div class="remote-track-row" class:current={isCurrent}>
	<button
		type="button"
		class="remote-track-button"
		aria-label={isCurrent ? 'Now playing' : `Play ${title}`}
		use:longPress={onLongPress}
		onclick={() => void onTap()}
	>
		{#if index != null}
			<span class="remote-track-index" aria-hidden="true">
				{#if isCurrent}
					<span class="remote-track-now-dot" aria-hidden="true"></span>
				{:else}
					{index}
				{/if}
			</span>
		{:else}
			<ArtworkImage
				className="remote-track-thumb"
				src={track.artwork_url ?? null}
				size={320}
				fallbackText="NOOR"
				decorative={true}
			/>
		{/if}
		<span class="remote-track-copy">
			<strong>
				{#if isCurrent && index == null}<em class="remote-track-now-pill" aria-hidden="true">Now</em>{/if}{title}
			</strong>
			<small>{artist}</small>
		</span>
		{#if !hideDuration && duration > 0}
			<small class="remote-track-duration">{formatTrackDuration(duration)}</small>
		{/if}
	</button>
</div>

<style>
	.remote-track-row {
		display: flex;
		align-items: center;
		min-height: 56px;
	}

	.remote-track-row.current {
		background: color-mix(in oklab, var(--accent) 12%, transparent);
		border-radius: 10px;
	}

	.remote-track-button {
		flex: 1;
		min-width: 0;
		display: flex;
		align-items: center;
		gap: 12px;
		padding: 6px 6px;
		background: transparent;
		color: var(--text-primary);
		text-align: left;
		border-radius: 10px;
	}

	.remote-track-row.current .remote-track-button {
		color: var(--accent);
	}

	.remote-track-button:active {
		background: var(--surface-1);
	}

	.remote-track-button :global(.remote-track-thumb) {
		width: 44px;
		height: 44px;
		border-radius: 6px;
		flex-shrink: 0;
	}

	.remote-track-button :global(img.remote-track-thumb) {
		object-fit: cover;
	}

	.remote-track-button :global(.remote-track-thumb.fallback) {
		display: grid;
		place-items: center;
		background: var(--surface-1);
		color: var(--text-muted);
		font-size: var(--font-size-xs);
	}

	.remote-track-index {
		width: 28px;
		flex-shrink: 0;
		color: var(--text-muted);
		text-align: center;
		font-variant-numeric: tabular-nums;
		font-size: var(--font-size-sm);
		display: grid;
		place-items: center;
	}

	.remote-track-row.current .remote-track-index {
		color: var(--accent);
	}

	.remote-track-now-dot {
		width: 8px;
		height: 8px;
		border-radius: 999px;
		background: var(--accent);
		display: inline-block;
		animation: remote-track-pulse 1400ms ease-in-out infinite;
	}

	.remote-track-now-pill {
		display: inline-block;
		margin-right: 6px;
		padding: 1px 6px;
		border-radius: 4px;
		background: var(--accent);
		color: var(--surface-0);
		font-size: var(--font-size-2xs);
		font-style: normal;
		text-transform: uppercase;
		letter-spacing: 0.04em;
		vertical-align: 1px;
	}

	.remote-track-copy {
		flex: 1;
		min-width: 0;
		display: grid;
		gap: 1px;
	}

	.remote-track-copy strong,
	.remote-track-copy small {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.remote-track-copy small {
		color: var(--text-muted);
		font-size: var(--font-size-xs);
	}

	.remote-track-duration {
		color: var(--text-muted);
		font-size: var(--font-size-xs);
		flex-shrink: 0;
	}

	@keyframes remote-track-pulse {
		0%,
		100% {
			opacity: 0.6;
			transform: scale(0.85);
		}
		50% {
			opacity: 1;
			transform: scale(1);
		}
	}
</style>
