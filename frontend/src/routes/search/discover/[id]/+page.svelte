<script lang="ts">
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import {
		api,
		ApiError,
		type TidalHomeItem,
		type TidalHomeModule
	} from '$lib/api/client';
	import { playTidalTrackNow, playTidalPlaylist } from '$lib/stores/player';
	import { formatTrackDuration } from '$lib/utils/format';
	import PlayOverlay from '$lib/components/ui/PlayOverlay.svelte';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { buildAlbumMenu } from '$lib/player/album_menu';
	import { buildArtistMenu } from '$lib/player/artist_menu';
	import { buildTidalTrackMenu } from '$lib/player/track_menu';
	import { tidalHomeItemToPlayable } from '$lib/utils/track';

	const moduleId = $derived(page.params.id ?? '');

	let mod = $state<TidalHomeModule | null>(null);
	let loading = $state(true);
	let error = $state<string | null>(null);
	let loadSeq = 0;

	$effect(() => {
		const id = moduleId;
		if (!id) {
			loadSeq += 1;
			mod = null;
			loading = false;
			error = 'Missing discover shelf.';
			return;
		}
		void load(id);
	});

	async function load(id: string) {
		const seq = ++loadSeq;
		loading = true;
		error = null;
		try {
			const res = await api.getTidalDiscoverModule(id, 50);
			if (seq !== loadSeq) return;
			mod = res.module;
		} catch (e) {
			if (seq !== loadSeq) return;
			if (e instanceof ApiError && e.status === 404) {
				error = "That discover shelf doesn't exist anymore — TIDAL may have rotated its home page.";
			} else if (e instanceof ApiError && e.status === 503) {
				error = 'Connect TIDAL to load discover content.';
			} else {
				error = e instanceof Error ? e.message : 'Failed to load module.';
			}
		} finally {
			if (seq === loadSeq) loading = false;
		}
	}

	function handleClick(item: TidalHomeItem) {
		if (item.kind === 'track') {
			void playTidalTrackNow(tidalHomeItemToPlayable(item));
			return;
		}
		if (item.kind === 'album' && item.album_id != null) {
			void goto(`/tidal/albums/${item.album_id}`);
			return;
		}
		if (item.kind === 'playlist') {
			void playTidalPlaylist(item.id);
			return;
		}
	}

	function handleContextMenu(event: MouseEvent, item: TidalHomeItem) {
		if (item.kind !== 'track' && item.kind !== 'album') return;
		event.preventDefault();
		event.stopPropagation();
		if (item.kind === 'track') {
			openContextMenu(event, buildTidalTrackMenu(tidalHomeItemToPlayable(item)), item.title);
			return;
		}
		openContextMenu(event, buildAlbumMenu({
			tidal_id: item.album_id ?? Number(item.id),
			title: item.title,
			artist_id: item.artist_id ?? null,
			artist_name: item.artist_name ?? null,
			in_library: false
		}, { isLocal: false }), item.title);
	}

	function openArtistContextMenu(event: MouseEvent, item: TidalHomeItem) {
		if (!item.artist_name) return;
		openContextMenu(event, buildArtistMenu({
			tidal_id: item.artist_id ?? null,
			name: item.artist_name,
			in_library: false
		}, { isLocal: false }), item.artist_name);
	}

	function openAlbumContextMenu(event: MouseEvent, item: TidalHomeItem) {
		if (!item.album_title || item.album_id == null) return;
		openContextMenu(event, buildAlbumMenu({
			tidal_id: item.album_id,
			title: item.album_title,
			artist_id: item.artist_id ?? null,
			artist_name: item.artist_name ?? null,
			in_library: false
		}, { isLocal: false }), item.album_title);
	}

	function fallbackGlyph(kind: TidalHomeItem['kind']): string {
		return kind === 'playlist' ? '☰' : '♫';
	}

	function ariaLabelFor(item: TidalHomeItem): string {
		if (item.kind === 'album') return `Open album ${item.title}`;
		return `Play ${item.title}`;
	}

	const isTrackList = $derived(
		mod !== null && mod.items.length > 0 && mod.items.every((i) => i.kind === 'track')
	);
</script>

<svelte:head>
	<title>{mod?.title ? `${mod.title} — NOOR` : 'Discover — NOOR'}</title>
</svelte:head>

<div class="page-shell discover-detail animate-in">
	<header class="hero">
		<button class="back-link" onclick={() => goto('/search')}>← Back to Search</button>
		<p class="eyebrow">TIDAL</p>
		<h1>{mod?.title ?? (loading ? 'Loading…' : 'Discover')}</h1>
		{#if mod}
			<p class="subtle">{mod.items.length} item{mod.items.length === 1 ? '' : 's'}</p>
		{/if}
	</header>

	{#if loading}
		<p class="status">Loading from TIDAL…</p>
	{:else if error}
		<p class="status error">{error}</p>
	{:else if !mod || mod.items.length === 0}
		<p class="status">No items in this shelf.</p>
	{:else if isTrackList}
		<ol class="track-list">
			{#each mod.items as item, i (item.id)}
				<li>
					<button
						type="button"
						class="track-row"
						aria-label={`Play ${item.title}`}
						onclick={() => handleClick(item)}
						oncontextmenu={(e) => handleContextMenu(e, item)}
					>
						<span class="track-index">{i + 1}</span>
						<div class="art-wrap">
							<ArtworkImage
								className="discover-art"
								src={item.artwork_url}
								alt={item.title}
								size={320}
								fallbackText={fallbackGlyph(item.kind)}
								decorative={true}
							/>
							<PlayOverlay position="center" size="sm" label={`Play ${item.title}`} />
						</div>
						<div class="meta">
							<span class="title">{item.title}</span>
							{#if item.artist_name}
								<!-- svelte-ignore a11y_no_static_element_interactions -->
								<span class="sub" oncontextmenu={(e) => openArtistContextMenu(e, item)}>{item.artist_name}</span>
							{/if}
						</div>
						{#if item.album_title}
							<!-- svelte-ignore a11y_no_static_element_interactions -->
							<span class="album" oncontextmenu={(e) => openAlbumContextMenu(e, item)}>{item.album_title}</span>
						{/if}
						{#if item.duration != null}
							<span class="duration">{formatTrackDuration(item.duration * 1000)}</span>
						{/if}
					</button>
				</li>
			{/each}
		</ol>
	{:else}
		<div class="card-grid">
			{#each mod.items as item (item.id)}
				<button
					type="button"
					class="card"
					aria-label={ariaLabelFor(item)}
					onclick={() => handleClick(item)}
					oncontextmenu={(e) => handleContextMenu(e, item)}
				>
					<div class="art-wrap">
						<ArtworkImage
							className="discover-art"
							src={item.artwork_url}
							alt={item.title}
							size={320}
							fallbackText={fallbackGlyph(item.kind)}
							decorative={true}
						/>
						<PlayOverlay position="center" size="md" label={ariaLabelFor(item)} />
					</div>
					<div class="meta">
						<h3 class="title">{item.title}</h3>
						{#if item.artist_name}
							<p class="sub" oncontextmenu={(e) => openArtistContextMenu(e, item)}>{item.artist_name}</p>
						{:else if item.creator_name}
							<p class="sub">{item.creator_name}</p>
						{/if}
					</div>
				</button>
			{/each}
		</div>
	{/if}
</div>

<style>
	/* `.page-shell` (app.css) already provides the column flex layout,
	   `width: min(100%, var(--content-width))`, `margin: 0 auto`, and
	   bottom padding. Only override the inter-section gap so the hero
	   sits closer to the list than `--space-6` would render. */
	.discover-detail {
		gap: var(--space-5);
	}

	.hero {
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
		padding-top: var(--space-3);
	}

	/* `.back-link` itself is defined globally in app.css. Only the
	   per-context layout (sticking to the start of the flex hero) lives
	   here so the global utility stays purely visual. */
	.back-link {
		align-self: flex-start;
	}

	.eyebrow {
		margin: 0;
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-bold);
		letter-spacing: 0.08em;
		color: var(--accent);
	}

	.hero h1 {
		margin: 0;
		font-size: var(--font-size-3xl);
		font-weight: var(--font-weight-bold);
		line-height: var(--line-height-tight);
	}

	.subtle {
		margin: 0;
		font-size: var(--font-size-sm);
		color: var(--text-secondary);
	}

	.status {
		font-size: var(--font-size-sm);
		color: var(--text-secondary);
	}
	.status.error {
		color: var(--state-error);
	}

	/* ── Track list (TRACK_LIST modules) ── */
	.track-list {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: var(--space-1);
	}

	.track-row {
		display: grid;
		grid-template-columns: auto auto 1fr 1fr auto;
		gap: var(--space-3);
		align-items: center;
		width: 100%;
		padding: var(--space-2);
		background: none;
		border: 1px solid transparent;
		border-radius: var(--radius-sm);
		text-align: left;
		cursor: pointer;
		font: inherit;
		color: inherit;
		transition: background var(--motion-base), border-color var(--motion-base);
	}
	.track-row:hover,
	.track-row:focus-visible {
		background: var(--bg-hover);
		border-color: var(--border-subtle);
		outline: none;
	}
	.track-row:hover :global(.play-overlay),
	.track-row:focus-visible :global(.play-overlay) {
		opacity: 1;
		transform: translateY(0);
	}

	.track-index {
		font-size: var(--font-size-sm);
		color: var(--text-muted);
		font-variant-numeric: tabular-nums;
		min-width: 2ch;
		text-align: right;
	}

	/* Spec value from STYLING.md "Small inline thumbnails" — kept literal
	   so this row matches video-list / playlist-row thumbs across the app. */
	.track-row .art-wrap {
		--track-thumb: clamp(2rem, 3vw, 2.5rem);
		width: var(--track-thumb);
		height: var(--track-thumb);
		flex: 0 0 var(--track-thumb);
		position: relative;
		aspect-ratio: 1 / 1;
		border-radius: var(--radius-sm);
		overflow: hidden;
		background: var(--bg-surface);
	}

	.track-row .meta {
		display: flex;
		flex-direction: column;
		gap: var(--space-1);
		min-width: 0;
	}
	.track-row .title {
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		color: var(--text-primary);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		line-height: var(--line-height-snug);
	}
	.track-row .sub {
		font-size: var(--font-size-xs);
		color: var(--text-secondary);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.track-row .album {
		font-size: var(--font-size-xs);
		color: var(--text-muted);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.track-row .duration {
		font-size: var(--font-size-xs);
		color: var(--text-muted);
		font-variant-numeric: tabular-nums;
	}

	/* ── Card grid (ALBUM / PLAYLIST modules) ── */
	.card-grid {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(min(160px, 100%), 1fr));
		gap: var(--gap);
	}

	.card {
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
		background: none;
		border: 1px solid transparent;
		padding: var(--space-2);
		border-radius: var(--radius-md);
		text-align: left;
		cursor: pointer;
		font: inherit;
		color: inherit;
		transition: background var(--motion-fast), border-color var(--motion-fast);
	}
	.card:hover,
	.card:focus-visible {
		background: var(--bg-hover);
		border-color: var(--border-subtle);
		outline: none;
	}
	.card:focus-visible {
		border-color: var(--accent-line);
	}
	.card:hover :global(.play-overlay),
	.card:focus-visible :global(.play-overlay) {
		opacity: 1;
		transform: translateY(0);
	}

	.card .art-wrap {
		position: relative;
		aspect-ratio: 1 / 1;
		width: 100%;
		border-radius: var(--radius-sm);
		overflow: hidden;
		background: var(--bg-surface);
	}

	.art-wrap :global(.discover-art) {
		width: 100%;
		height: 100%;
		object-fit: cover;
		object-position: center;
		display: block;
		transition: transform var(--motion-base);
	}
	.card:hover :global(.discover-art),
	.track-row:hover :global(.discover-art) {
		transform: scale(1.05);
	}
	.art-wrap :global(.discover-art.fallback) {
		display: flex;
		align-items: center;
		justify-content: center;
		background: var(--bg-surface);
	}
	.art-wrap :global(.discover-art.fallback span) {
		font-size: var(--font-size-3xl);
		color: var(--text-muted);
	}

	.card .meta {
		display: flex;
		flex-direction: column;
		gap: var(--space-1);
		min-width: 0;
	}
	.card .title {
		margin: 0;
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		color: var(--text-primary);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		line-height: var(--line-height-snug);
	}
	.card .sub {
		margin: 0;
		font-size: var(--font-size-xs);
		color: var(--text-secondary);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
</style>
