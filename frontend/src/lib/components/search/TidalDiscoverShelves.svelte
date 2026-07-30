<script lang="ts">
	import { goto } from '$app/navigation';
	import {
		type TidalHomeItem,
		type TidalHomeModule
	} from '$lib/api/client';
	import { playTidalTrackNow, playTidalPlaylist } from '$lib/stores/player';
	import { wheelToHorizontal } from '$lib/actions/wheel-to-horizontal';
	import PlayOverlay from '$lib/components/ui/PlayOverlay.svelte';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import SectionHeader from '$lib/components/ui/SectionHeader.svelte';
	import AlbumPopup from '$lib/components/album/AlbumPopup.svelte';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { buildAlbumMenu } from '$lib/player/album_menu';
	import { buildArtistMenu } from '$lib/player/artist_menu';
	import { buildTidalTrackMenu, downloadMenuItem } from '$lib/player/track_menu';
	import { downloadTidalPlaylist } from '$lib/stores/downloads';
	import { tidalHomeItemToPlayable } from '$lib/utils/track';

	let { modules, onViewAll, mediaKind = 'audio', startIndex = 0, nested = false, homeModules = false, onItemSelect }: {
		modules: TidalHomeModule[];
		onViewAll?: (mod: TidalHomeModule) => void;
		/** These modules came from /api/tidal/home-modules, so their ids can be
		 *  resolved by the module-detail route. Defaults to false because only
		 *  one caller can promise that; see `showViewAll`. */
		homeModules?: boolean;
		mediaKind?: 'audio' | 'video';
		/** Slot this group starts at in the host page's entrance cascade, so a
		 *  stack of shelves keeps counting up instead of restarting at zero. */
		startIndex?: number;
		/** Rendered underneath a group header that already says "TIDAL". Drops
		 *  the per-module eyebrow and demotes the heading so the two levels do
		 *  not repeat themselves. */
		nested?: boolean;
		/** Return true to claim the click. The /videos route uses this because
		 *  the default video handling navigates to /videos, which is a no-op
		 *  when you are already there. */
		onItemSelect?: (item: TidalHomeItem) => boolean;
	} = $props();

	/** The album tile whose detail popup is open, if any. */
	let albumPopupItem = $state<TidalHomeItem | null>(null);

	function handleItemClick(item: TidalHomeItem) {
		if (onItemSelect?.(item)) return;
		// A real video item always routes to the video player, whatever page
		// hosted it. (VIDEO_LIST modules now parse as kind 'video'; older
		// payload shapes may still surface videos as pseudo-tracks, handled
		// by the mediaKind branch below.)
		if (item.kind === 'video') {
			void goto(`/videos?videoId=${encodeURIComponent(item.id)}`);
			return;
		}
		// On the editorial video page every remaining item is a music video
		// (kind 'track') or a video playlist (kind 'playlist'). Route both to
		// the video player instead of the audio engine, which is what played
		// the song instead.
		if (mediaKind === 'video') {
			if (item.kind === 'track') {
				void goto(`/videos?videoId=${encodeURIComponent(item.id)}`);
				return;
			}
			if (item.kind === 'playlist') {
				void goto(`/videos?playlistId=${encodeURIComponent(item.id)}&play=1`);
				return;
			}
		}
		if (item.kind === 'track') {
			void playTidalTrackNow(tidalHomeItemToPlayable(item));
			return;
		}
		if (item.kind === 'album' && item.album_id != null) {
			// The mini detail popup, the same as a Library album card. Opening
			// /tidal/albums/{id} is still a click away, from the right-click menu or
			// the artist link inside the popup - but a tracklist is what you want
			// from an album tile, and a full page navigation to get it is a lot of
			// ceremony for a browse surface.
			albumPopupItem = item;
			return;
		}
		if (item.kind === 'playlist') {
			void playTidalPlaylist(item.id);
			return;
		}
	}

	function handleItemKeydown(event: KeyboardEvent, item: TidalHomeItem) {
		if (event.key !== 'Enter' && event.key !== ' ') return;
		event.preventDefault();
		handleItemClick(item);
	}

	function openArtist(event: MouseEvent, item: TidalHomeItem) {
		if (item.artist_id == null) return;
		event.preventDefault();
		event.stopPropagation();
		void goto(`/tidal/artists/${item.artist_id}`);
	}

	function openAlbumContextMenu(event: MouseEvent, item: TidalHomeItem) {
		event.preventDefault();
		event.stopPropagation();
		const tidalId = item.kind === 'album' ? (item.album_id ?? Number(item.id)) : item.album_id;
		const title = item.kind === 'album' ? item.title : item.album_title;
		if (!title || tidalId == null) return;
		openContextMenu(event, buildAlbumMenu({
			tidal_id: tidalId,
			title,
			artist_id: item.artist_id ?? null,
			artist_name: item.artist_name ?? null,
			in_library: false
		}, { isLocal: false }), title);
	}

	function openArtistContextMenu(event: MouseEvent, item: TidalHomeItem) {
		event.preventDefault();
		event.stopPropagation();
		if (!item.artist_name) return;
		openContextMenu(event, buildArtistMenu({
			tidal_id: item.artist_id ?? null,
			name: item.artist_name,
			in_library: false
		}, { isLocal: false }), item.artist_name);
	}

	function handleItemContextMenu(event: MouseEvent, item: TidalHomeItem) {
		event.preventDefault();
		event.stopPropagation();
		if (mediaKind === 'video') {
			const label = item.kind === 'playlist' ? 'Play video playlist' : 'Play video';
			openContextMenu(
				event,
				[{ label, icon: '▶', onSelect: () => handleItemClick(item) }],
				item.title
			);
			return;
		}
		if (item.kind === 'video') {
			openContextMenu(
				event,
				[{ label: 'Play video', icon: '▶', onSelect: () => handleItemClick(item) }],
				item.title
			);
			return;
		}
		if (item.kind === 'track') {
			openContextMenu(event, buildTidalTrackMenu(tidalHomeItemToPlayable(item)), item.title);
			return;
		}
		if (item.kind === 'album') {
			openAlbumContextMenu(event, item);
			return;
		}
		if (item.kind === 'playlist') {
			openContextMenu(
				event,
				[
					{
						label: 'Play playlist',
						icon: '▶',
						onSelect: () => { void playTidalPlaylist(item.id); }
					},
					downloadMenuItem((format) => void downloadTidalPlaylist(item.id, format), 'Download playlist')
				],
				item.title
			);
		}
	}

	function subtitleFor(item: TidalHomeItem): string | null {
		return item.artist_name ?? item.creator_name ?? null;
	}

	function ariaLabelFor(item: TidalHomeItem): string {
		if (mediaKind === 'video') return `Play video ${item.title}`;
		if (item.kind === 'album') return `Open album ${item.title}`;
		return `Play ${item.title}`;
	}

	function fallbackGlyph(kind: TidalHomeItem['kind']): string {
		if (kind === 'playlist') return '#';
		return 'M';
	}

	function viewAll(mod: TidalHomeModule) {
		if (onViewAll) { onViewAll(mod); return; }
		void goto(`/search/discover/${encodeURIComponent(mod.id)}`);
	}

	// The module-detail page is audio-only: it plays every item through
	// playTidalTrackNow, so following it from a video module plays the song
	// instead of the video. Until a video-capable detail page exists, video
	// modules only show View all when the host page supplies its own handler.
	//
	// `homeModules` is the second gate, and it is why these buttons were dead.
	// /search/discover/[id] resolves an id through
	// GET /api/tidal/discover-modules/{id}/items, and that handler looks the id
	// up in the home-modules cache and 404s otherwise
	// (tidal_home_routes.rs: load_tidal_home_modules_cached, then
	// `modules.into_iter().find(...)`). Every module rendered from an editorial
	// page - /hires, /new-releases, /explore, and the Hi-Res preview on Home -
	// comes from /api/tidal/page/{section} instead, so its id is never in that
	// cache and the link could only ever land on "That discover shelf doesn't
	// exist anymore".
	//
	// Those surfaces lose nothing: the rail already scrolls the full module, and
	// each editorial section carries a "See all" to the page that is the
	// complete set. So the per-module button is shown only where it resolves.
	let showViewAll = $derived(
		(mediaKind !== 'video' || Boolean(onViewAll)) && (homeModules || Boolean(onViewAll)),
	);

	/// Third gate, per module: is there anything behind the link?
	///
	/// `more_path` is upstream's `pagedList.dataApiPath`. The detail handler
	/// only fetches when it is set, and otherwise returns `module.items` - the
	/// very list the rail is already showing. So on a module without it, "View
	/// all" reloads the same cards onto a page of their own. The carousel is
	/// already the whole set; scrolling it is the better answer.
	function canViewAll(mod: TidalHomeModule): boolean {
		if (onViewAll) return true; // the host page decides what it means
		return showViewAll && Boolean(mod.more_path);
	}

	function isTrackList(mod: TidalHomeModule): boolean {
		return mod.kind === 'TRACK_LIST'
			|| (mod.items.length > 0 && mod.items.every((i) => i.kind === 'track'));
	}
</script>

{#snippet trackGrid(mod: TidalHomeModule)}
	<div class="track-grid">
		{#each mod.items as item (`${mod.id}-${item.id}`)}
			<div
				class="track-row"
				title={item.artist_name ? `${item.title} - ${item.artist_name}` : item.title}
				aria-label={ariaLabelFor(item)}
				onclick={() => handleItemClick(item)}
				onkeydown={(e) => handleItemKeydown(e, item)}
				oncontextmenu={(e) => handleItemContextMenu(e, item)}
				role="button"
				tabindex="0"
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
					<PlayOverlay position="center" size="sm" label={ariaLabelFor(item)} />
				</div>
				<div class="meta">
					<span class="title">{item.title}</span>
					{#if item.artist_name}
						<button
							class="sub sub-link"
							type="button"
							onclick={(e) => openArtist(e, item)}
							oncontextmenu={(e) => openArtistContextMenu(e, item)}
							disabled={item.artist_id == null}
						>{item.artist_name}</button>
					{/if}
				</div>
			</div>
		{/each}
	</div>
{/snippet}

{#snippet cardRail(mod: TidalHomeModule)}
	<div class="rail" use:wheelToHorizontal>
		{#each mod.items as item (`${mod.id}-${item.id}`)}
			<div
				class="card"
				title={subtitleFor(item) ?? item.title}
				aria-label={ariaLabelFor(item)}
				onclick={() => handleItemClick(item)}
				onkeydown={(e) => handleItemKeydown(e, item)}
				oncontextmenu={(e) => handleItemContextMenu(e, item)}
				role="button"
				tabindex="0"
			>
				<div class="art-wrap">
					<ArtworkImage
						className="discover-art"
						src={item.artwork_url}
						alt={item.title}
						size={320}
						tint={true}
						fallbackText={fallbackGlyph(item.kind)}
						decorative={true}
					/>
					<PlayOverlay
						position="corner"
						size="sm"
						label={ariaLabelFor(item)}
					/>
				</div>
				<div class="meta">
					<h3 class="title">{item.title}</h3>
					{#if subtitleFor(item)}
						{#if item.artist_id != null && item.artist_name}
							<button
								class="sub sub-link"
								type="button"
								onclick={(e) => openArtist(e, item)}
								oncontextmenu={(e) => openArtistContextMenu(e, item)}
							>{subtitleFor(item)}</button>
						{:else}
							<p class="sub">{subtitleFor(item)}</p>
						{/if}
					{/if}
				</div>
			</div>
		{/each}
	</div>
{/snippet}

{#if modules.length > 0}
	<div class="discover-stack">
		{#each modules as mod, modIndex (mod.id || mod.title)}
			<!-- When nested, the enclosing section already runs the entrance;
			     animating again here would stage the same content twice. -->
			<section
				class="discover-section"
				class:rise-in-shelf={!nested}
				data-section={mod.id || mod.title}
				style={nested ? undefined : `--rise-index: ${startIndex + modIndex}`}
			>
				<SectionHeader
					eyebrow={nested ? '' : 'TIDAL'}
					title={mod.title}
					variant="charts"
					level={nested ? 3 : 2}
				>
					{#snippet actions()}
						{#if canViewAll(mod)}
							<button type="button" class="view-all-link" onclick={() => viewAll(mod)}>
								View all -&gt;
							</button>
						{/if}
					{/snippet}
				</SectionHeader>
				{#if isTrackList(mod)}
					{@render trackGrid(mod)}
				{:else}
					{@render cardRail(mod)}
				{/if}
			</section>
		{/each}
	</div>
{/if}

{#if albumPopupItem}
	<!-- Keyed so picking a different album mounts a fresh popup rather than
	     asking the existing one to swap albums mid-load. -->
	{#key albumPopupItem}
		<AlbumPopup
			tidalAlbumId={albumPopupItem.album_id}
			artistTidalId={albumPopupItem.artist_id}
			hints={{
				title: albumPopupItem.title,
				artistName: albumPopupItem.artist_name,
				artworkUrl: albumPopupItem.artwork_url,
			}}
			onClose={() => (albumPopupItem = null)}
		/>
	{/key}
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
		gap: var(--space-3);
	}

	.view-all-link {
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		color: var(--text-secondary);
		background: none;
		border: none;
		cursor: pointer;
		padding: 0;
		transition: color var(--motion-fast);
	}
	.view-all-link:hover,
	.view-all-link:focus-visible {
		color: var(--text-primary);
		outline: none;
	}

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
	.sub-link {
		background: transparent;
		border: 0;
		padding: 0;
		font: inherit;
		text-align: left;
		cursor: pointer;
	}
	.sub-link:hover:not(:disabled),
	.sub-link:focus-visible:not(:disabled) {
		color: var(--text-primary);
		text-decoration: underline;
		text-underline-offset: 0.12em;
		outline: none;
	}
	.sub-link:disabled {
		cursor: default;
	}
	.track-row:hover :global(.discover-art) {
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

	.card {
		--card-w: clamp(120px, 11vw, 168px);
		flex: 0 0 var(--card-w);
		width: var(--card-w);
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
		background: none;
		border: 0;
		padding: 0;
		border-radius: var(--radius-md);
		text-align: left;
		scroll-snap-align: start;
		transition: transform var(--motion-base);
		box-sizing: border-box;
		cursor: pointer;
		font: inherit;
		color: inherit;
	}
	.card:hover {
		transform: translateY(-4px);
	}
	.card:focus-visible {
		outline: 2px solid var(--accent);
		outline-offset: 4px;
	}
	.card:hover :global(.play-overlay),
	.card:focus-visible :global(.play-overlay) {
		opacity: 1;
		transform: translateY(0);
	}

	.art-wrap {
		position: relative;
		aspect-ratio: 1 / 1;
		width: 100%;
		border-radius: var(--radius-sm);
		overflow: hidden;
		background: var(--bg-surface);
	}
	/* Album/mixed cards get the clean square-with-shadow artwork; the track-row
	   variant keeps its own smaller thumbnail styling below. */
	.card .art-wrap {
		border-radius: var(--radius-md);
		background: var(--bg-raised);
		box-shadow: 0 2px 8px rgba(0, 0, 0, 0.22);
		transition: box-shadow var(--motion-base);
	}
	.card:hover .art-wrap {
		box-shadow: 0 12px 26px -6px rgba(0, 0, 0, 0.5);
	}
	.art-wrap :global(.discover-art) {
		width: 100%;
		height: 100%;
		object-fit: cover;
		object-position: center;
		display: block;
		transition: transform var(--motion-base);
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
