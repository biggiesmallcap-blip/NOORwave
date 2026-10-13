<script lang="ts">
	import { onDestroy, onMount } from 'svelte';
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import type { Snapshot } from './$types';
	import { api, ApiError, type Playlist, type Track } from '$lib/api/client';
	import { cachedApi } from '$lib/cache/api_queries';
	import { invalidatePlaylistCaches } from '$lib/cache/ws_events';
	import { wsMessages } from '$lib/api/ws';
	import {
		currentTrack,
		isPlaying,
		playTracksInContext,
		shufflePlaylist,
		startPlaylistRadio,
	} from '$lib/stores/player';
	import { downloadPlaylist } from '$lib/stores/downloads';
	import { dismissToast, showToast } from '$lib/stores/toast';
	import { createSelection } from '$lib/stores/selection';
	import { createPersistedStore, oneOf } from '$lib/stores/persisted';
	import { createDragReorder } from '$lib/actions/drag_reorder';
	import TrackRow from '$lib/components/TrackRow.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import StateBadge from '$lib/components/ui/StateBadge.svelte';
	import Skeleton from '$lib/components/ui/Skeleton.svelte';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import DetailHero from '$lib/components/ui/DetailHero.svelte';
	import ActionBar from '$lib/components/ui/ActionBar.svelte';
	import ErrorState from '$lib/components/ui/ErrorState.svelte';
	import SelectionBar from '$lib/components/ui/SelectionBar.svelte';
	import Dropdown from '$lib/components/ui/Dropdown.svelte';
	import SearchField from '$lib/search/ui/SearchField.svelte';
	import PlaylistDetailsDialog from '$lib/components/playlist/PlaylistDetailsDialog.svelte';
	import PlaylistAddTracks from '$lib/components/playlist/PlaylistAddTracks.svelte';
	import { openContextMenu, openMenuAtElement } from '$lib/stores/context_menu';
	import { buildPlaylistMenu, buildAddToPlaylistSubmenu } from '$lib/player/playlist_menu';
	import { goBack } from '$lib/navigation/back';
	import { captureScroll, restoreScroll } from '$lib/navigation/scroll';
	import { pickArtworkUrls, nameToGradient } from '$lib/stores/playlist_artwork_cache';
	import { formatTotalDuration } from '$lib/utils/format';

	type ViewSort = 'playlist' | 'title' | 'artist' | 'album' | 'duration';
	const VIEW_SORTS: readonly { value: ViewSort; label: string }[] = [
		{ value: 'playlist', label: 'Playlist order' },
		{ value: 'title', label: 'Title' },
		{ value: 'artist', label: 'Artist' },
		{ value: 'album', label: 'Album' },
		{ value: 'duration', label: 'Duration' },
	];
	// A view sort only changes what is shown; the playlist's order is untouched.
	const viewSort = createPersistedStore<ViewSort>('playlistDetail.sort', 'playlist', {
		parse: oneOf(VIEW_SORTS.map((option) => option.value)),
	});

	/** A playlist row and its position in the stored order. */
	type Entry = { track: Track; position: number };

	const UNDO_MS = 6000;

	let playlistId = $derived(Number(page.params.id));

	let playlist = $state<Playlist | null>(null);
	let tracks = $state<Track[]>([]);
	let allPlaylists = $state<Playlist[]>([]);
	let loading = $state(true);
	let error = $state<string | null>(null);
	let busy = $state(false);
	let loadSeq = 0;

	let detailsOpen = $state(false);
	let addOpen = $state(false);
	let pendingDelete = $state(false);
	let viewQuery = $state('');

	// Removal waits UNDO_MS behind an Undo toast before it reaches the server
	// (and TIDAL), so Undo never has to rebuild anything. Rows are hidden
	// meanwhile. Positions are stored positions.
	let pendingRemoval = $state(new Set<number>());
	let removalTimer: ReturnType<typeof setTimeout> | undefined;
	let removalToast: number | null = null;
	let committingRemoval = false;

	// Keyed by stored position, not track id: a playlist can hold the same track
	// twice, and selecting one copy must not select (and remove) the other.
	const selection = createSelection();
	const selectedPositions = selection.ids;
	const lastSelected = selection.lastId;

	export const snapshot: Snapshot<{ scrollY: number }> = {
		capture: () => ({ scrollY: captureScroll() }),
		restore: (saved) => restoreScroll(saved.scrollY),
	};

	// A smart playlist's contents come from its rules, so nothing here is
	// editable; the page renders as a read-only preview of the evaluation.
	let isSmart = $derived(playlist?.is_smart === true);
	let isTidal = $derived(Boolean(playlist?.tidal_uuid));
	let editable = $derived(playlist !== null && !isSmart);

	// Snippet bodies do not inherit the `{:else if playlist}` narrowing from the
	// markup, so anything a snippet reads goes through a null-safe derived.
	let playlistName = $derived(playlist?.name ?? '');
	let playlistDescription = $derived(playlist?.description ?? null);
	let isFavorite = $derived(playlist?.is_favorite === true);
	let tidalUpdated = $derived(formatSyncDate(playlist?.tidal_last_updated));

	let entries = $derived<Entry[]>(
		tracks
			.map((track, position) => ({ track, position }))
			.filter((entry) => !pendingRemoval.has(entry.position)),
	);
	let liveTracks = $derived(entries.map((entry) => entry.track));
	let presentIds = $derived(new Set(liveTracks.map((track) => track.id)));
	let visible = $derived(sortEntries(filterEntries(entries, viewQuery), $viewSort));
	// Reordering only means something when rows are shown in stored order.
	let inPlaylistOrder = $derived($viewSort === 'playlist' && !viewQuery.trim());
	let canReorder = $derived(editable && inPlaylistOrder);
	let duplicatePositions = $derived(laterCopies(entries));

	let mosaic = $derived(pickArtworkUrls(liveTracks));
	let totalDuration = $derived(
		formatTotalDuration(liveTracks.reduce((sum, track) => sum + (track.duration_ms ?? 0), 0)),
	);
	let sourceLabel = $derived(isSmart ? 'Smart' : isTidal ? 'TIDAL' : 'Local');

	onMount(() => {
		void load();
		const unsubscribeWs = wsMessages.subscribe((messages) => {
			if (messages.at(-1)?.type === 'playlists_changed') void load();
		});
		// Leaving the app inside the undo window still applies the removal.
		const flush = () => void commitRemoval();
		window.addEventListener('pagehide', flush);
		return () => {
			unsubscribeWs();
			window.removeEventListener('pagehide', flush);
		};
	});

	// Navigating away inside the undo window applies the removal too.
	onDestroy(() => void commitRemoval());

	async function load() {
		const seq = ++loadSeq;
		loading = true;
		error = null;
		try {
			const [listData, trackData] = await Promise.all([
				cachedApi.getPlaylists(),
				cachedApi.getPlaylistTracks(playlistId),
			]);
			if (seq !== loadSeq) return;
			allPlaylists = listData.playlists;
			playlist = listData.playlists.find((p) => p.id === playlistId) ?? null;
			if (trackOrder(trackData.tracks) !== trackOrder(tracks)) {
				// Positions mean something else once the order changes underneath.
				selection.clear();
				if (pendingRemoval.size && !committingRemoval) {
					cancelRemoval();
					showToast('The playlist changed, so nothing was removed', 'info');
				}
			}
			tracks = trackData.tracks;
			if (!playlist) error = 'That playlist no longer exists.';
		} catch (reason) {
			if (seq !== loadSeq) return;
			error = `Failed to load playlist: ${reason}`;
		} finally {
			if (seq === loadSeq) loading = false;
		}
	}

	function trackOrder(list: Track[]) {
		return list.map((track) => track.id).join(',');
	}

	function formatSyncDate(raw: string | null | undefined) {
		if (!raw) return null;
		// Stored as UTC "YYYY-MM-DD HH:MM:SS".
		const date = new Date(`${raw.replace(' ', 'T')}Z`);
		if (Number.isNaN(date.getTime())) return null;
		return date.toLocaleDateString(undefined, { day: 'numeric', month: 'short', year: 'numeric' });
	}

	function filterEntries(list: Entry[], query: string) {
		const needle = query.trim().toLowerCase();
		if (!needle) return list;
		return list.filter(({ track }) =>
			[track.title, track.artist_name, track.album_title].some((field) =>
				field?.toLowerCase().includes(needle),
			),
		);
	}

	function sortEntries(list: Entry[], sort: ViewSort) {
		if (sort === 'playlist') return list;
		const text = (value: string | null | undefined) => value ?? '';
		const key: Record<Exclude<ViewSort, 'playlist'>, (a: Entry, b: Entry) => number> = {
			title: (a, b) => text(a.track.title).localeCompare(text(b.track.title)),
			artist: (a, b) => text(a.track.artist_name).localeCompare(text(b.track.artist_name)),
			album: (a, b) => text(a.track.album_title).localeCompare(text(b.track.album_title)),
			duration: (a, b) => (a.track.duration_ms ?? 0) - (b.track.duration_ms ?? 0),
		};
		return [...list].sort((a, b) => key[sort](a, b) || a.position - b.position);
	}

	/** Positions of every copy of a track after its first. */
	function laterCopies(list: Entry[]) {
		const seen = new Set<number>();
		const later: number[] = [];
		for (const { track, position } of list) {
			if (seen.has(track.id)) later.push(position);
			else seen.add(track.id);
		}
		return later;
	}

	// Playlist routes answer in sentences ("This playlist changed on TIDAL.
	// Refresh it and try again."). Show those; never "ApiError: ...".
	function failureText(failure: string, reason: unknown) {
		if (reason instanceof ApiError && reason.status !== 500 && !/^API error/i.test(reason.message)) {
			return `${failure}. ${reason.message}`;
		}
		return `${failure}. Try again.`;
	}

	/**
	 * Run a mutation, refresh, and surface failures as a toast. Mutations run
	 * one at a time, in the order they were asked for, so a removal committing
	 * from its timer never collides with a reorder or a favourite toggle.
	 */
	let queue: Promise<unknown> = Promise.resolve();
	function mutate(action: () => Promise<unknown>, failure: string): Promise<boolean> {
		const run = queue.then(async () => {
			busy = true;
			try {
				await action();
				invalidatePlaylistCaches();
				await load();
				return true;
			} catch (reason) {
				showToast(failureText(failure, reason), 'error');
				return false;
			} finally {
				busy = false;
			}
		});
		queue = run;
		return run;
	}

	async function playAll(startTrackId?: number) {
		if (!liveTracks.length) return;
		await playTracksInContext(
			liveTracks.map((t) => t.id),
			startTrackId,
		);
	}

	// A row plays the list as shown, so a sorted view plays in that order.
	async function playFromRow(track: Track) {
		await playTracksInContext(
			visible.map((entry) => entry.track.id),
			track.id,
		);
	}

	// --- Removal with undo -------------------------------------------------------
	function removePositions(positions: number[]) {
		if (!editable || !positions.length) return;
		pendingRemoval = new Set([...pendingRemoval, ...positions]);
		selection.clear();
		const count = pendingRemoval.size;
		if (removalToast !== null) dismissToast(removalToast);
		removalToast = showToast(`Removed ${count} ${count === 1 ? 'track' : 'tracks'}`, 'info', UNDO_MS, [
			{ label: 'Undo', onClick: cancelRemoval },
		]);
		clearTimeout(removalTimer);
		removalTimer = setTimeout(() => void commitRemoval(), UNDO_MS);
	}

	function removeSelected() {
		removePositions([...$selectedPositions]);
	}

	function cancelRemoval() {
		clearTimeout(removalTimer);
		if (removalToast !== null) dismissToast(removalToast);
		removalToast = null;
		pendingRemoval = new Set();
	}

	/** Apply the pending removal now. Resolves false if it failed (rows return). */
	async function commitRemoval(): Promise<boolean> {
		clearTimeout(removalTimer);
		if (removalToast !== null) dismissToast(removalToast);
		removalToast = null;
		const positions = [...pendingRemoval].sort((a, b) => a - b);
		if (!positions.length) return true;
		committingRemoval = true;
		const ok = await mutate(
			() => api.removePlaylistTracks(playlistId, positions),
			'Could not remove those tracks',
		);
		committingRemoval = false;
		pendingRemoval = new Set();
		return ok;
	}

	// ─── Reorder ──────────────────────────────────────────────────────────────
	// Shares the queue's drag action, so the index conversion and the Firefox
	// dragstart quirk are handled in one place. Rows are keyed by stored
	// position, which distinguishes repeated tracks.
	const drag = createDragReorder({
		indexOf: (position) => visible.findIndex((entry) => entry.position === position),
		length: () => visible.length,
		canDrag: () => canReorder && !busy,
		onDrop: (position, toIndex) =>
			moveTrack(
				visible.findIndex((entry) => entry.position === position),
				toIndex,
			),
	});
	const dragState = drag.state;

	/** Move the row shown at `from` to `to` (both after lifting it out). */
	async function moveTrack(from: number, to: number) {
		if (!canReorder || from < 0 || from === to) return;
		// Hidden rows would make shown indices disagree with stored positions.
		if (pendingRemoval.size && !(await commitRemoval())) return;
		// Optimistic: the drop indicator already showed the user where it lands,
		// so snapping back on a slow round trip would read as a dropped input.
		// A failed move does snap back, so the list never shows an order the
		// server (or TIDAL) does not have.
		const previous = tracks;
		const next = [...tracks];
		const [moved] = next.splice(from, 1);
		next.splice(Math.min(Math.max(to, 0), next.length), 0, moved);
		tracks = next;
		selection.clear();
		const ok = await mutate(() => api.movePlaylistTrack(playlistId, from, to), 'Could not reorder');
		if (!ok) tracks = previous;
	}

	// --- Playlist actions --------------------------------------------------------
	async function confirmDelete() {
		cancelRemoval();
		const ok = await mutate(() => api.deletePlaylist(playlistId), 'Could not delete');
		if (ok) await goto('/playlists');
	}

	async function refreshFromTidal() {
		if (pendingRemoval.size && !(await commitRemoval())) return;
		const ok = await mutate(
			() => api.refreshPlaylistFromTidal(playlistId),
			'Could not refresh from TIDAL',
		);
		if (ok) showToast('Refreshed from TIDAL', 'success');
	}

	async function toggleFavorite() {
		await mutate(() => api.togglePlaylistFavorite(playlistId), 'Could not update favourite');
	}

	// --- Keyboard ----------------------------------------------------------------
	function isTyping(target: EventTarget | null) {
		return (
			target instanceof HTMLElement &&
			(target.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName))
		);
	}

	// Delete removes the selection, Escape clears it, Ctrl/Cmd+A selects every
	// row shown. Left alone while typing, while a dialog is open, or when another
	// handler (the context menu) already took the key.
	function handleListKeys(event: KeyboardEvent) {
		if (!editable || event.defaultPrevented || isTyping(event.target)) return;
		if (document.querySelector('dialog[open]')) return;
		if ((event.key === 'Delete' || event.key === 'Backspace') && $selectedPositions.size) {
			event.preventDefault();
			removeSelected();
		} else if (event.key === 'Escape' && $selectedPositions.size) {
			event.preventDefault();
			selection.clear();
		} else if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'a' && visible.length) {
			event.preventDefault();
			selection.select(visible.map((entry) => entry.position));
		}
	}

	// ─── Menus ────────────────────────────────────────────────────────────────
	function playlistMenuItems() {
		if (!playlist) return [];
		return buildPlaylistMenu(playlist, {
			onPlay: () => void playAll(),
			onShuffle: () => void shufflePlaylist(liveTracks),
			onRadio: () => void startPlaylistRadio(liveTracks),
			onToggleFavorite: () => void toggleFavorite(),
			onEditDetails: editable ? () => (detailsOpen = true) : undefined,
			onEditRules: isSmart ? () => void goto(`/playlists?edit=${playlistId}`) : undefined,
			onRemoveDuplicates:
				editable && duplicatePositions.length ? () => removePositions(duplicatePositions) : undefined,
			onDownload: () => void downloadPlaylist(playlistId),
			onRefreshFromTidal: isTidal ? () => void refreshFromTidal() : undefined,
			onDelete: () => {
				pendingDelete = true;
			},
		});
	}

	function openHeaderMenu(event: MouseEvent) {
		event.preventDefault();
		event.stopPropagation();
		if (!playlist) return;
		openMenuAtElement(event.currentTarget as HTMLElement, playlistMenuItems(), playlist.name);
	}

	function openHeaderContextMenu(event: MouseEvent) {
		event.preventDefault();
		if (!playlist) return;
		openContextMenu(event, playlistMenuItems(), playlist.name);
	}

	function trackMenuOptions(entry: Entry, shownAt: number) {
		const last = visible.length - 1;
		return {
			addToPlaylistSubmenu: buildAddToPlaylistSubmenu(allPlaylists, async () => [entry.track.id]),
			onRemoveFromPlaylist: editable ? () => removePositions([entry.position]) : undefined,
			onMoveToTop: canReorder && shownAt > 0 ? () => void moveTrack(shownAt, 0) : undefined,
			onMoveToBottom: canReorder && shownAt < last ? () => void moveTrack(shownAt, last) : undefined,
		};
	}

	function handleSelect(event: MouseEvent | KeyboardEvent, position: number) {
		const additive = event.ctrlKey || event.metaKey;
		const anchor = $lastSelected;
		if (event.shiftKey && anchor !== null) {
			// Range-select, in the order shown, from the last clicked row to this one.
			const order = visible.map((entry) => entry.position);
			const [a, b] = [order.indexOf(anchor), order.indexOf(position)];
			if (a >= 0 && b >= 0) {
				selection.select(order.slice(Math.min(a, b), Math.max(a, b) + 1), true);
				return;
			}
		}
		selection.select([position], additive);
	}
</script>

<svelte:head>
	<title>{playlist?.name ?? 'Playlist'} | NOOR</title>
</svelte:head>

<svelte:window onkeydown={handleListKeys} />

<div class="page-shell playlist-detail animate-in">
	<button class="back-link" type="button" onclick={() => goBack('/playlists')}>Back</button>

	{#if error}
		<ErrorState title="Playlist unavailable" {error} onretry={() => void load()}>
			{#snippet actions()}
				<button class="btn btn-glass" onclick={() => goto('/playlists')}>All playlists</button>
			{/snippet}
		</ErrorState>
	{:else if loading && !playlist}
		<Skeleton rows={6} label="Loading playlist" />
	{:else if playlist}
		<DetailHero
			eyebrow="Playlist"
			title={playlistName}
			backdrop={mosaic}
			fallbackText={playlistName.slice(0, 1)}
			variant="immersive"
			label="Playlist header"
			oncontextmenu={openHeaderContextMenu}
		>
			{#snippet cover()}
				<div
					class="playlist-cover"
					class:has-mosaic={mosaic.length >= 4}
					style:background={mosaic.length === 0 ? nameToGradient(playlistName) : undefined}
				>
					{#if mosaic.length >= 4}
						{#each mosaic.slice(0, 4) as url (url)}
							<ArtworkImage src={url} size={320} className="playlist-cover-art" decorative />
						{/each}
					{:else if mosaic.length > 0}
						<ArtworkImage src={mosaic[0]} size={640} className="playlist-cover-art" decorative />
					{:else}
						<span class="playlist-initial" aria-hidden="true">{playlistName.slice(0, 1)}</span>
					{/if}
				</div>
			{/snippet}
			{#snippet meta()}
				<StateBadge label={sourceLabel} tone={isSmart ? 'active' : 'muted'} compact />
				<span>{liveTracks.length} {liveTracks.length === 1 ? 'track' : 'tracks'}</span>
				{#if totalDuration}
					<span aria-hidden="true">&middot;</span><span>{totalDuration}</span>
				{/if}
				{#if isTidal && tidalUpdated}
					<span aria-hidden="true">&middot;</span><span title="When this playlist last changed on TIDAL">Updated on TIDAL {tidalUpdated}</span>
				{/if}
			{/snippet}
			{#snippet details()}
				{#if playlistDescription}<p class="playlist-desc t-body">{playlistDescription}</p>{/if}
			{/snippet}
			{#snippet actions()}
				<ActionBar
					onplay={liveTracks.length ? () => void playAll() : undefined}
					onshuffle={liveTracks.length ? () => void shufflePlaylist(liveTracks) : undefined}
					shuffleHint="Play this playlist in random order"
					onradio={liveTracks.length ? () => void startPlaylistRadio(liveTracks) : undefined}
					radioHint="Similar tracks across your library and TIDAL"
					liked={isFavorite}
					onlike={() => void toggleFavorite()}
					likeLabel="Add to favourites"
					unlikeLabel="Remove from favourites"
					onmore={openHeaderMenu}
				>
					{#if editable}
						<button class="btn btn-secondary" aria-expanded={addOpen} onclick={() => (addOpen = !addOpen)}>
							Add tracks
						</button>
					{/if}
					{#if isSmart}
						<a class="btn btn-secondary" href={`/playlists?edit=${playlistId}`}>Edit rules</a>
					{/if}
					{#if isTidal}
						<button class="btn btn-ghost" disabled={busy} onclick={() => void refreshFromTidal()}>Refresh from TIDAL</button>
					{/if}
				</ActionBar>
			{/snippet}
		</DetailHero>

		{#if pendingDelete}
			<div class="confirm-strip">
				<span>Delete "{playlist.name}"{isTidal ? ' from TIDAL too' : ''}? This cannot be undone.</span>
				<button class="btn btn-glass btn-sm" onclick={() => (pendingDelete = false)}>Cancel</button>
				<button class="btn btn-sm danger-solid" disabled={busy} onclick={() => void confirmDelete()}>
					Delete
				</button>
			</div>
		{/if}

		{#if addOpen && editable}
			<PlaylistAddTracks
				{playlistId}
				present={presentIds}
				onAdded={() => void load()}
				onClose={() => (addOpen = false)}
			/>
		{/if}

		{#if $selectedPositions.size > 0 && editable}
			<SelectionBar summary={`${$selectedPositions.size} selected`} sticky>
				{#snippet actions()}
					<button class="btn btn-glass btn-sm" onclick={() => selection.clear()}>Clear</button>
					<button class="btn btn-sm danger-solid" onclick={removeSelected}>Remove from playlist</button>
				{/snippet}
			</SelectionBar>
		{/if}

		{#if liveTracks.length === 0}
			<EmptyState
				title="Nothing in here yet"
				copy={editable ? 'Search for tracks to add, or add them from the library, search, or an album page.' : 'No tracks match these rules yet.'}
			>
				{#snippet actions()}
					{#if editable && !addOpen}
						<button class="btn btn-primary" onclick={() => (addOpen = true)}>Add tracks</button>
					{/if}
				{/snippet}
			</EmptyState>
		{:else}
			<div class="list-toolbar">
				<p class="t-meta list-count">
					{#if viewQuery.trim()}
						{visible.length} of {liveTracks.length}
					{:else}
						{liveTracks.length} {liveTracks.length === 1 ? 'track' : 'tracks'}
					{/if}
					{#if editable && !inPlaylistOrder}
						<span class="reorder-hint"> - switch to Playlist order with no filter to drag</span>
					{/if}
				</p>
				<div class="list-tools">
					<SearchField bind:value={viewQuery} size="sm" placeholder="Filter tracks" ariaLabel="Filter tracks" />
					<Dropdown label="Sort" options={VIEW_SORTS} value={$viewSort} onchange={(value) => viewSort.set(value)} />
				</div>
			</div>

			{#if visible.length === 0}
				<EmptyState title="No tracks match" copy="Try a different filter.">
					{#snippet actions()}
						<button class="btn btn-glass" onclick={() => (viewQuery = '')}>Clear filter</button>
					{/snippet}
				</EmptyState>
			{:else}
				<ol class="detail-tracks" class:refreshing={busy} aria-label="Playlist tracks" aria-busy={busy}>
					{#each visible as entry, shownAt (entry.position)}
						<li
							class="detail-track"
							class:dragging={$dragState.draggingId === entry.position}
							class:drag-over={$dragState.dragOverId === entry.position &&
								$dragState.draggingId !== entry.position}
							draggable={canReorder}
							use:drag.row={entry.position}
						>
							{#if canReorder}
								<span class="detail-grip" aria-hidden="true" title="Drag to reorder (or Alt+Up/Down)">&#8942;&#8942;</span>
							{/if}
							<TrackRow
								track={entry.track}
								variant="numbered"
								index={entry.position}
								isCurrent={$currentTrack?.id === entry.track.id}
								isPlaying={$isPlaying && $currentTrack?.id === entry.track.id}
								selected={$selectedPositions.has(entry.position)}
								onSelect={editable ? (event) => handleSelect(event, entry.position) : undefined}
								onRowClick={() => void playFromRow(entry.track)}
								menuOptions={trackMenuOptions(entry, shownAt)}
							/>
						</li>
					{/each}
				</ol>
			{/if}
		{/if}
	{/if}
</div>

{#if detailsOpen && playlist}
	<PlaylistDetailsDialog {playlist} onClose={() => (detailsOpen = false)} onSaved={() => void load()} />
{/if}

<style>
	.playlist-detail {
		width: min(100%, var(--content-width));
		margin: 0 auto;
	}

	.playlist-cover {
		display: grid;
		width: 100%;
		height: 100%;
		place-items: center;
		overflow: hidden;
	}

	.playlist-cover.has-mosaic {
		grid-template-columns: 1fr 1fr;
		grid-template-rows: 1fr 1fr;
	}

	.playlist-cover :global(.playlist-cover-art) {
		width: 100%;
		height: 100%;
		object-fit: cover;
		display: block;
	}

	.playlist-initial {
		color: var(--text-primary);
		font-size: var(--font-size-3xl);
		font-weight: var(--font-weight-semibold);
		line-height: 1;
	}

	.playlist-desc {
		margin: 0;
		max-width: var(--measure-text);
		color: var(--text-secondary);
	}

	.confirm-strip {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--space-2);
		margin-bottom: var(--space-3);
		padding: var(--space-2) var(--space-3);
		border-radius: var(--radius-sm);
		color: var(--text-primary);
		font-size: var(--font-size-xs);
	}

	.danger-solid {
		border-color: transparent;
		background: var(--state-error);
		color: #fff;
	}

	/* Same 36px toolbar row as the command headers (STYLING.md). */
	.list-toolbar {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-3);
		min-height: 36px;
		margin-bottom: var(--group-gap);
	}

	.list-count {
		margin: 0;
	}

	.reorder-hint {
		color: var(--text-tertiary);
	}

	.list-tools {
		display: flex;
		align-items: center;
		gap: var(--space-2);
	}

	.detail-tracks {
		display: flex;
		flex-direction: column;
		margin: 0;
		padding: 0;
		list-style: none;
		transition: opacity var(--motion-fast);
	}

	/* STYLING.md "States": a refresh keeps the list, dimmed. */
	.detail-tracks.refreshing {
		opacity: 0.55;
	}

	.detail-track {
		display: grid;
		align-items: center;
		grid-template-columns: auto minmax(0, 1fr);
		border-radius: var(--radius-sm);
		/* Browser-native virtualization: long playlists stay cheap to paint. */
		content-visibility: auto;
		contain-intrinsic-size: 0 56px;
	}

	.detail-track.dragging {
		opacity: 0.4;
		cursor: grabbing;
	}

	.detail-track.drag-over {
		background: color-mix(in srgb, var(--accent-soft) 55%, transparent);
		box-shadow: inset 0 2px 0 var(--accent-strong);
	}

	.detail-grip {
		width: 14px;
		color: var(--text-tertiary);
		cursor: grab;
		opacity: 0.35;
		text-align: center;
		transition: opacity var(--motion-fast);
	}

	.detail-track:hover .detail-grip,
	.detail-track:focus-within .detail-grip {
		opacity: 0.8;
	}

	@container workspace (max-width: 760px) {
		.list-tools {
			width: 100%;
		}

		.list-tools :global(> :first-child) {
			flex: 1;
		}
	}
</style>
