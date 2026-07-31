<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import type { Snapshot } from './$types';
	import { api, type Playlist, type Track } from '$lib/api/client';
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
	import { showToast } from '$lib/stores/toast';
	import { createSelection } from '$lib/stores/selection';
	import { createDragReorder } from '$lib/actions/drag_reorder';
	import TrackRow from '$lib/components/TrackRow.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import StateBadge from '$lib/components/ui/StateBadge.svelte';
	import Skeleton from '$lib/components/ui/Skeleton.svelte';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import SelectionBar from '$lib/components/ui/SelectionBar.svelte';
	import { openContextMenu, openMenuAtElement } from '$lib/stores/context_menu';
	import { buildPlaylistMenu, buildAddToPlaylistSubmenu } from '$lib/player/playlist_menu';
	import { goBack } from '$lib/navigation/back';
	import { captureScroll, restoreScroll } from '$lib/navigation/scroll';
	import { pickArtworkUrls, nameToGradient } from '$lib/stores/playlist_artwork_cache';
	import { formatTotalDuration } from '$lib/utils/format';
	import { upscaleTidalArtwork } from '$lib/utils/artwork';

	let playlistId = $derived(Number(page.params.id));

	let playlist = $state<Playlist | null>(null);
	let tracks = $state<Track[]>([]);
	let allPlaylists = $state<Playlist[]>([]);
	let loading = $state(true);
	let error = $state<string | null>(null);
	let busy = $state(false);
	let loadSeq = 0;

	// Inline rename. The header title becomes an input rather than opening a
	// dialog, matching how the smart-rule name field already behaves.
	let renaming = $state(false);
	let draftName = $state('');

	let pendingDelete = $state(false);

	const selection = createSelection();
	const selectedIds = selection.ids;

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

	let mosaic = $derived(pickArtworkUrls(tracks));
	let totalDuration = $derived(
		formatTotalDuration(tracks.reduce((sum, track) => sum + (track.duration_ms ?? 0), 0)),
	);
	let sourceLabel = $derived(isSmart ? 'Smart' : isTidal ? 'TIDAL' : 'Local');

	// Blurred backdrop for the hero. A decorative CSS background, so the URL is
	// normalized through upscaleTidalArtwork first per the artwork rules; the
	// heavy blur is why 1280 is not overkill here.
	let bannerUrl = $derived(mosaic.length > 0 ? upscaleTidalArtwork(mosaic[0], 1280) : null);

	onMount(() => {
		void load();
		const unsubscribeWs = wsMessages.subscribe((messages) => {
			if (messages.at(-1)?.type === 'playlists_changed') void load();
		});
		return unsubscribeWs;
	});

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
			tracks = trackData.tracks;
			if (!playlist) error = 'That playlist no longer exists.';
		} catch (reason) {
			if (seq !== loadSeq) return;
			error = `Failed to load playlist: ${reason}`;
		} finally {
			if (seq === loadSeq) loading = false;
		}
	}

	/**
	 * Run a mutation, refresh, and surface failures as a toast.
	 *
	 * Every playlist write can fail remotely (a 409 means TIDAL moved underneath
	 * us), so none of these are optimistic except the drag reorder, which has a
	 * visible drop position to honour.
	 */
	async function mutate(action: () => Promise<unknown>, failure: string) {
		if (busy) return false;
		busy = true;
		try {
			await action();
			invalidatePlaylistCaches();
			await load();
			return true;
		} catch (reason) {
			showToast(`${failure}: ${reason}`, 'error');
			return false;
		} finally {
			busy = false;
		}
	}

	async function playAll(startTrackId?: number) {
		if (!tracks.length) return;
		await playTracksInContext(
			tracks.map((t) => t.id),
			startTrackId,
		);
	}

	async function removePositions(positions: number[]) {
		if (!positions.length) return;
		const ok = await mutate(
			() => api.removePlaylistTracks(playlistId, positions),
			'Could not remove those tracks',
		);
		if (ok) selection.clear();
	}

	async function removeSelected() {
		const ids = new Set($selectedIds);
		const positions = tracks
			.map((track, index) => (ids.has(track.id) ? index : -1))
			.filter((index) => index >= 0);
		await removePositions(positions);
	}

	// ─── Reorder ──────────────────────────────────────────────────────────────
	// Shares the queue's drag action, so the index conversion and the Firefox
	// dragstart quirk are handled in one place. Rows are keyed by track id.
	const drag = createDragReorder({
		indexOf: (id) => tracks.findIndex((track) => track.id === id),
		length: () => tracks.length,
		canDrag: () => editable,
		onDrop: (id, toIndex) =>
			moveTrack(
				tracks.findIndex((t) => t.id === id),
				toIndex,
			),
	});
	const dragState = drag.state;

	async function moveTrack(from: number, to: number) {
		if (from < 0 || from === to) return;
		// Optimistic: the drop indicator already showed the user where it lands,
		// so snapping back on a slow round trip would read as a dropped input.
		const next = [...tracks];
		const [moved] = next.splice(from, 1);
		next.splice(Math.min(Math.max(to, 0), next.length), 0, moved);
		tracks = next;
		await mutate(() => api.movePlaylistTrack(playlistId, from, to), 'Could not reorder');
	}

	// ─── Rename / delete ──────────────────────────────────────────────────────
	function startRename() {
		if (!playlist) return;
		draftName = playlist.name;
		renaming = true;
	}

	async function commitRename() {
		const name = draftName.trim();
		if (!playlist || !name || name === playlist.name) {
			renaming = false;
			return;
		}
		const ok = await mutate(
			() => api.updatePlaylist(playlistId, name, playlist?.description ?? null),
			'Could not rename',
		);
		if (ok) renaming = false;
	}

	function handleRenameKeydown(event: KeyboardEvent) {
		if (event.key === 'Enter') {
			event.preventDefault();
			void commitRename();
		} else if (event.key === 'Escape') {
			event.preventDefault();
			renaming = false;
		}
	}

	async function confirmDelete() {
		const ok = await mutate(() => api.deletePlaylist(playlistId), 'Could not delete');
		if (ok) await goto('/playlists');
	}

	async function refreshFromTidal() {
		const ok = await mutate(
			() => api.refreshPlaylistFromTidal(playlistId),
			'Could not refresh from TIDAL',
		);
		if (ok) showToast('Refreshed from TIDAL', 'success');
	}

	async function toggleFavorite() {
		await mutate(() => api.togglePlaylistFavorite(playlistId), 'Could not update favourite');
	}

	// ─── Menus ────────────────────────────────────────────────────────────────
	function playlistMenuItems() {
		if (!playlist) return [];
		return buildPlaylistMenu(playlist, {
			onPlay: () => void playAll(),
			onShuffle: () => void shufflePlaylist(tracks),
			onRadio: () => void startPlaylistRadio(tracks),
			onToggleFavorite: () => void toggleFavorite(),
			onRename: editable ? startRename : undefined,
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

	function trackMenuOptions(index: number) {
		return {
			addToPlaylistSubmenu: buildAddToPlaylistSubmenu(allPlaylists, async () => [
				tracks[index].id,
			]),
			onRemoveFromPlaylist: editable ? () => void removePositions([index]) : undefined,
		};
	}

	function handleSelect(event: MouseEvent | KeyboardEvent, index: number) {
		const additive = event.ctrlKey || event.metaKey;
		if (event.shiftKey && $selectedIds.size > 0) {
			// Range-select from the first already-selected row to this one.
			const anchor = tracks.findIndex((track) => $selectedIds.has(track.id));
			const [from, to] = anchor <= index ? [anchor, index] : [index, anchor];
			selection.select(
				tracks.slice(from, to + 1).map((track) => track.id),
				true,
			);
			return;
		}
		selection.select([tracks[index].id], additive);
	}
</script>

<svelte:head>
	<title>{playlist?.name ?? 'Playlist'} | NOOR</title>
</svelte:head>

<div class="page-shell playlist-detail animate-in">
	<button class="back-link" onclick={() => goBack('/playlists')}>Back</button>

	{#if error}
		<EmptyState title="Playlist unavailable" copy={error}>
			{#snippet actions()}
				<button class="btn btn-glass" onclick={() => goto('/playlists')}>All playlists</button>
			{/snippet}
		</EmptyState>
	{:else if loading && !playlist}
		<Skeleton rows={6} label="Loading playlist" />
	{:else if playlist}
		<!-- One hero block. Title, source, counts, artwork and every action used to
		     be scattered between a PageHeader (title far left, actions far right on a
		     wide screen), a separate artwork row, a floating Rename, and a note. -->
		<header class="detail-hero" role="group" aria-label="Playlist header" oncontextmenu={openHeaderContextMenu}>
			{#if bannerUrl}
				<div class="detail-banner" style:background-image={`url("${bannerUrl}")`} aria-hidden="true"></div>
			{/if}
			<div class="detail-hero-inner">
				<div
					class="detail-cover"
					class:has-mosaic={mosaic.length >= 4}
					style:background={mosaic.length === 0 ? nameToGradient(playlistName) : undefined}
				>
					{#if mosaic.length >= 4}
						{#each mosaic.slice(0, 4) as url (url)}
							<ArtworkImage src={url} size={320} className="detail-cover-art" decorative />
						{/each}
					{:else if mosaic.length > 0}
						<ArtworkImage src={mosaic[0]} size={640} className="detail-cover-art" decorative />
					{:else}
						<span class="detail-initial" aria-hidden="true">{playlistName.slice(0, 1)}</span>
					{/if}
				</div>

				<div class="detail-headings">
					<p class="eyebrow">Playlist</p>

					{#if renaming}
						<form
							class="rename-form"
							onsubmit={(event) => {
								event.preventDefault();
								void commitRename();
							}}
						>
							<label class="sr-only" for="playlist-rename">Playlist name</label>
							<!-- svelte-ignore a11y_autofocus -->
							<input
								id="playlist-rename"
								bind:value={draftName}
								onkeydown={handleRenameKeydown}
								autofocus
								autocomplete="off"
							/>
							<button class="btn btn-primary btn-sm" type="submit" disabled={busy}>Save</button>
							<button class="btn btn-glass btn-sm" type="button" onclick={() => (renaming = false)}>
								Cancel
							</button>
						</form>
					{:else}
						<h1 class="detail-title">{playlistName}</h1>
					{/if}

					<p class="detail-meta">
						<StateBadge label={sourceLabel} tone={isSmart ? 'active' : 'muted'} compact />
						<span>{tracks.length} {tracks.length === 1 ? 'track' : 'tracks'}</span>
						{#if totalDuration}
							<span aria-hidden="true">&middot;</span><span>{totalDuration}</span>
						{/if}
						{#if playlistDescription}
							<span aria-hidden="true">&middot;</span><span class="detail-desc">{playlistDescription}</span>
						{/if}
					</p>

					<div class="detail-actions">
						<button class="btn btn-primary" disabled={!tracks.length} onclick={() => void playAll()}>
							Play
						</button>
						<button
							class="btn btn-glass"
							disabled={!tracks.length}
							onclick={() => void shufflePlaylist(tracks)}
						>
							Shuffle
						</button>
						<button
							class="btn btn-glass"
							disabled={!tracks.length}
							onclick={() => void startPlaylistRadio(tracks)}
						>
							Radio
						</button>
						<button
							class="icon-btn"
							class:active={isFavorite}
							aria-label={isFavorite ? 'Remove from favourites' : 'Add to favourites'}
							aria-pressed={isFavorite}
							title={isFavorite ? 'Remove from favourites' : 'Add to favourites'}
							onclick={() => void toggleFavorite()}
						>
							&#9829;
						</button>
						<button class="icon-btn" aria-label="More actions" title="More actions" onclick={openHeaderMenu}>
							&#8943;
						</button>
					</div>
				</div>
			</div>
		</header>

		{#if pendingDelete}
			<div class="confirm-strip glass">
				<span>Delete "{playlist.name}"{isTidal ? ' from TIDAL too' : ''}? This cannot be undone.</span>
				<button class="btn btn-glass btn-sm" onclick={() => (pendingDelete = false)}>Cancel</button>
				<button class="btn btn-sm danger-solid" disabled={busy} onclick={() => void confirmDelete()}>
					Delete
				</button>
			</div>
		{/if}

		{#if $selectedIds.size > 0 && editable}
			<SelectionBar summary={`${$selectedIds.size} selected`} sticky>
				{#snippet actions()}
					<button class="btn btn-glass btn-sm" onclick={() => selection.clear()}>Clear</button>
					<button class="btn btn-sm danger-solid" disabled={busy} onclick={() => void removeSelected()}>
						Remove from playlist
					</button>
				{/snippet}
			</SelectionBar>
		{/if}

		{#if tracks.length === 0}
			<EmptyState
				title="Nothing in here yet"
				copy="Add tracks from the library, search, or an album page."
			/>
		{:else}
			<ol class="detail-tracks" aria-label="Playlist tracks">
				{#each tracks as track, index (`${track.id}-${index}`)}
					<li
						class="detail-track"
						class:dragging={$dragState.draggingId === track.id}
						class:drag-over={$dragState.dragOverId === track.id &&
							$dragState.draggingId !== track.id}
						draggable={editable}
						use:drag.row={track.id}
					>
						{#if editable}
							<span class="detail-grip" aria-hidden="true" title="Drag to reorder">⋮⋮</span>
						{/if}
						<TrackRow
							{track}
							variant="numbered"
							{index}
							isCurrent={$currentTrack?.id === track.id}
							isPlaying={$isPlaying && $currentTrack?.id === track.id}
							selected={$selectedIds.has(track.id)}
							onSelect={editable ? (event) => handleSelect(event, index) : undefined}
							onRowClick={() => void playAll(track.id)}
							menuOptions={trackMenuOptions(index)}
						/>
					</li>
				{/each}
			</ol>
		{/if}
	{/if}
</div>

<style>
	.playlist-detail {
		width: min(100%, var(--content-width));
		margin: 0 auto;
	}

	/* ─── Hero ────────────────────────────────────────────────────────────── */

	.detail-hero {
		position: relative;
		isolation: isolate;
		margin-bottom: var(--space-4);
		padding: var(--space-5) var(--space-4);
		overflow: hidden;
		border-radius: var(--radius-lg);
	}

	/* Decorative only: blurred hard enough that the source image reads as colour,
	   with a gradient wash so the text keeps its contrast whatever the artwork is. */
	.detail-banner {
		position: absolute;
		inset: 0;
		z-index: -1;
		background-position: center;
		background-size: cover;
		filter: blur(40px) saturate(1.25);
		transform: scale(1.25);
		opacity: 0.5;
	}

	.detail-banner::after {
		content: '';
		position: absolute;
		inset: 0;
		background: linear-gradient(
			to right,
			var(--bg-base) 0%,
			color-mix(in srgb, var(--bg-base) 72%, transparent) 55%,
			color-mix(in srgb, var(--bg-base) 92%, transparent) 100%
		);
	}

	.detail-hero-inner {
		display: flex;
		align-items: flex-end;
		gap: var(--space-4);
	}

	.detail-cover {
		display: grid;
		flex: none;
		width: clamp(7rem, 14vw, 11rem);
		aspect-ratio: 1 / 1;
		place-items: center;
		overflow: hidden;
		border-radius: var(--radius-md);
		background: var(--bg-raised);
		box-shadow: 0 12px 32px rgba(0, 0, 0, 0.45);
	}

	.detail-cover.has-mosaic {
		grid-template-columns: 1fr 1fr;
		grid-template-rows: 1fr 1fr;
	}

	.detail-cover :global(.detail-cover-art) {
		width: 100%;
		height: 100%;
		object-fit: cover;
		display: block;
	}

	.detail-initial {
		color: var(--text-primary);
		font-size: var(--font-size-3xl);
		font-weight: var(--font-weight-semibold);
		line-height: 1;
	}

	.detail-headings {
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
		min-width: 0;
	}

	/* The title and the rename field occupy the same slot, so they share a height
	   and a left edge - otherwise opening rename nudged the whole hero. */
	.detail-title,
	.rename-form {
		display: flex;
		align-items: center;
		min-height: 3rem;
	}

	.detail-title {
		margin: 0;
		font-family: var(--font-display);
		font-size: var(--font-size-3xl);
		font-weight: var(--font-weight-semibold);
		line-height: var(--line-height-tight);
		overflow-wrap: anywhere;
	}

	.detail-meta {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 6px;
		margin: 0;
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
	}

	.detail-desc {
		color: var(--text-tertiary);
	}

	.detail-actions {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--space-2);
		margin-top: var(--space-1);
	}

	/* Rename happens in place of the title, so the field inherits its scale.
	   nowrap keeps Save and Cancel on the title's line: letting the input grow to
	   min(420px, 100%) pushed Cancel onto a second row on its own. */
	.rename-form {
		flex-wrap: nowrap;
		gap: var(--space-2);
	}

	.rename-form input {
		flex: 1 1 auto;
		min-width: 0;
		max-width: 28rem;
		/* Cancel out the field's own padding and border so the text starts on the
		   same x as the <h1> it replaced. */
		margin-left: calc(-1 * (var(--space-2) + 1px));
		padding: var(--space-1) var(--space-2);
		border: 1px solid var(--border-strong);
		border-radius: var(--radius-sm);
		background: var(--bg-elevated);
		color: var(--text-primary);
		font-family: var(--font-display);
		font-size: var(--font-size-2xl);
		font-weight: var(--font-weight-semibold);
		line-height: var(--line-height-tight);
	}

	.rename-form input:focus-visible {
		border-color: var(--accent-line);
		outline: 2px solid var(--accent);
		outline-offset: 1px;
	}

	.rename-form .btn {
		flex: none;
	}

	.sr-only {
		position: absolute;
		width: 1px;
		height: 1px;
		padding: 0;
		margin: -1px;
		overflow: hidden;
		clip-path: inset(50%);
		white-space: nowrap;
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

	.icon-btn {
		display: inline-flex;
		width: var(--control-h);
		height: var(--control-h);
		align-items: center;
		justify-content: center;
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-sm);
		background: var(--bg-surface);
		color: var(--text-secondary);
		cursor: pointer;
		transition:
			background var(--motion-fast),
			color var(--motion-fast),
			border-color var(--motion-fast);
	}

	.icon-btn:hover {
		border-color: var(--accent-line);
		background: var(--accent-soft);
		color: var(--text-primary);
	}

	.icon-btn.active {
		color: var(--state-favorite);
	}

	.detail-tracks {
		display: flex;
		flex-direction: column;
		margin: 0;
		padding: 0;
		list-style: none;
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

	@media (max-width: 760px) {
		.detail-hero {
			padding: var(--space-4) var(--space-3);
		}

		/* Side by side leaves the title about 200px on a phone. Stack instead. */
		.detail-hero-inner {
			flex-direction: column;
			align-items: flex-start;
		}

		.detail-title {
			font-size: var(--font-size-2xl);
		}

		/* No room for field + Save + Cancel on one line at phone widths; nowrap
		   ran Cancel past the edge where the hero's overflow clipped it. Give the
		   field its own line and let the buttons share the next one. */
		.rename-form {
			flex-wrap: wrap;
			align-items: flex-start;
		}

		.rename-form input {
			flex: 1 1 100%;
			max-width: 100%;
			font-size: var(--font-size-lg);
		}
	}
</style>
