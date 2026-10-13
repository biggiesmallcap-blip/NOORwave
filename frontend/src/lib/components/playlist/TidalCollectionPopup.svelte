<script lang="ts">
	// Quick view of a TIDAL mix or playlist that has no local copy yet. Shares
	// the album quick view's container; the hero is the detail-page DetailHero
	// and ActionBar, and Save playlist keeps the collection's title.
	import { onMount } from 'svelte';
	import { api, type TidalPlayable, type Playlist } from '$lib/api/client';
	import DetailHero from '$lib/components/ui/DetailHero.svelte';
	import ActionBar from '$lib/components/ui/ActionBar.svelte';
	import QuickViewPopup from '$lib/components/ui/QuickViewPopup.svelte';
	import Skeleton from '$lib/components/ui/Skeleton.svelte';
	import ErrorState from '$lib/components/ui/ErrorState.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import TidalTrackRow from '$lib/components/TidalTrackRow.svelte';
	import { currentTrack, isPlaying, playTidalTracksNow, shuffleTidalTracksNow, togglePlayback } from '$lib/stores/player';
	import { showToast } from '$lib/stores/toast';
	import { openContextMenu, openMenuAtElement } from '$lib/stores/context_menu';
	import { buildTidalCollectionMenu } from '$lib/player/tidal_collection_menu';
	import { saveTidalPlaylist } from '$lib/player/save_tidal_playlist';
	import { actionErrorMessage } from '$lib/utils/describe_error';

	let { id, kind, title, subtitle = null, artworkUrl = null, onClose }: {
		id: string; kind: 'mix' | 'playlist'; title: string;
		subtitle?: string | null; artworkUrl?: string | null; onClose: () => void;
	} = $props();

	let tracks = $state<TidalPlayable[]>([]);
	let loading = $state(true);
	let loadError = $state<unknown>(null);
	let saving = $state(false);
	let saved = $state<Playlist | null>(null);
	let loadSeq = 0;

	let hasTracks = $derived(tracks.length > 0);
	let canSave = $derived(hasTracks && !loading && !saving && !saved && title.trim() !== '');
	let playingFromHere = $derived(
		$currentTrack?.tidal_id != null && tracks.some((track) => track.tidal_id === $currentTrack?.tidal_id),
	);
	let collectionPlaying = $derived($isPlaying && playingFromHere);

	onMount(() => {
		void load();
		return () => { loadSeq++; };
	});

	async function load() {
		const seq = ++loadSeq;
		loading = true;
		loadError = null;
		try {
			const result = kind === 'mix'
				? await api.getTidalMixTracks(id)
				: await api.getTidalPlaylistTracks(id, true);
			if (seq !== loadSeq) return;
			tracks = result.tracks.map((track) => ({ ...track, artist_name: track.artist_name ?? null }));
		} catch (reason) {
			if (seq === loadSeq) loadError = reason;
		} finally {
			if (seq === loadSeq) loading = false;
		}
	}

	function play(startIndex = 0) {
		void playTidalTracksNow(tracks, title, { startIndex, shuffleMode: 'off' });
	}

	// Play pauses and resumes once this collection is what is playing, like the
	// detail pages' ActionBar.
	function playOrToggle() {
		if (playingFromHere) void togglePlayback();
		else play();
	}

	function shuffle() {
		void shuffleTidalTracksNow(tracks, title);
	}

	async function save() {
		if (!canSave) return;
		saving = true;
		try {
			saved = await saveTidalPlaylist(title, tracks);
			showToast(`Saved ${saved.name} to NOORwave and TIDAL`, 'success');
		} catch (reason) {
			console.error(`Could not save TIDAL ${kind} ${id}`, reason);
			showToast(actionErrorMessage(reason, `Couldn't save ${title}. Try again.`), 'error');
		} finally {
			saving = false;
		}
	}

	function menu() {
		if (!hasTracks) return [];
		return buildTidalCollectionMenu({
			onPlay: () => play(),
			onShuffle: shuffle,
			onSave: canSave ? () => void save() : undefined,
		});
	}

	function openHeroMenu(event: MouseEvent) {
		event.preventDefault();
		openContextMenu(event, menu(), title);
	}

	// Stop the opening click before it reaches ContextMenu's window listener,
	// which would close the menu again on the same click.
	function openMoreMenu(event: MouseEvent) {
		event.preventDefault();
		event.stopPropagation();
		openMenuAtElement(event.currentTarget as HTMLElement, menu(), title);
	}
</script>

<QuickViewPopup {title} {onClose} busy={loading && hasTracks}>
	{#snippet hero(requestClose)}
		<DetailHero {title} eyebrow={`TIDAL ${kind}`} artwork={artworkUrl} fallbackText="MIX" oncontextmenu={openHeroMenu}>
			{#snippet meta()}
				{#if hasTracks}<span>{tracks.length} tracks</span>{/if}
			{/snippet}
			{#snippet details()}
				{#if subtitle}<p class="t-meta">{subtitle}</p>{/if}
			{/snippet}
			{#snippet actions()}
				<ActionBar
					disabled={!hasTracks}
					playing={collectionPlaying}
					onplay={playOrToggle}
					onshuffle={shuffle}
					shuffleHint={`Play this ${kind} in random order`}
					onmore={openMoreMenu}
					moreLabel={`More ${kind} actions`}
				>
					{#if saved}
						<a class="btn btn-ghost" href={`/playlists/${saved.id}`} onclick={requestClose}>Open saved playlist</a>
					{:else}
						<button type="button" class="btn btn-secondary" disabled={!canSave}
							title="Save these tracks to NOORwave and TIDAL" onclick={() => void save()}>
							{saving ? 'Saving...' : 'Save playlist'}
						</button>
					{/if}
					<button type="button" class="btn btn-icon" aria-label="Refresh from TIDAL" title="Refresh from TIDAL"
						disabled={loading || saving} onclick={() => void load()}>
						<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><path d="M20 7v5h-5M4 17v-5h5"/><path d="M6 7a7 7 0 0 1 12-1l2 6M4 12l2 6a7 7 0 0 0 12-1"/></svg>
					</button>
				</ActionBar>
			{/snippet}
		</DetailHero>
	{/snippet}

	{#if loadError}
		<ErrorState
			title={hasTracks ? `Couldn't refresh this ${kind}` : `Couldn't load this ${kind}`}
			error={loadError}
			onretry={() => void load()}
		/>
	{/if}
	{#if hasTracks}
		<ol class="track-list" class:refreshing={loading} aria-busy={loading}>
			{#each tracks as track, index (`${index}:${track.tidal_id}`)}
				<TidalTrackRow {track} {index} variant="numbered" isCurrent={$currentTrack?.tidal_id === track.tidal_id}
					isPlaying={$isPlaying} onRowClick={() => play(index)} />
			{/each}
		</ol>
	{:else if loading}
		<Skeleton rows={6} label="Loading tracks" />
	{:else if !loadError}
		<EmptyState title={`No tracks in this ${kind}`} copy="TIDAL returned an empty list. Refresh to try again." />
	{/if}
</QuickViewPopup>

<style>
	p {
		margin: 0;
	}

	.track-list {
		list-style: none;
		margin: 0 calc(-1 * var(--space-2));
		padding: 0;
		transition: opacity var(--motion-fast);
	}

	.track-list.refreshing {
		opacity: 0.55;
	}
</style>
