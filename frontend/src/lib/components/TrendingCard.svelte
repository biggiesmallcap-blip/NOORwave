<script lang="ts">
	import type { ChartEntry, TidalPlayable, Track } from '$lib/api/client';
	import { formatDuration } from '$lib/stores/library';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { buildTrackMenu, buildTidalTrackMenu } from '$lib/player/track_menu';
	import { currentTrack, isPlaying } from '$lib/stores/player';
	import { lazyTidalArt } from '$lib/actions/lazy-tidal-art';
	import { canPlayTrack, getPlayableLabel } from '$lib/player/playable';

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
	let albumTitle = $derived(local?.album_title ?? tidal?.album_title ?? null);

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

	function letterColor(name: string): string {
		const colors = ['#e63946', '#457b9d', '#2a9d8f', '#e9c46a', '#f4a261', '#9b5de5', '#00b4d8'];
		let h = 0;
		for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) & 0xffffffff;
		return colors[Math.abs(h) % colors.length];
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

		<div class="play-overlay" aria-hidden="true">
			{#if resolving}
				<span class="mini-spinner"></span>
			{:else if !playable}
				<span class="play-state-label">{actionLabel}</span>
			{:else if isCurrent && $isPlaying}
				<svg viewBox="0 0 16 16" width="20" height="20" fill="currentColor">
					<rect x="3" y="2.5" width="3.5" height="11" rx="1" />
					<rect x="9.5" y="2.5" width="3.5" height="11" rx="1" />
				</svg>
			{:else}
				<svg viewBox="0 0 16 16" width="20" height="20" fill="currentColor">
					<path d="M3 2.5l10 5.5-10 5.5V2.5z" />
				</svg>
			{/if}
		</div>
	</div>

	<div class="meta">
		<p class="title">{title}</p>
		{#if artistId !== null}
			<a class="artist" href="/artists/{artistId}" onclick={stop}>{artistName ?? 'Unknown artist'}</a>
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
				<span class="duration">{formatDuration(durationMs)}</span>
			{/if}
		</div>

		{#if albumTitle}
			<p class="album">{albumTitle}</p>
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
		padding: 8px;
		border-radius: 12px;
		cursor: pointer;
		text-align: left;
		transition: background 140ms ease, border-color 140ms ease, transform 140ms ease;
	}

	.trending-card:hover,
	.trending-card:focus-visible {
		background: rgba(255, 255, 255, 0.04);
		border-color: rgba(255, 255, 255, 0.08);
		outline: none;
	}

	.trending-card:focus-visible {
		border-color: var(--accent-line, rgba(125, 200, 175, 0.6));
	}

	.trending-card.active {
		border-color: var(--accent-line, rgba(125, 200, 175, 0.5));
		background: rgba(125, 200, 175, 0.06);
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
		border-color: rgba(255, 255, 255, 0.06);
	}

	.art-wrap {
		position: relative;
		aspect-ratio: 1 / 1;
		width: 100%;
		border-radius: 8px;
		overflow: hidden;
		background: rgba(255, 255, 255, 0.04);
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
		font-size: 48px;
		color: rgba(255, 255, 255, 0.55);
	}

	.rank {
		position: absolute;
		top: 6px;
		left: 6px;
		min-width: 22px;
		padding: 2px 6px;
		border-radius: 6px;
		font-size: 10px;
		font-weight: 700;
		color: rgba(255, 255, 255, 0.92);
		background: rgba(0, 0, 0, 0.55);
		letter-spacing: 0.04em;
		text-align: center;
		backdrop-filter: blur(4px);
	}

	.play-overlay {
		position: absolute;
		inset: 0;
		display: flex;
		align-items: center;
		justify-content: center;
		background: linear-gradient(180deg, rgba(0, 0, 0, 0.05) 0%, rgba(0, 0, 0, 0.55) 100%);
		opacity: 0;
		color: #fff;
		transition: opacity 160ms ease;
	}

	.trending-card:hover .play-overlay,
	.trending-card.active .play-overlay,
	.trending-card.unresolved .play-overlay,
	.trending-card.resolving .play-overlay {
		opacity: 1;
	}

	.trending-card.disabled .play-overlay {
		opacity: 1;
	}

	.play-state-label {
		max-width: calc(100% - 20px);
		padding: 5px 8px;
		border-radius: 6px;
		background: rgba(0, 0, 0, 0.58);
		color: rgba(255, 255, 255, 0.9);
		font-size: 11px;
		font-weight: 700;
		line-height: 1.2;
		text-align: center;
	}

	.mini-spinner {
		width: 22px;
		height: 22px;
		border: 2px solid rgba(255, 255, 255, 0.28);
		border-top-color: #fff;
		border-radius: 50%;
		animation: card-spin 0.8s linear infinite;
	}

	@keyframes card-spin {
		to { transform: rotate(360deg); }
	}

	.meta {
		display: flex;
		flex-direction: column;
		gap: 4px;
		min-width: 0;
	}

	.title {
		margin: 0;
		font-size: 13.5px;
		font-weight: 600;
		color: var(--text-primary, #fff);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		line-height: 1.3;
	}

	.artist {
		font-size: 12px;
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
		font-size: 10px;
		font-weight: 600;
		letter-spacing: 0.04em;
		text-transform: uppercase;
		white-space: nowrap;
	}

	.chip.genre {
		background: rgba(125, 200, 175, 0.12);
		color: var(--accent, #7dc8af);
	}

	.chip.lib {
		background: rgba(125, 200, 175, 0.10);
		color: var(--accent, #7dc8af);
	}

	.chip.src {
		background: rgba(255, 255, 255, 0.06);
		color: rgba(255, 255, 255, 0.6);
	}

	.duration {
		font-size: 11px;
		color: rgba(255, 255, 255, 0.5);
		font-variant-numeric: tabular-nums;
	}

	.album {
		margin: 2px 0 0;
		font-size: 11px;
		color: rgba(255, 255, 255, 0.4);
		font-style: italic;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
</style>
