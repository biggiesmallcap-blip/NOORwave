<script lang="ts">
	import type { ArchiveSong } from '$lib/api/video_explore_types';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import PlayOverlay from '$lib/components/ui/PlayOverlay.svelte';
	import { openContextMenu, type MenuItem } from '$lib/stores/context_menu';
	import { buildVideoMenu } from '$lib/player/video_menu';
	import { buildArtistMenu } from '$lib/player/artist_menu';
	import { formatTrackDuration } from '$lib/utils/format';
	import LightDot from './LightDot.svelte';

	// One song in the Archive, in the Search card language: 16:9 artwork,
	// title, artist, a light dot. The artwork and title play the best cut;
	// the cuts badge is its own control and opens the Versions view.

	let {
		song,
		onplay,
		onversions,
	}: {
		song: ArchiveSong;
		onplay: (song: ArchiveSong) => void;
		onversions: (song: ArchiveSong) => void;
	} = $props();

	let duration = $derived(song.best.duration_ms ? formatTrackDuration(song.best.duration_ms) : null);
	let artistName = $derived(song.artist_name ?? song.best.artist_name ?? 'TIDAL video');
	let artistId = $derived(song.artist_id ?? song.best.artist_id);

	function videoMenu(event: MouseEvent) {
		event.preventDefault();
		event.stopPropagation();
		const items: MenuItem[] = [...buildVideoMenu(song.best)];
		if (song.cut_count > 1) {
			items.splice(1, 0, {
				label: `Show all ${song.cut_count} cuts`,
				icon: '\u2261',
				onSelect: () => onversions(song),
			});
		}
		openContextMenu(event, items, song.title);
	}

	function artistMenu(event: MouseEvent) {
		if (artistId == null) return;
		event.preventDefault();
		event.stopPropagation();
		openContextMenu(event, buildArtistMenu({ tidal_id: artistId, name: artistName }, { isLocal: false }), artistName);
	}
</script>

<div class="tile" oncontextmenu={videoMenu} role="group" aria-label={`${song.title} by ${artistName}`}>
	<button type="button" class="art" onclick={() => onplay(song)} aria-label={`Play ${song.title}`}>
		<ArtworkImage className="poster" src={song.best.artwork_url} size={320} fallbackText="VID" decorative={true} fadeIn={true} />
		<PlayOverlay position="corner" size="sm" label={`Play ${song.title}`} />
		{#if duration}<span class="duration">{duration}</span>{/if}
	</button>
	{#if song.cut_count > 1}
		<button
			type="button"
			class="cuts-badge"
			onclick={() => onversions(song)}
			aria-label={`Show all ${song.cut_count} cuts of ${song.title}`}
			title="Every cut of this song"
		>x{song.cut_count} cuts</button>
	{/if}
	<div class="meta">
		<button type="button" class="title t-row-title" onclick={() => onplay(song)} title={song.title}>{song.title}</button>
		<span class="sub t-meta">
			<LightDot light={song.light} />
			{#if artistId != null}
				<a class="artist" href={`/tidal/artists/${artistId}`} oncontextmenu={artistMenu}>{artistName}</a>
			{:else}
				<span class="artist">{artistName}</span>
			{/if}
			{#if song.song_year}<span class="year">{song.song_year}</span>{/if}
		</span>
	</div>
</div>

<style>
	/* Borderless, like Search and the Liked wall: art, then two lines. The
	   tile lifts on hover, the shadow deepens with it. */
	.tile {
		position: relative;
		display: grid;
		gap: var(--space-2);
		min-width: 0;
		transition: transform var(--motion-base);
	}

	.tile:hover {
		transform: translateY(-4px);
	}

	.art {
		position: relative;
		display: block;
		width: 100%;
		aspect-ratio: 16 / 9;
		padding: 0;
		border: 0;
		border-radius: var(--radius-sm);
		overflow: hidden;
		background: var(--bg-raised);
		box-shadow: 0 2px 8px rgba(0, 0, 0, 0.22);
		cursor: pointer;
		transition: box-shadow var(--motion-base);
	}

	.tile:hover .art {
		box-shadow: 0 12px 26px -6px rgba(0, 0, 0, 0.5);
	}

	.art:focus-visible {
		outline: 2px solid var(--accent-strong);
		outline-offset: 2px;
	}

	.art :global(.poster) {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
		color: var(--text-tertiary);
	}

	.art :global(.poster:not(.fallback)) {
		object-fit: cover;
		display: block;
	}

	.art :global(.poster.fallback) {
		display: grid;
		place-items: center;
		background: var(--bg-raised);
	}

	.art:hover :global(.play-overlay),
	.art:focus-visible :global(.play-overlay) {
		opacity: 1;
		transform: translateY(0);
	}

	/* Text over artwork: white on a dark scrim in every theme. */
	.duration,
	.cuts-badge {
		position: absolute;
		padding: 3px 8px;
		border-radius: 999px;
		background: rgba(0, 0, 0, 0.6);
		color: rgba(255, 255, 255, 0.92);
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-semibold);
		font-variant-numeric: tabular-nums;
		line-height: 1;
	}

	.duration {
		left: 8px;
		bottom: 8px;
		pointer-events: none;
	}

	.cuts-badge {
		top: 8px;
		left: 8px;
		border: 1px solid rgba(255, 255, 255, 0.16);
		cursor: pointer;
		transition: background var(--motion-fast), border-color var(--motion-fast);
	}

	.cuts-badge:hover,
	.cuts-badge:focus-visible {
		background: rgba(0, 0, 0, 0.82);
		border-color: rgba(255, 255, 255, 0.5);
	}

	.cuts-badge:focus-visible {
		outline: 2px solid var(--accent-strong);
		outline-offset: 2px;
	}

	.meta {
		display: grid;
		gap: 2px;
		min-width: 0;
	}

	.title {
		display: block;
		width: 100%;
		padding: 0;
		border: 0;
		background: none;
		color: var(--text-primary);
		text-align: left;
		cursor: pointer;
	}

	.sub {
		display: flex;
		align-items: center;
		gap: 6px;
		min-width: 0;
	}

	.artist {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		color: var(--text-secondary);
		text-decoration: none;
	}

	a.artist:hover {
		color: var(--text-primary);
		text-decoration: underline;
	}

	.year {
		flex: 0 0 auto;
		color: var(--text-tertiary);
		font-variant-numeric: tabular-nums;
	}

	.year::before {
		content: '\00b7';
		margin-right: 6px;
	}

	@media (prefers-reduced-motion: reduce) {
		.tile:hover {
			transform: none;
		}
	}
</style>
