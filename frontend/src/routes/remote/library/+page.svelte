<script lang="ts">
	import {
		api,
		type Album,
		type AnalyticsTopArtist,
		type Artist,
		type ListenHistoryEntry,
		type Track
	} from '$lib/api/client';
	import { playTrackNow } from '$lib/stores/player';
	import RemoteAlbumTile from '$lib/components/remote/RemoteAlbumTile.svelte';
	import RemotePageShell from '$lib/components/remote/RemotePageShell.svelte';
	import { upscaleTidalArtwork, type TidalArtworkSize } from '$lib/utils/artwork';
	import { hapticTap } from '$lib/remote/haptics';
	import { tap } from '$lib/remote/tap';
	import { goto } from '$app/navigation';

	type Tab = 'artists' | 'albums' | 'tracks';

	let tab = $state<Tab>('artists');
	let filter = $state('');
	let failedArtworkUrls = $state<Record<string, boolean>>({});

	// Top rails - fetched once on mount via the dashboard endpoint.
	let recents = $state<ListenHistoryEntry[]>([]);
	let topArtists = $state<AnalyticsTopArtist[]>([]);

	// Alphabetical browse - paginated.
	const PAGE = 50;
	let artists = $state<Artist[]>([]);
	let artistsHasMore = $state(true);
	let artistsLoading = $state(false);
	let albums = $state<Album[]>([]);
	let albumsHasMore = $state(true);
	let albumsLoading = $state(false);
	let tracks = $state<Track[]>([]);
	let tracksHasMore = $state(true);
	let tracksLoading = $state(false);

	async function loadDashboard() {
		try {
			const { dashboard } = await api.getAnalyticsDashboard(15, 8, 30);
			recents = dashboard.recent_listens;
			topArtists = dashboard.top_artists;
		} catch {
			// Non-critical - the alphabetical lists below still work.
		}
	}

	async function loadArtistsPage() {
		if (artistsLoading || !artistsHasMore) return;
		artistsLoading = true;
		try {
			const offset = artists.length;
			const res = await api.getArtists('name', 'asc', PAGE, offset);
			artists = [...artists, ...res.artists];
			if (res.artists.length < PAGE) artistsHasMore = false;
		} catch {
			artistsHasMore = false;
		} finally {
			artistsLoading = false;
		}
	}

	async function loadAlbumsPage() {
		if (albumsLoading || !albumsHasMore) return;
		albumsLoading = true;
		try {
			const offset = albums.length;
			// `favorite_only` defaults to true on the backend; pass false so we
			// list the whole library rather than just favourited albums.
			const res = await api.getAlbums('title', 'asc', PAGE, offset, false);
			albums = [...albums, ...res.albums];
			if (res.albums.length < PAGE) albumsHasMore = false;
		} catch {
			albumsHasMore = false;
		} finally {
			albumsLoading = false;
		}
	}

	async function loadTracksPage() {
		if (tracksLoading || !tracksHasMore) return;
		tracksLoading = true;
		try {
			const offset = tracks.length;
			// `liked_only=true` strictly filters to user-favourited tracks
			// (the looser `favorite_only` also includes tracks whose parent
			// album is favourited - that's not what the user asked for here).
			// Default sort is recency-first: the most recently liked tracks
			// surface at the top, matching how Spotify / Apple Music order
			// their "Liked Songs" view on mobile.
			const res = await api.getTracks('date_added', 'desc', PAGE, offset, true, true);
			tracks = [...tracks, ...res.tracks];
			if (res.tracks.length < PAGE) tracksHasMore = false;
		} catch {
			tracksHasMore = false;
		} finally {
			tracksLoading = false;
		}
	}

	$effect(() => {
		void loadDashboard();
		void loadArtistsPage();
	});

	// Lazy-load each tab the first time the user opens it.
	let albumsTouched = $state(false);
	let tracksTouched = $state(false);
	$effect(() => {
		if (tab === 'albums' && !albumsTouched) {
			albumsTouched = true;
			void loadAlbumsPage();
		}
		if (tab === 'tracks' && !tracksTouched) {
			tracksTouched = true;
			void loadTracksPage();
		}
	});

	let filteredArtists = $derived.by(() => {
		const q = filter.trim().toLowerCase();
		if (!q) return artists;
		return artists.filter((a) => a.name.toLowerCase().includes(q));
	});
	let filteredAlbums = $derived.by(() => {
		const q = filter.trim().toLowerCase();
		if (!q) return albums;
		return albums.filter(
			(a) =>
				a.title.toLowerCase().includes(q) ||
				(a.artist_name ?? '').toLowerCase().includes(q)
		);
	});
	let filteredTracks = $derived.by(() => {
		const q = filter.trim().toLowerCase();
		if (!q) return tracks;
		return tracks.filter(
			(t) =>
				t.title.toLowerCase().includes(q) ||
				(t.artist_name ?? '').toLowerCase().includes(q)
		);
	});

	function pickRecent(entry: ListenHistoryEntry) {
		hapticTap();
		void playTrackNow(entry.track_id);
	}

	function pickArtist(id: number) {
		hapticTap();
		void goto(`/remote/artists/${id}`);
	}

	function pickAlbum(id: number) {
		hapticTap();
		void goto(`/remote/albums/${id}`);
	}

	function pickTrack(t: Track) {
		hapticTap();
		void playTrackNow(t.id);
	}

	function artworkCandidate(
		rawUrl: string | null | undefined,
		size: TidalArtworkSize
	): string | null {
		const renderedUrl = upscaleTidalArtwork(rawUrl, size);
		if (renderedUrl && !failedArtworkUrls[renderedUrl]) return renderedUrl;
		return null;
	}

	function markArtworkFailed(renderedUrl: string | null) {
		if (!renderedUrl) return;
		failedArtworkUrls = { ...failedArtworkUrls, [renderedUrl]: true };
	}

	// Scroll-driven pagination. When the sentinel below the visible list enters
	// the viewport, request the next page for whichever tab is active.
	let sentinelEl: HTMLDivElement | null = $state(null);
	$effect(() => {
		if (!sentinelEl) return;
		const observer = new IntersectionObserver((entries) => {
			if (!entries.some((entry) => entry.isIntersecting)) return;
			if (tab === 'artists') void loadArtistsPage();
			else if (tab === 'albums') void loadAlbumsPage();
			else void loadTracksPage();
		}, { rootMargin: '300px' });
		observer.observe(sentinelEl);
		return () => observer.disconnect();
	});

	function artistPortrait(a: Artist | AnalyticsTopArtist): string | null {
		// Top-artist analytics entries don't carry photo_url; fall back to null
		// and let the placeholder render the initial.
		if ('photo_url' in a) return artworkCandidate(a.photo_url ?? null, 320);
		return null;
	}

	let activeLoading = $derived(
		tab === 'artists' ? artistsLoading : tab === 'albums' ? albumsLoading : tracksLoading
	);
	let activeHasMore = $derived(
		tab === 'artists' ? artistsHasMore : tab === 'albums' ? albumsHasMore : tracksHasMore
	);
	let activeEmpty = $derived(
		tab === 'artists'
			? filteredArtists.length === 0 && !artistsLoading
			: tab === 'albums'
				? filteredAlbums.length === 0 && !albumsLoading
				: filteredTracks.length === 0 && !tracksLoading
	);
	let filterPlaceholder = $derived(
		tab === 'artists' ? 'Filter artists' : tab === 'albums' ? 'Filter albums' : 'Filter tracks'
	);
</script>

<svelte:head>
	<title>Library - NOOR Remote</title>
</svelte:head>

<RemotePageShell title="Library">
	{#if recents.length > 0}
		<section class="remote-section">
			<header><h3>Recently played</h3></header>
			<div class="remote-rail">
				{#each recents as entry (entry.id)}
					{@const recentArt = artworkCandidate(entry.artwork_url, 320)}
					<button
						type="button"
						class="remote-recent-tile"
						aria-label="Play {entry.track_title}"
						onclick={() => pickRecent(entry)}
					>
						<span class="remote-recent-art">
							{#if recentArt}
								<img
									src={recentArt}
									alt=""
									loading="lazy"
									decoding="async"
									onerror={() => markArtworkFailed(recentArt)}
								/>
							{:else}
								<span aria-hidden="true">NOOR</span>
							{/if}
						</span>
						<strong>{entry.track_title}</strong>
						<small>{entry.artist_name ?? 'Unknown artist'}</small>
					</button>
				{/each}
			</div>
		</section>
	{/if}

	{#if topArtists.length > 0}
		<section class="remote-section">
			<header><h3>Top artists</h3></header>
			<div class="remote-rail">
				{#each topArtists as a (a.artist_id)}
					<button
						type="button"
						class="remote-top-artist"
						aria-label="Open {a.artist_name}"
						onclick={() => pickArtist(a.artist_id)}
					>
						<span class="remote-top-artist-portrait" aria-hidden="true">
							{a.artist_name.slice(0, 1)}
						</span>
						<strong>{a.artist_name}</strong>
					</button>
				{/each}
			</div>
		</section>
	{/if}

	<section class="remote-section">
		<div class="remote-segmented" role="tablist" aria-label="Browse">
			<button
				type="button"
				role="tab"
				aria-selected={tab === 'artists'}
				class:active={tab === 'artists'}
				use:tap={() => {
					tab = 'artists';
				}}
			>
				Artists
			</button>
			<button
				type="button"
				role="tab"
				aria-selected={tab === 'albums'}
				class:active={tab === 'albums'}
				use:tap={() => {
					tab = 'albums';
				}}
			>
				Albums
			</button>
			<button
				type="button"
				role="tab"
				aria-selected={tab === 'tracks'}
				class:active={tab === 'tracks'}
				use:tap={() => {
					tab = 'tracks';
				}}
			>
				Tracks
			</button>
			<span class="remote-segmented-thumb" data-tab={tab} aria-hidden="true"></span>
		</div>

		<div class="remote-search-bar">
			<svg viewBox="0 0 24 24" aria-hidden="true" focusable="false">
				<circle cx="11" cy="11" r="6.5" fill="none" stroke="currentColor" stroke-width="1.8" />
				<line x1="16" y1="16" x2="20.5" y2="20.5" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" />
			</svg>
			<input
				bind:value={filter}
				type="search"
				inputmode="search"
				autocomplete="off"
				autocapitalize="off"
				spellcheck="false"
				placeholder={filterPlaceholder}
				aria-label="Filter library"
			/>
		</div>

		<!-- All three lists are always rendered with the inactive ones hidden
		     via CSS. Switching tabs becomes a class toggle instead of a DOM
		     unmount/remount, so the tap registers and the new list paints in
		     the same frame. content-visibility: auto on the rows means the
		     hidden lists' rows pay no layout cost while inactive. -->
		<ul class="remote-row-list" hidden={tab !== 'artists'} aria-hidden={tab !== 'artists'}>
			{#each filteredArtists as artist (artist.id)}
				{@const artistArt = artistPortrait(artist)}
				<li>
					<button type="button" class="remote-row" onclick={() => pickArtist(artist.id)}>
						<span class="remote-row-thumb">
							{#if artistArt}
								<img
									src={artistArt}
									alt=""
									loading="lazy"
									decoding="async"
									onerror={() => markArtworkFailed(artistArt)}
								/>
							{:else}
								<span aria-hidden="true">{artist.name.slice(0, 1)}</span>
							{/if}
						</span>
						<span class="remote-row-copy">
							<strong>{artist.name}</strong>
						</span>
					</button>
				</li>
			{/each}
		</ul>

		<ul class="remote-row-list" hidden={tab !== 'albums'} aria-hidden={tab !== 'albums'}>
			{#each filteredAlbums as album (album.id)}
				{@const albumArt = artworkCandidate(album.artwork_url, 320)}
				<li>
					<button type="button" class="remote-row" onclick={() => pickAlbum(album.id)}>
						<span class="remote-row-thumb">
							{#if albumArt}
								<img
									src={albumArt}
									alt=""
									loading="lazy"
									decoding="async"
									onerror={() => markArtworkFailed(albumArt)}
								/>
							{:else}
								<span aria-hidden="true">{album.title.slice(0, 1)}</span>
							{/if}
						</span>
						<span class="remote-row-copy">
							<strong>{album.title}</strong>
							<small>{album.artist_name ?? 'Unknown artist'}</small>
						</span>
					</button>
				</li>
			{/each}
		</ul>

		<ul class="remote-row-list" hidden={tab !== 'tracks'} aria-hidden={tab !== 'tracks'}>
			{#each filteredTracks as track (track.id)}
				{@const trackArt = artworkCandidate(track.artwork_url, 320)}
				<li>
					<button type="button" class="remote-row" onclick={() => pickTrack(track)}>
						<span class="remote-row-thumb">
							{#if trackArt}
								<img
									src={trackArt}
									alt=""
									loading="lazy"
									decoding="async"
									onerror={() => markArtworkFailed(trackArt)}
								/>
							{:else}
								<span aria-hidden="true">{track.title.slice(0, 1)}</span>
							{/if}
						</span>
						<span class="remote-row-copy">
							<strong>{track.title}</strong>
							<small>{track.artist_name ?? 'Unknown artist'}</small>
						</span>
					</button>
				</li>
			{/each}
		</ul>

		{#if activeEmpty}
			<p class="remote-empty">No {tab} match.</p>
		{/if}

		<div bind:this={sentinelEl} class="remote-sentinel" aria-hidden="true">
			{#if activeLoading}
				<span>Loading more…</span>
			{:else if !activeHasMore}
				<span class="remote-end">End of list</span>
			{/if}
		</div>
	</section>
</RemotePageShell>

<style>
	.remote-section {
		display: grid;
		gap: 8px;
	}

	.remote-section header h3 {
		margin: 0;
		font-size: var(--font-size-sm);
		text-transform: uppercase;
		letter-spacing: 0.06em;
		color: var(--text-muted);
	}

	.remote-rail {
		display: flex;
		gap: 6px;
		overflow-x: auto;
		-webkit-overflow-scrolling: touch;
		padding-bottom: 4px;
		scroll-snap-type: x proximity;
	}

	.remote-recent-tile {
		flex: 0 0 auto;
		width: 124px;
		display: grid;
		gap: 4px;
		padding: 4px;
		background: transparent;
		color: var(--text-primary);
		text-align: left;
		border-radius: 12px;
		scroll-snap-align: start;
		touch-action: manipulation;
	}

	.remote-recent-tile:active {
		background: var(--surface-1);
	}

	.remote-recent-art {
		display: block;
		width: 116px;
		height: 116px;
		border-radius: 10px;
		overflow: hidden;
		background: var(--surface-1);
		display: grid;
		place-items: center;
		color: var(--text-muted);
	}

	.remote-recent-art img {
		width: 100%;
		height: 100%;
		object-fit: cover;
	}

	.remote-recent-tile strong,
	.remote-recent-tile small {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		display: block;
	}

	.remote-recent-tile strong {
		font-size: var(--font-size-sm);
	}

	.remote-recent-tile small {
		color: var(--text-muted);
		font-size: var(--font-size-xs);
	}

	.remote-top-artist {
		flex: 0 0 auto;
		width: 96px;
		display: grid;
		gap: 4px;
		padding: 4px;
		background: transparent;
		color: var(--text-primary);
		text-align: center;
		border-radius: 12px;
		scroll-snap-align: start;
		touch-action: manipulation;
	}

	.remote-top-artist:active {
		background: var(--surface-1);
	}

	.remote-top-artist-portrait {
		display: grid;
		place-items: center;
		width: 88px;
		height: 88px;
		margin: 0 auto;
		border-radius: 999px;
		background: var(--surface-1);
		color: var(--text-muted);
		font-size: var(--font-size-3xl);
		font-weight: var(--font-weight-semibold);
	}

	.remote-top-artist strong {
		display: block;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-size: var(--font-size-xs);
	}

	.remote-segmented {
		position: relative;
		display: grid;
		grid-template-columns: repeat(3, 1fr);
		padding: 4px;
		background: var(--surface-1);
		border-radius: 12px;
		isolation: isolate;
	}

	.remote-segmented button {
		position: relative;
		z-index: 1;
		min-height: 38px;
		background: transparent;
		color: var(--text-muted);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		border-radius: 9px;
		touch-action: manipulation;
	}

	.remote-segmented button.active {
		color: var(--text-primary);
	}

	.remote-segmented-thumb {
		position: absolute;
		z-index: 0;
		top: 4px;
		bottom: 4px;
		left: 4px;
		width: calc((100% - 8px) / 3);
		border-radius: 9px;
		background: var(--surface-2);
		transition: transform 240ms cubic-bezier(0.22, 1.2, 0.36, 1);
	}

	.remote-segmented-thumb[data-tab='albums'] {
		transform: translateX(100%);
	}

	.remote-segmented-thumb[data-tab='tracks'] {
		transform: translateX(200%);
	}

	.remote-search-bar {
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 0 14px;
		background: var(--surface-1);
		border-radius: 14px;
		min-height: 48px;
	}

	.remote-search-bar svg {
		width: 18px;
		height: 18px;
		color: var(--text-muted);
		flex-shrink: 0;
	}

	.remote-search-bar input {
		flex: 1;
		min-width: 0;
		background: transparent;
		border: 0;
		outline: none;
		color: var(--text-primary);
		font-size: var(--font-size-lg);
		min-height: 44px;
	}

	.remote-row-list {
		list-style: none;
		margin: 0;
		padding: 0;
		display: grid;
		gap: 2px;
	}

	/* The `hidden` HTML attribute applies `display: none` by default, but
	   `.remote-row-list { display: grid }` above overrides it - so the
	   inactive lists end up rendered on top of the active one and the user
	   only sees the first tab. Restore the hidden semantics. */
	.remote-row-list[hidden] {
		display: none;
	}

	/* `content-visibility: auto` on these rows looks great in isolation but
	   iOS Safari (as of iOS 18-26) thrashes layout on rapid scroll reversals
	   with hundreds of rows, occasionally freezing the scroll until the
	   gesture stops. loading="lazy" on the row images is enough on its own
	   for the perf budget here - leave the rows fully laid out. */

	.remote-row {
		display: flex;
		align-items: center;
		gap: 12px;
		width: 100%;
		padding: 6px 6px;
		background: transparent;
		color: var(--text-primary);
		text-align: left;
		border-radius: 10px;
		min-height: 56px;
		/* Tell iOS Safari this button is a fast-tap target that still
		   allows vertical scrolling. Without it, holding-then-dragging
		   on a row could be interpreted as a selection/callout intent
		   and stop the page from scrolling. */
		touch-action: manipulation;
	}

	.remote-row:active {
		background: var(--surface-1);
	}

	.remote-row-thumb {
		flex-shrink: 0;
		width: 44px;
		height: 44px;
		display: grid;
		place-items: center;
		border-radius: 8px;
		overflow: hidden;
		background: var(--surface-1);
		color: var(--text-muted);
		font-weight: var(--font-weight-semibold);
	}

	.remote-row-thumb img {
		width: 100%;
		height: 100%;
		object-fit: cover;
	}

	.remote-row-copy {
		flex: 1;
		min-width: 0;
		display: grid;
		gap: 1px;
	}

	.remote-row-copy strong,
	.remote-row-copy small {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.remote-row-copy small {
		color: var(--text-muted);
		font-size: var(--font-size-xs);
	}

	.remote-empty {
		margin: 12px 4px;
		color: var(--text-muted);
		font-size: var(--font-size-sm);
		text-align: center;
	}

	.remote-sentinel {
		min-height: 32px;
		display: grid;
		place-items: center;
		color: var(--text-muted);
		font-size: var(--font-size-xs);
	}

	.remote-end {
		opacity: 0.7;
	}
</style>
