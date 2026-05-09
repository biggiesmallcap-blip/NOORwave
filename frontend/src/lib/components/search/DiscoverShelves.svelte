<script lang="ts">
	import { onMount, untrack } from 'svelte';
	import { goto } from '$app/navigation';
	import {
		api,
		ApiError,
		type TidalHomeItem,
		type TidalHomeModule,
		type TidalPlayable
	} from '$lib/api/client';
	import { playTidalTrackNow, playTidalPlaylist } from '$lib/stores/player';
	import { tidalStatus } from '$lib/stores/tidal';
	import {
		getCachedHomeModules,
		putCachedHomeModules,
		clearCachedHomeModules
	} from '$lib/stores/tidal-home-modules-cache';
	import { wheelToHorizontal } from '$lib/actions/wheel-to-horizontal';
	import PlayOverlay from '$lib/components/ui/PlayOverlay.svelte';

	type State = 'loading' | 'ready' | 'empty' | 'disconnected' | 'error';

	// Sync-read the cache on script init so the shelves render instantly
	// when revisiting Search within the 6h TTL — no skeleton flash, no
	// round-trip. Same pattern as YourMixesShelf / PersonalRadioShelf.
	const cachedOnMount = getCachedHomeModules();
	let modules = $state<TidalHomeModule[]>(cachedOnMount ?? []);
	let viewState = $state<State>(
		cachedOnMount && cachedOnMount.length > 0 ? 'ready' : 'loading'
	);

	onMount(() => {
		if (cachedOnMount && cachedOnMount.length > 0) return;
		void load();
	});

	// Re-fetch only when TIDAL flips to 'connected'. We deliberately untrack
	// `viewState` — reading it inside the effect would otherwise re-fire on
	// every load completion and create a fetch loop on empty/error states.
	$effect(() => {
		if ($tidalStatus !== 'connected') return;
		const cur = untrack(() => viewState);
		if (cur !== 'loading' && cur !== 'ready') {
			void load();
		}
	});

	async function load() {
		viewState = 'loading';
		try {
			const data = await api.getTidalHomeModules();
			modules = data.modules ?? [];
			if (modules.length > 0) putCachedHomeModules(modules);
			viewState = modules.length > 0 ? 'ready' : 'empty';
		} catch (e) {
			if (e instanceof ApiError && e.status === 503) {
				clearCachedHomeModules();
				viewState = 'disconnected';
			} else {
				viewState = 'error';
			}
		}
	}

	function itemToPlayable(item: TidalHomeItem): TidalPlayable {
		return {
			tidal_id: Number(item.id),
			title: item.title,
			artist_name: item.artist_name ?? null,
			album_title: item.album_title ?? null,
			artwork_url: item.artwork_url ?? null,
			duration_ms: item.duration != null ? item.duration * 1000 : null,
			artist_tidal_id: item.artist_id ?? null,
			album_tidal_id: item.album_id ?? null
		};
	}

	function handleItemClick(item: TidalHomeItem) {
		if (item.kind === 'track') {
			void playTidalTrackNow(itemToPlayable(item));
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

	function subtitleFor(item: TidalHomeItem): string | null {
		return item.artist_name ?? item.creator_name ?? null;
	}

	function ariaLabelFor(item: TidalHomeItem): string {
		if (item.kind === 'album') return `Open album ${item.title}`;
		return `Play ${item.title}`;
	}

	function fallbackGlyph(kind: TidalHomeItem['kind']): string {
		if (kind === 'playlist') return '☰';
		return '♫';
	}

	function viewAll(mod: TidalHomeModule) {
		void goto(`/search/discover/${encodeURIComponent(mod.id)}`);
	}

	// TIDAL caps TRACK_LIST preview at 5 items. Render those as a 2-row
	// compact grid; album / playlist modules (10-20 items) keep the
	// horizontal card carousel.
	function isTrackList(mod: TidalHomeModule): boolean {
		return mod.kind === 'TRACK_LIST'
			|| (mod.items.length > 0 && mod.items.every((i) => i.kind === 'track'));
	}
</script>

{#snippet trackGrid(mod: TidalHomeModule)}
	<!-- 2-row × auto-fit columns. TIDAL caps TRACK_LIST modules at 5 items
	     on pages/home, so a horizontal carousel of large square cards left
	     the row sparse. The compact row layout matches the Tidal web UI's
	     own "Recommended new tracks" panel and reuses the screen better. -->
	<div class="track-grid">
		{#each mod.items as item (`${mod.id}-${item.id}`)}
			<button
				type="button"
				class="track-row"
				title={item.artist_name ? `${item.title} — ${item.artist_name}` : item.title}
				aria-label={`Play ${item.title}`}
				onclick={() => handleItemClick(item)}
			>
				<div class="art-wrap">
					{#if item.artwork_url}
						<div class="art" style="background-image: url('{item.artwork_url}')"></div>
					{:else}
						<div class="art fallback">♫</div>
					{/if}
					<PlayOverlay position="center" size="sm" label={`Play ${item.title}`} />
				</div>
				<div class="meta">
					<span class="title">{item.title}</span>
					{#if item.artist_name}
						<span class="sub">{item.artist_name}</span>
					{/if}
				</div>
			</button>
		{/each}
	</div>
{/snippet}

{#snippet cardRail(mod: TidalHomeModule)}
	<div class="rail" use:wheelToHorizontal>
		{#each mod.items as item (`${mod.id}-${item.id}`)}
			<button
				type="button"
				class="card"
				title={subtitleFor(item) ?? item.title}
				aria-label={ariaLabelFor(item)}
				onclick={() => handleItemClick(item)}
			>
				<div class="art-wrap">
					{#if item.artwork_url}
						<div class="art" style="background-image: url('{item.artwork_url}')"></div>
					{:else}
						<div class="art fallback">{fallbackGlyph(item.kind)}</div>
					{/if}
					<PlayOverlay
						position="center"
						size="md"
						label={ariaLabelFor(item)}
					/>
				</div>
				<div class="meta">
					<h3 class="title">{item.title}</h3>
					{#if subtitleFor(item)}
						<p class="sub">{subtitleFor(item)}</p>
					{/if}
				</div>
			</button>
		{/each}
	</div>
{/snippet}

{#if viewState === 'loading'}
	<p class="muted-line">Loading discover…</p>
{:else if viewState === 'ready'}
	<div class="discover-stack">
		{#each modules as mod (mod.id || mod.title)}
			<section class="discover-section" data-section={mod.id || mod.title}>
				<div class="section-header">
					<div class="section-title-group">
						<p class="eyebrow">TIDAL</p>
						<h2>{mod.title}</h2>
					</div>
					<button type="button" class="view-all-link" onclick={() => viewAll(mod)}>
						View all →
					</button>
				</div>
				{#if isTrackList(mod)}
					{@render trackGrid(mod)}
				{:else}
					{@render cardRail(mod)}
				{/if}
			</section>
		{/each}
	</div>
{:else if viewState === 'empty'}
	<p class="muted-line">
		TIDAL returned no editorial modules right now.
		<button class="inline-link" onclick={load}>Retry</button>
	</p>
{:else if viewState === 'disconnected'}
	<p class="muted-line">
		Connect TIDAL to see fresh discover picks.
		<a class="inline-link" href="/settings#sources-tidal">Open settings</a>
	</p>
{:else if viewState === 'error'}
	<p class="muted-line">
		Couldn't load discover.
		<button class="inline-link" onclick={load}>Retry</button>
	</p>
{/if}

<style>
	.discover-stack {
		display: flex;
		flex-direction: column;
		gap: var(--space-6);
	}
	.discover-section {
		display: flex;
		flex-direction: column;
		gap: var(--gap);
	}
	.section-header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--gap);
	}
	.section-title-group {
		display: flex;
		flex-direction: column;
		gap: var(--space-1);
	}
	.section-title-group h2 {
		font-size: var(--font-size-lg);
		font-weight: var(--font-weight-bold);
		margin: 0;
	}

	/* "View all →" matches the library Recent Tracks header CTA so the two
	   shelves read as siblings — same secondary-text colour, hover-fade to
	   primary, no fill or border. Action scrolls the rail to the rightmost
	   card; will swap to per-module navigation once those routes exist. */
	.view-all-link {
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		color: var(--text-secondary);
		background: none;
		border: none;
		cursor: pointer;
		padding: 0;
		transition: color var(--motion-fast) ease;
	}
	.view-all-link:hover,
	.view-all-link:focus-visible {
		color: var(--text-primary);
		outline: none;
	}

	/* ── Compact track grid (TRACK_LIST modules) ──
	   2-row layout, auto-fit columns. Items flow column-by-column so 5
	   items fill 3 columns × 2 rows (last cell empty). On narrow viewports
	   columns auto-collapse and items reflow into more rows. */
	.track-grid {
		display: grid;
		grid-template-rows: repeat(2, auto);
		grid-auto-flow: column;
		grid-auto-columns: minmax(min(260px, 100%), 1fr);
		gap: var(--gap-sm);
		overflow-x: auto;
		padding-bottom: var(--space-2);
	}
	.track-grid::-webkit-scrollbar { height: 6px; }
	.track-grid::-webkit-scrollbar-track {
		background: var(--bg-surface);
		border-radius: var(--radius-xs);
	}
	.track-grid::-webkit-scrollbar-thumb {
		background: var(--border-subtle);
		border-radius: var(--radius-xs);
	}

	.track-row {
		display: grid;
		grid-template-columns: auto 1fr;
		gap: var(--space-3);
		align-items: center;
		min-width: 0;
		background: none;
		border: 1px solid transparent;
		padding: var(--space-1) var(--space-2);
		border-radius: var(--radius-sm);
		text-align: left;
		cursor: pointer;
		font: inherit;
		color: inherit;
		transition: background var(--motion-fast) ease, border-color var(--motion-fast) ease;
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
	.track-row:hover .art {
		transform: scale(1.05);
	}

	.rail {
		display: flex;
		gap: var(--gap-sm);
		overflow-x: auto;
		padding-bottom: var(--space-2);
		scroll-snap-type: x mandatory;
		mask-image: linear-gradient(
			to right,
			transparent 0,
			black 16px,
			black calc(100% - 32px),
			transparent 100%
		);
		-webkit-mask-image: linear-gradient(
			to right,
			transparent 0,
			black 16px,
			black calc(100% - 32px),
			transparent 100%
		);
	}
	.rail::-webkit-scrollbar { height: 6px; }
	.rail::-webkit-scrollbar-track {
		background: var(--bg-surface);
		border-radius: var(--radius-xs);
	}
	.rail::-webkit-scrollbar-thumb {
		background: var(--border-subtle);
		border-radius: var(--radius-xs);
	}
	.rail::-webkit-scrollbar-thumb:hover {
		background: var(--text-muted);
	}

	/* ── Card variant (albums / playlists) ──
	   Card width clamps with viewport so the rail compacts on narrow windows
	   instead of forcing a horizontal scroll for the first 1–2 cards. */
	.card {
		--card-w: clamp(120px, 11vw, 168px);
		flex: 0 0 var(--card-w);
		width: var(--card-w);
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
		background: none;
		border: 1px solid transparent;
		padding: var(--space-2);
		border-radius: var(--radius-md);
		text-align: left;
		scroll-snap-align: start;
		transition: background var(--motion-fast) ease, border-color var(--motion-fast) ease;
		box-sizing: border-box;
		cursor: pointer;
		font: inherit;
		color: inherit;
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

	/* ── Shared art treatment ── */
	.art-wrap {
		position: relative;
		aspect-ratio: 1 / 1;
		width: 100%;
		border-radius: var(--radius-sm);
		overflow: hidden;
		background: var(--bg-surface);
	}
	.art {
		width: 100%;
		height: 100%;
		background-size: cover;
		background-position: center;
		transition: transform var(--motion-base) ease;
	}
	.card:hover .art {
		transform: scale(1.05);
	}
	.art.fallback {
		display: flex;
		align-items: center;
		justify-content: center;
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

	.muted-line {
		margin: 0;
		font-size: var(--font-size-sm);
		color: var(--text-secondary);
	}
	.inline-link {
		background: none;
		border: none;
		padding: 0;
		font: inherit;
		color: var(--accent-line);
		cursor: pointer;
		text-decoration: underline;
		text-underline-offset: 2px;
		margin-left: var(--space-1);
	}
</style>
