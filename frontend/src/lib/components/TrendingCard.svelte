<script lang="ts">
	import type { ChartEntry, TidalPlayable, Track } from '$lib/api/client';
	import { formatTrackDuration } from '$lib/utils/format';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { buildTrackMenu, buildTidalTrackMenu } from '$lib/player/track_menu';
	import { buildAlbumMenu } from '$lib/player/album_menu';
	import { buildArtistMenu } from '$lib/player/artist_menu';
	import { currentTrack, isPlaying } from '$lib/stores/player';
	import { lazyTidalArt } from '$lib/actions/lazy-tidal-art';
	import { canPlayTrack, getPlayableLabel } from '$lib/player/playable';
	import { letterColor } from '$lib/utils/color';
	import PlayOverlay from '$lib/components/ui/PlayOverlay.svelte';

	let {
		entry,
		index,
		onTrack,
		onTidal,
	}: {
		entry: ChartEntry;
		index: number;
		onTrack: (t: Track) => void;
		onTidal: (t: TidalPlayable) => void | Promise<void>;
	} = $props();

	let local = $derived(entry.local_track);
	let tidal = $derived(entry.tidal_playable);

	let title = $derived(local?.title ?? tidal?.title ?? 'Unknown track');
	let artistName = $derived(local?.artist_name ?? tidal?.artist_name ?? null);
	let artistId = $derived(local?.artist_id ?? null);
	let tidalArtistId = $derived(local?.artist_tidal_id ?? tidal?.artist_tidal_id ?? null);
	let albumTitle = $derived(local?.album_title ?? tidal?.album_title ?? null);
	let albumId = $derived(local?.album_id ?? null);
	let tidalAlbumId = $derived(tidal?.album_tidal_id ?? null);

	let lazyArtwork = $state<string | null>(null);
	let artwork = $derived(
		usableArtwork(lazyArtwork, local?.artwork_url, tidal?.artwork_url, entry.image_url),
	);
	let needsLazyFetch = $derived(
		usableArtwork(local?.artwork_url, tidal?.artwork_url, entry.image_url) === null,
	);

	// Filter out the well-known Last.fm "blank star" placeholder so we fall
	// back to the letter tile instead of rendering a tray of identical stars.
	const LASTFM_PLACEHOLDER_HASH = '2a96cbd8b46e442fc41c2b86b821562f';
	function usableArtwork(...candidates: (string | null | undefined)[]): string | null {
		for (const c of candidates) {
			if (!c) continue;
			const trimmed = c.trim();
			if (!trimmed) continue;
			if (trimmed.includes(LASTFM_PLACEHOLDER_HASH)) continue;
			return trimmed;
		}
		return null;
	}
	let durationMs = $derived(local?.duration_ms ?? tidal?.duration_ms ?? null);
	let inLibrary = $derived(local !== null);
	let qualityClass = $derived(qualityClassFor(local?.best_quality));
	let qualityLabel = $derived(qualityLabelFor(local?.best_quality));
	let playableTarget = $derived(local ?? tidal ?? null);
	let playable = $derived(playableTarget !== null && canPlayTrack(playableTarget));
	let unresolved = $derived(local === null && tidal !== null && tidal.tidal_id <= 0);
	let actionable = $derived(playable || unresolved);
	let resolving = $state(false);
	let actionLabel = $derived.by(() => {
		if (resolving) return 'Resolving on TIDAL...';
		if (unresolved) return 'Resolve on TIDAL';
		return playableTarget ? getPlayableLabel(playableTarget) : 'Unavailable';
	});
	let isCurrent = $derived(
		local !== null
			? $currentTrack?.id === local.id
			: tidal?.tidal_id != null && tidal.tidal_id > 0 && $currentTrack?.tidal_id === tidal.tidal_id
	);

	function qualityClassFor(q: string | null | undefined): string | null {
		if (!q) return null;
		const upper = q.toUpperCase();
		if (upper.startsWith('HI_RES')) return 'hires';
		if (upper === 'LOSSLESS') return 'lossless';
		if (upper === 'HIGH' || upper === 'LOW') return 'lossy';
		return null;
	}

	function qualityLabelFor(q: string | null | undefined): string | null {
		if (!q) return null;
		const upper = q.toUpperCase();
		if (upper === 'HI_RES_LOSSLESS') return 'Hi-Res';
		if (upper === 'HI_RES') return 'Hi-Res';
		if (upper === 'LOSSLESS') return 'Lossless';
		if (upper === 'HIGH') return 'High';
		if (upper === 'LOW') return 'Low';
		return null;
	}

	async function play() {
		if (!actionable || resolving) return;
		if (local) onTrack(local);
		else if (tidal) {
			resolving = unresolved;
			try {
				await onTidal(tidal);
			} finally {
				resolving = false;
			}
		}
	}

	function handleKey(e: KeyboardEvent) {
		if (e.key === 'Enter' || e.key === ' ') {
			e.preventDefault();
			void play();
		}
	}

	function handleContext(e: MouseEvent) {
		e.preventDefault();
		if (local) {
			openContextMenu(e, buildTrackMenu(local), local.title);
		} else if (tidal) {
			openContextMenu(e, buildTidalTrackMenu(tidal), tidal.title);
		}
	}

	function openArtistContextMenu(e: MouseEvent) {
		if (!artistName || (artistId == null && tidalArtistId == null)) return;
		e.preventDefault();
		e.stopPropagation();
		openContextMenu(
			e,
			buildArtistMenu({
				id: artistId,
				tidal_id: tidalArtistId,
				name: artistName,
				in_library: artistId != null,
			}),
			artistName
		);
	}

	function openAlbumContextMenu(e: MouseEvent) {
		if (!albumTitle || (albumId == null && tidalAlbumId == null)) return;
		e.preventDefault();
		e.stopPropagation();
		openContextMenu(
			e,
			buildAlbumMenu({
				id: albumId,
				tidal_id: tidalAlbumId,
				title: albumTitle,
				artist_id: artistId,
				artist_name: artistName,
				in_library: albumId != null,
			}),
			albumTitle
		);
	}

	function stop(e: Event) {
		e.stopPropagation();
	}
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_to_interactive_role -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<article
	class="trending-card"
	class:active={isCurrent}
	class:disabled={!actionable}
	class:unresolved
	class:resolving
	role="button"
	tabindex={actionable ? 0 : -1}
	aria-disabled={!actionable}
	aria-label="{actionLabel}: {title}"
	onclick={() => void play()}
	onkeydown={handleKey}
	oncontextmenu={handleContext}
	title={actionable ? title : actionLabel}
	use:lazyTidalArt={{
		enabled: needsLazyFetch,
		query: { artist: artistName, title },
		onResolve: (url) => (lazyArtwork = url),
	}}
>
	<div class="art-wrap">
		{#if artwork}
			<div class="art" style="background-image: url('{artwork}')"></div>
		{:else}
			<div class="art fallback" style="background: {letterColor(title)}">
				<span aria-hidden="true">♫</span>
			</div>
		{/if}

		<span class="rank" aria-label="Chart position {index + 1}">{index + 1}</span>

		{#if !playable && !resolving}
			<div class="play-overlay-scrim" aria-hidden="true">
				<span class="play-state-label">{actionLabel}</span>
			</div>
		{:else}
			<PlayOverlay
				position="center"
				size="lg"
				state={resolving ? 'loading' : isCurrent && $isPlaying ? 'pause' : 'play'}
			/>
		{/if}
	</div>

	<div class="meta">
		<p class="title">{title}</p>
		{#if artistId !== null}
			<a
				class="artist"
				href="/artists/{artistId}"
				onclick={stop}
				oncontextmenu={openArtistContextMenu}
			>{artistName ?? 'Unknown artist'}</a>
		{:else if tidalArtistId !== null && artistName}
			<a
				class="artist"
				href="/tidal/artists/{tidalArtistId}"
				onclick={stop}
				oncontextmenu={openArtistContextMenu}
			>{artistName}</a>
		{:else if artistName}
			<span class="artist">{artistName}</span>
		{/if}

		<div class="info-row">
			{#if entry.genre}
				<span class="chip genre">{entry.genre}</span>
			{/if}
			{#if inLibrary}
				<span class="chip lib">In library</span>
			{:else}
				<span class="chip src">Tidal</span>
			{/if}
			{#if qualityClass && qualityLabel}
				<span class="quality-badge {qualityClass}">{qualityLabel}</span>
			{/if}
			{#if durationMs}
				<span class="duration">{formatTrackDuration(durationMs)}</span>
			{/if}
		</div>

		{#if albumId !== null && albumTitle}
			<a
				class="album"
				href="/albums/{albumId}"
				onclick={stop}
				oncontextmenu={openAlbumContextMenu}
			>{albumTitle}</a>
		{:else if tidalAlbumId !== null && albumTitle}
			<a
				class="album"
				href="/tidal/albums/{tidalAlbumId}"
				onclick={stop}
				oncontextmenu={openAlbumContextMenu}
			>{albumTitle}</a>
		{:else if albumTitle}
			<!-- svelte-ignore a11y_no_static_element_interactions -->
			<p class="album" oncontextmenu={openAlbumContextMenu}>{albumTitle}</p>
		{/if}
	</div>
</article>

<style>
	.trending-card {
		display: flex;
		flex-direction: column;
		gap: 10px;
		background: none;
		border: 1px solid transparent;
		padding: var(--space-2);
		border-radius: var(--radius-md);
		cursor: pointer;
		text-align: left;
		transition: background var(--motion-base), border-color var(--motion-base), transform var(--motion-base);
	}

	.trending-card:hover,
	.trending-card:focus-visible {
		background: var(--bg-hover);
		border-color: var(--panel-border);
		outline: none;
	}

	.trending-card:focus-visible {
		border-color: var(--accent-line);
	}

	.trending-card.active {
		border-color: var(--accent-line);
		background: var(--accent-soft);
	}

	.trending-card.disabled {
		cursor: default;
		opacity: 0.68;
	}

	.trending-card.disabled:hover,
	.trending-card.disabled:focus-visible {
		background: none;
		border-color: transparent;
	}

	.trending-card.unresolved {
		border-color: var(--border-subtle);
	}

	.art-wrap {
		position: relative;
		aspect-ratio: 1 / 1;
		width: 100%;
		border-radius: var(--radius-sm);
		overflow: hidden;
		background: var(--bg-hover);
	}

	.art {
		width: 100%;
		height: 100%;
		background-size: cover;
		background-position: center;
		transition: transform 220ms ease;
	}

	.trending-card:hover .art {
		transform: scale(1.05);
	}

	.art.fallback {
		display: flex;
		align-items: center;
		justify-content: center;
		font-size: var(--font-size-4xl);
		color: rgba(255, 255, 255, 0.55);
	}

	.rank {
		position: absolute;
		top: 6px;
		left: 6px;
		min-width: 22px;
		padding: 2px 6px;
		border-radius: var(--radius-xs);
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-bold);
		color: rgba(255, 255, 255, 0.92);
		background: rgba(0, 0, 0, 0.55);
		letter-spacing: 0.04em;
		text-align: center;
		backdrop-filter: var(--blur-base);
		-webkit-backdrop-filter: var(--blur-base);
	}

	.play-overlay-scrim {
		position: absolute;
		inset: 0;
		display: flex;
		align-items: center;
		justify-content: center;
		background: linear-gradient(180deg, rgba(0, 0, 0, 0.05) 0%, rgba(0, 0, 0, 0.55) 100%);
		opacity: 1;
		color: #fff;
		pointer-events: none;
	}

	.trending-card:hover :global(.play-overlay),
	.trending-card.active :global(.play-overlay),
	.trending-card.resolving :global(.play-overlay) {
		opacity: 1;
		transform: translateY(0);
	}

	.play-state-label {
		max-width: calc(100% - 20px);
		padding: 5px 8px;
		border-radius: var(--radius-xs);
		background: rgba(0, 0, 0, 0.58);
		color: rgba(255, 255, 255, 0.9);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-bold);
		line-height: var(--line-height-snug);
		text-align: center;
	}

	.meta {
		display: flex;
		flex-direction: column;
		gap: 4px;
		min-width: 0;
	}

	.title {
		margin: 0;
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		color: var(--text-primary, #fff);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		line-height: var(--line-height-snug);
	}

	.artist {
		font-size: var(--font-size-xs);
		color: var(--text-secondary, rgba(255, 255, 255, 0.6));
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		text-decoration: none;
	}

	a.artist:hover {
		color: var(--text-primary, #fff);
		text-decoration: underline;
	}

	.info-row {
		display: flex;
		align-items: center;
		flex-wrap: wrap;
		gap: 6px;
		margin-top: 2px;
	}

	.chip {
		display: inline-flex;
		align-items: center;
		padding: 2px 7px;
		border-radius: 99px;
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-semibold);
		letter-spacing: 0.04em;
		text-transform: uppercase;
		white-space: nowrap;
	}

	.chip.genre {
		background: var(--accent-soft);
		color: var(--accent-strong);
	}

	.chip.lib {
		background: var(--accent-soft);
		color: var(--accent-strong);
	}

	.chip.src {
		background: var(--border-subtle);
		color: var(--text-secondary);
	}

	.duration {
		font-size: var(--font-size-xs);
		color: rgba(255, 255, 255, 0.5);
		font-variant-numeric: tabular-nums;
	}

	.album {
		margin: 2px 0 0;
		font-size: var(--font-size-xs);
		color: rgba(255, 255, 255, 0.4);
		font-style: italic;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		text-decoration: none;
	}

	a.album:hover {
		color: var(--text-primary, #fff);
		text-decoration: underline;
		text-underline-offset: 0.12em;
	}
</style>
