<script lang="ts">
	import { goto } from '$app/navigation';
	import { api, type Playlist, type TidalSearchTrack, type Track } from '$lib/api/client';
	import {
		addTidalTrackToQueue,
		addTrackToQueue,
		playTidalTrackNow,
		playTrackNow
	} from '$lib/stores/player';
	import {
		createRemoteSearchGate,
		normalizeRemoteSearchQuery,
		shouldRunRemoteSearch
	} from '$lib/remote/search';
	import { tidalSearchTrackToPlayable } from '$lib/utils/track';
	import { showToast } from '$lib/stores/toast';
	import { hapticTap } from '$lib/remote/haptics';
	import { longPress } from '$lib/remote/long_press';
	import { openActionSheet } from '$lib/remote/action_sheet';
	import {
		buildTidalTrackMenu,
		buildTrackMenu,
		type MenuTrack
	} from '$lib/player/track_menu';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';

	type SearchMode = 'library' | 'tidal' | 'playlists';

	let query = $state('');
	let open = $state(false);
	let busy = $state(false);
	let error = $state('');
	let mode = $state<SearchMode>('library');
	let results = $state<Track[]>([]);
	let tidalResults = $state<TidalSearchTrack[]>([]);
	let playlists = $state<Playlist[]>([]);
	let playlistsLoaded = $state(false);
	const searchGate = createRemoteSearchGate();
	let playlistLoadSeq = 0;

	let inputEl: HTMLInputElement | null = $state(null);

	// Slide-down dismiss gesture on the sheet's grabber. The handle gets pointer
	// capture so a finger that drifts off it still drives the gesture.
	const DISMISS_PX = 90;
	let dragOffset = $state(0);
	let dragStartY = 0;
	let dragging = $state(false);

	$effect(() => {
		if (!open) return;
		// Autofocus the input one tick after the sheet starts opening so the
		// transform animation isn't preempted by keyboard scroll-into-view.
		const t = setTimeout(() => inputEl?.focus(), 80);
		return () => clearTimeout(t);
	});

	$effect(() => {
		if (!open) return;
		const prev = document.body.style.overflow;
		document.body.style.overflow = 'hidden';
		return () => {
			document.body.style.overflow = prev;
		};
	});

	$effect(() => {
		const activeMode = mode;
		if (activeMode === 'playlists') {
			// Playlist tab doesn't use the search input - it lists every playlist
			// once and filters client-side. Fetch on first switch.
			searchGate.invalidate();
			busy = false;
			error = '';
			if (!playlistsLoaded) {
				void loadPlaylists();
			}
			return;
		}
		playlistLoadSeq++;
		const normalized = normalizeRemoteSearchQuery(query);
		if (!shouldRunRemoteSearch(normalized)) {
			searchGate.invalidate();
			results = [];
			tidalResults = [];
			error = '';
			busy = false;
			return;
		}
		const token = searchGate.begin();
		const controller = new AbortController();
		busy = true;
		const timer = setTimeout(() => {
			const request =
				activeMode === 'tidal'
					? api.searchTidal(normalized, 12, controller.signal).then((data) => {
							if (!searchGate.isCurrent(token)) return;
							tidalResults = data.tracks;
							results = [];
							error = '';
						})
					: api.search(normalized, 12, controller.signal).then((data) => {
							if (!searchGate.isCurrent(token)) return;
							results = data.tracks;
							tidalResults = [];
							error = '';
						});
			void request
				.catch(() => {
					if (controller.signal.aborted) return;
					if (!searchGate.isCurrent(token)) return;
					results = [];
					tidalResults = [];
					error = 'Search failed.';
				})
				.finally(() => {
					if (controller.signal.aborted) return;
					if (searchGate.isCurrent(token)) busy = false;
				});
		}, 180);
		return () => {
			clearTimeout(timer);
			controller.abort();
		};
	});

	let filteredPlaylists = $derived.by(() => {
		const q = query.trim().toLowerCase();
		if (!q) return playlists;
		return playlists.filter((p) => p.name.toLowerCase().includes(q));
	});

	async function loadPlaylists() {
		const token = ++playlistLoadSeq;
		busy = true;
		error = '';
		try {
			const res = await api.getPlaylists();
			if (token !== playlistLoadSeq || mode !== 'playlists') return;
			playlists = res.playlists;
			playlistsLoaded = true;
		} catch {
			if (token !== playlistLoadSeq || mode !== 'playlists') return;
			error = 'Could not load playlists.';
		} finally {
			if (token === playlistLoadSeq && mode === 'playlists') busy = false;
		}
	}

	function libraryMenuTrack(t: Track): MenuTrack {
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

	function openLibraryMenu(track: Track) {
		// Long-press closes the search sheet so the action menu isn't trapped
		// behind it - otherwise tapping "Go to artist" can't navigate cleanly.
		close();
		openActionSheet({
			title: track.title,
			subtitle: track.artist_name,
			items: buildTrackMenu(libraryMenuTrack(track), { remoteRoutes: true })
		});
	}

	function openTidalMenu(track: TidalSearchTrack) {
		close();
		const playable = tidalSearchTrackToPlayable(track);
		openActionSheet({
			title: track.title,
			subtitle: track.artist_name,
			items: buildTidalTrackMenu(playable, { remoteRoutes: true })
		});
	}

	function pickPlaylist(playlist: Playlist) {
		hapticTap();
		close();
		void goto(`/remote/playlists/${playlist.id}`);
	}

	function close() {
		open = false;
		dragOffset = 0;
	}

	function clearQuery() {
		query = '';
		inputEl?.focus();
	}

	async function pickLibrary(track: Track) {
		await playTrackNow(track.id);
		close();
	}

	async function queueLibrary(track: Track, event: Event) {
		event.stopPropagation();
		await addTrackToQueue(track.id);
		showToast(`Queued ${track.title}`, 'info');
	}

	async function pickTidal(track: TidalSearchTrack) {
		await playTidalTrackNow(tidalSearchTrackToPlayable(track));
		close();
	}

	async function queueTidal(track: TidalSearchTrack, event: Event) {
		event.stopPropagation();
		await addTidalTrackToQueue(tidalSearchTrackToPlayable(track));
		showToast(`Queued ${track.title}`, 'info');
	}

	function onHandleDown(event: PointerEvent) {
		if (event.pointerType === 'mouse' && event.button !== 0) return;
		dragging = true;
		dragStartY = event.clientY;
		dragOffset = 0;
		(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
	}

	function onHandleMove(event: PointerEvent) {
		if (!dragging) return;
		const dy = event.clientY - dragStartY;
		// Slight rubber-band when pulled upward so it feels alive.
		dragOffset = dy > 0 ? dy : dy * 0.2;
	}

	function onHandleUp() {
		if (!dragging) return;
		dragging = false;
		if (dragOffset >= DISMISS_PX) {
			close();
		} else {
			dragOffset = 0;
		}
	}
</script>

<button
	type="button"
	class="remote-search-trigger"
	aria-label="Open search"
	onclick={() => {
		open = true;
	}}
>
	<svg viewBox="0 0 24 24" aria-hidden="true" focusable="false">
		<circle cx="11" cy="11" r="6.5" fill="none" stroke="currentColor" stroke-width="1.8" />
		<line x1="16" y1="16" x2="20.5" y2="20.5" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" />
	</svg>
	<span>Search</span>
</button>

{#if open}
	<div
		class="remote-search-overlay"
		role="dialog"
		aria-modal="true"
		aria-label="Search"
	>
		<button
			type="button"
			class="remote-search-scrim"
			aria-label="Close search"
			onclick={close}
		></button>

		<div
			class="remote-search-sheet"
			class:dragging
			style="--drag-y: {Math.max(0, dragOffset)}px;"
		>
			<div
				class="remote-search-handle"
				role="presentation"
				onpointerdown={onHandleDown}
				onpointermove={onHandleMove}
				onpointerup={onHandleUp}
				onpointercancel={onHandleUp}
			>
				<span class="remote-search-grab" aria-hidden="true"></span>
			</div>

			<div class="remote-search-bar">
				<svg class="remote-search-bar-icon" viewBox="0 0 24 24" aria-hidden="true" focusable="false">
					<circle cx="11" cy="11" r="6.5" fill="none" stroke="currentColor" stroke-width="1.8" />
					<line x1="16" y1="16" x2="20.5" y2="20.5" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" />
				</svg>
				<input
					bind:this={inputEl}
					bind:value={query}
					type="search"
					inputmode="search"
					autocomplete="off"
					autocapitalize="off"
					spellcheck="false"
					enterkeyhint="search"
					placeholder={mode === 'tidal'
						? 'Search TIDAL'
						: mode === 'playlists'
							? 'Filter playlists'
							: 'Search your library'}
					aria-label="Search query"
				/>
				{#if query}
					<button
						type="button"
						class="remote-search-clear"
						aria-label="Clear query"
						onclick={clearQuery}
					>
						<svg viewBox="0 0 24 24" aria-hidden="true" focusable="false">
							<circle cx="12" cy="12" r="9" fill="currentColor" opacity="0.18" />
							<path d="M9 9l6 6M15 9l-6 6" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" />
						</svg>
					</button>
				{/if}
			</div>

			<div class="remote-search-segmented" role="tablist" aria-label="Search source">
				<button
					type="button"
					role="tab"
					aria-selected={mode === 'library'}
					class:active={mode === 'library'}
					onclick={() => {
						mode = 'library';
					}}
				>
					Library
				</button>
				<button
					type="button"
					role="tab"
					aria-selected={mode === 'tidal'}
					class:active={mode === 'tidal'}
					onclick={() => {
						mode = 'tidal';
					}}
				>
					TIDAL
				</button>
				<button
					type="button"
					role="tab"
					aria-selected={mode === 'playlists'}
					class:active={mode === 'playlists'}
					onclick={() => {
						mode = 'playlists';
					}}
				>
					Playlists
				</button>
				<span class="remote-search-segmented-thumb" data-mode={mode} aria-hidden="true"></span>
			</div>

			<div class="remote-search-body">
				{#if mode === 'playlists'}
					{#if busy && playlists.length === 0}
						<p class="remote-search-empty">Loading playlists…</p>
					{:else if error}
						<p class="remote-search-empty">{error}</p>
					{:else if filteredPlaylists.length === 0 && query}
						<p class="remote-search-empty">No playlists match.</p>
					{:else if playlists.length === 0}
						<p class="remote-search-empty">You don't have any playlists yet.</p>
					{:else}
						<ul class="remote-search-list">
							{#each filteredPlaylists as playlist (playlist.id)}
								<li>
									<button
										type="button"
										class="remote-search-card"
										aria-label="Open {playlist.name}"
										onclick={() => pickPlaylist(playlist)}
									>
										<span class="remote-search-thumb-empty" aria-hidden="true">{playlist.name.charAt(0) || 'P'}</span>
										<span class="remote-search-card-copy">
											<strong>{playlist.name}</strong>
											<small>
												{playlist.track_count} {playlist.track_count === 1 ? 'track' : 'tracks'}
												{#if playlist.is_smart}<em class="remote-search-pill">Smart</em>{/if}
											</small>
										</span>
									</button>
								</li>
							{/each}
						</ul>
					{/if}
				{:else if busy && results.length === 0 && tidalResults.length === 0}
					<div class="remote-search-skeleton" aria-hidden="true">
						{#each Array(4) as _, i (i)}
							<div class="remote-search-skeleton-row">
								<span class="remote-search-skeleton-thumb"></span>
								<span class="remote-search-skeleton-text">
									<span></span>
									<span></span>
								</span>
							</div>
						{/each}
					</div>
				{:else if error}
					<p class="remote-search-empty">{error}</p>
				{:else if !shouldRunRemoteSearch(query)}
					<p class="remote-search-empty">
						{mode === 'tidal'
							? 'Search TIDAL for tracks, artists, or albums.'
							: 'Search anything in your library.'}
					</p>
				{:else if mode === 'library' && results.length === 0}
					<p class="remote-search-empty">No matches in your library.</p>
				{:else if mode === 'tidal' && tidalResults.length === 0}
					<p class="remote-search-empty">No matches on TIDAL.</p>
				{:else if mode === 'library'}
					<ul class="remote-search-list">
						{#each results as track (track.id)}
							<li>
								<button
									type="button"
									class="remote-search-card"
									aria-label="Play {track.title}"
									use:longPress={() => openLibraryMenu(track)}
									onclick={() => void pickLibrary(track)}
								>
									<ArtworkImage
										className="remote-search-thumb"
										src={track.artwork_url}
										size={320}
										fallbackText="NOOR"
										decorative={true}
									/>
									<span class="remote-search-card-copy">
										<strong>{track.title}</strong>
										<small>{track.artist_name ?? 'Unknown artist'}</small>
									</span>
								</button>
								<button
									type="button"
									class="remote-search-add"
									aria-label="Add {track.title} to queue"
									onclick={(e) => void queueLibrary(track, e)}
								>
									<svg viewBox="0 0 24 24" aria-hidden="true" focusable="false">
										<line x1="12" y1="5" x2="12" y2="19" stroke="currentColor" stroke-width="2" stroke-linecap="round" />
										<line x1="5" y1="12" x2="19" y2="12" stroke="currentColor" stroke-width="2" stroke-linecap="round" />
									</svg>
								</button>
							</li>
						{/each}
					</ul>
				{:else}
					<ul class="remote-search-list">
						{#each tidalResults as track (track.tidal_id)}
							<li>
								<button
									type="button"
									class="remote-search-card"
									aria-label="Play {track.title}"
									use:longPress={() => openTidalMenu(track)}
									onclick={() => void pickTidal(track)}
								>
									<ArtworkImage
										className="remote-search-thumb"
										src={track.artwork_url}
										size={320}
										fallbackText="NOOR"
										decorative={true}
									/>
									<span class="remote-search-card-copy">
										<strong>{track.title}</strong>
										<small>
											{track.artist_name ?? 'Unknown artist'}
											{#if track.in_library}<em class="remote-search-pill">In library</em>{/if}
										</small>
									</span>
								</button>
								<button
									type="button"
									class="remote-search-add"
									aria-label="Add {track.title} to queue"
									onclick={(e) => void queueTidal(track, e)}
								>
									<svg viewBox="0 0 24 24" aria-hidden="true" focusable="false">
										<line x1="12" y1="5" x2="12" y2="19" stroke="currentColor" stroke-width="2" stroke-linecap="round" />
										<line x1="5" y1="12" x2="19" y2="12" stroke="currentColor" stroke-width="2" stroke-linecap="round" />
									</svg>
								</button>
							</li>
						{/each}
					</ul>
				{/if}
			</div>
		</div>
	</div>
{/if}

<style>
	.remote-search-trigger {
		display: inline-flex;
		align-items: center;
		gap: 8px;
		min-height: 40px;
		padding: 0 14px;
		border-radius: 999px;
		background: var(--surface-1);
		color: var(--text-primary);
		font-size: var(--font-size-sm);
	}

	.remote-search-trigger:active {
		background: var(--surface-2);
	}

	.remote-search-trigger svg {
		width: 16px;
		height: 16px;
	}

	.remote-search-overlay {
		position: fixed;
		inset: 0;
		z-index: 50;
		display: grid;
	}

	.remote-search-scrim {
		position: absolute;
		inset: 0;
		background: rgba(0, 0, 0, 0.55);
		backdrop-filter: blur(6px);
		-webkit-backdrop-filter: blur(6px);
		animation: remote-search-fade 220ms ease both;
	}

	.remote-search-sheet {
		position: absolute;
		inset: max(48px, env(safe-area-inset-top)) 0 0 0;
		display: grid;
		grid-template-rows: auto auto auto 1fr;
		gap: 14px;
		padding: 6px 16px max(20px, env(safe-area-inset-bottom));
		background: var(--bg-base);
		border-top-left-radius: 22px;
		border-top-right-radius: 22px;
		box-shadow: 0 -24px 60px rgba(0, 0, 0, 0.45);
		transform: translate3d(0, var(--drag-y, 0px), 0);
		animation: remote-search-slide 280ms cubic-bezier(0.22, 1.2, 0.36, 1) both;
		transition: transform 220ms cubic-bezier(0.22, 1.2, 0.36, 1);
	}

	.remote-search-sheet.dragging {
		transition: none;
	}

	.remote-search-handle {
		display: grid;
		place-items: center;
		padding: 8px 0 4px;
		touch-action: none;
		cursor: grab;
	}

	.remote-search-grab {
		display: block;
		width: 42px;
		height: 4px;
		border-radius: 999px;
		background: var(--surface-2);
	}

	.remote-search-bar {
		position: relative;
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 0 14px;
		background: var(--surface-1);
		border-radius: 14px;
		min-height: 50px;
	}

	.remote-search-bar-icon {
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
		min-height: 48px;
	}

	.remote-search-clear {
		width: 28px;
		height: 28px;
		display: grid;
		place-items: center;
		color: var(--text-muted);
		background: transparent;
		flex-shrink: 0;
	}

	.remote-search-clear svg {
		width: 22px;
		height: 22px;
	}

	.remote-search-segmented {
		position: relative;
		display: grid;
		grid-template-columns: repeat(3, 1fr);
		padding: 4px;
		background: var(--surface-1);
		border-radius: 12px;
		isolation: isolate;
	}

	.remote-search-segmented button {
		position: relative;
		z-index: 1;
		min-height: 38px;
		background: transparent;
		color: var(--text-muted);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		border-radius: 9px;
	}

	.remote-search-segmented button.active {
		color: var(--text-primary);
	}

	.remote-search-segmented-thumb {
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

	.remote-search-segmented-thumb[data-mode='tidal'] {
		transform: translateX(100%);
	}

	.remote-search-segmented-thumb[data-mode='playlists'] {
		transform: translateX(200%);
	}

	.remote-search-body {
		overflow-y: auto;
		-webkit-overflow-scrolling: touch;
		padding-bottom: 8px;
	}

	.remote-search-empty {
		margin: 24px 8px 0;
		color: var(--text-muted);
		font-size: var(--font-size-sm);
		text-align: center;
	}

	.remote-search-list {
		list-style: none;
		margin: 0;
		padding: 0;
		display: grid;
		gap: 4px;
	}

	.remote-search-list li {
		display: flex;
		align-items: center;
		gap: 8px;
	}

	.remote-search-card {
		flex: 1;
		min-width: 0;
		display: flex;
		align-items: center;
		gap: 12px;
		padding: 8px 8px;
		border-radius: 12px;
		background: transparent;
		color: var(--text-primary);
		text-align: left;
		min-height: 60px;
	}

	.remote-search-card:active {
		background: var(--surface-1);
	}

	:global(.remote-search-thumb),
	.remote-search-thumb-empty {
		width: 48px;
		height: 48px;
		border-radius: 8px;
		object-fit: cover;
		flex-shrink: 0;
	}

	:global(.remote-search-thumb.fallback),
	.remote-search-thumb-empty {
		display: grid;
		place-items: center;
		background: var(--surface-1);
		color: var(--text-muted);
		font-size: var(--font-size-xs);
	}

	:global(.remote-search-thumb.fallback span) {
		font-weight: var(--font-weight-semibold);
	}

	.remote-search-card-copy {
		min-width: 0;
		display: grid;
		gap: 2px;
	}

	.remote-search-card-copy strong,
	.remote-search-card-copy small {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.remote-search-card-copy small {
		color: var(--text-muted);
		font-size: var(--font-size-xs);
	}

	.remote-search-pill {
		display: inline-block;
		margin-left: 6px;
		padding: 1px 6px;
		border-radius: 4px;
		background: var(--surface-2);
		color: var(--text-primary);
		font-size: var(--font-size-2xs);
		font-style: normal;
		text-transform: uppercase;
		letter-spacing: 0.04em;
		vertical-align: 1px;
	}

	.remote-search-add {
		flex: 0 0 auto;
		width: 44px;
		height: 44px;
		display: grid;
		place-items: center;
		border-radius: 999px;
		background: var(--surface-1);
		color: var(--text-primary);
	}

	.remote-search-add:active {
		background: var(--surface-2);
	}

	.remote-search-add svg {
		width: 20px;
		height: 20px;
	}

	.remote-search-skeleton {
		display: grid;
		gap: 12px;
		padding: 10px 8px 0;
	}

	.remote-search-skeleton-row {
		display: flex;
		align-items: center;
		gap: 12px;
	}

	.remote-search-skeleton-thumb {
		width: 48px;
		height: 48px;
		border-radius: 8px;
		background: var(--surface-1);
		animation: remote-search-pulse 1200ms ease-in-out infinite;
	}

	.remote-search-skeleton-text {
		flex: 1;
		display: grid;
		gap: 6px;
	}

	.remote-search-skeleton-text span {
		display: block;
		height: 10px;
		border-radius: 4px;
		background: var(--surface-1);
		animation: remote-search-pulse 1200ms ease-in-out infinite;
	}

	.remote-search-skeleton-text span:first-child {
		width: 70%;
	}

	.remote-search-skeleton-text span:last-child {
		width: 45%;
		opacity: 0.7;
	}

	@keyframes remote-search-slide {
		from {
			transform: translate3d(0, 100%, 0);
		}
		to {
			transform: translate3d(0, 0, 0);
		}
	}

	@keyframes remote-search-fade {
		from {
			opacity: 0;
		}
		to {
			opacity: 1;
		}
	}

	@keyframes remote-search-pulse {
		0%,
		100% {
			opacity: 0.55;
		}
		50% {
			opacity: 0.85;
		}
	}
</style>
