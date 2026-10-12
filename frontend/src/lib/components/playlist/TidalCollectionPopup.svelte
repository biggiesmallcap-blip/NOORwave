<script lang="ts">
	import { onMount } from 'svelte';
	import { api, type TidalPlayable, type Playlist } from '$lib/api/client';
	import DetailHero from '$lib/components/ui/DetailHero.svelte';
	import ActionBar from '$lib/components/ui/ActionBar.svelte';
	import QuickViewPopup from '$lib/components/ui/QuickViewPopup.svelte';
	import Skeleton from '$lib/components/ui/Skeleton.svelte';
	import TidalTrackRow from '$lib/components/TidalTrackRow.svelte';
	import { currentTrack, isPlaying, playTidalTracksNow, shuffleTidalTracksNow } from '$lib/stores/player';
	import { showToast } from '$lib/stores/toast';
	import { openContextMenu, openMenuAtElement } from '$lib/stores/context_menu';
	import { buildTidalCollectionMenu } from '$lib/player/tidal_collection_menu';
	import { saveTidalPlaylist } from '$lib/player/save_tidal_playlist';

	let { id, kind, title, subtitle = null, artworkUrl = null, onClose }: {
		id: string; kind: 'mix' | 'playlist'; title: string;
		subtitle?: string | null; artworkUrl?: string | null; onClose: () => void;
	} = $props();
	let tracks = $state<TidalPlayable[]>([]);
	let loading = $state(true);
	let error = $state<string | null>(null);
	let saving = $state(false);
	let saveError = $state<string | null>(null);
	let saved = $state<Playlist | null>(null);
	let name = $state('');
	let loadSeq = 0;

	onMount(() => {
		name = title;
		void load();
		return () => {
			loadSeq++;
		};
	});

	async function load() {
		const seq = ++loadSeq;
		loading = true;
		error = null;
		try {
			const result = kind === 'mix'
				? await api.getTidalMixTracks(id)
				: await api.getTidalPlaylistTracks(id, true);
			if (seq !== loadSeq) return;
			tracks = result.tracks.map((track) => ({ ...track, artist_name: track.artist_name ?? null }));
		} catch (reason) {
			if (seq === loadSeq) error = `Could not load tracks: ${reason}`;
		} finally {
			if (seq === loadSeq) loading = false;
		}
	}

	function play(startIndex = 0) {
		void playTidalTracksNow(tracks, title, { startIndex, shuffleMode: 'off' });
	}

	async function save() {
		if (saving || saved || !name.trim() || !tracks.length) return;
		saving = true;
		saveError = null;
		try {
			saved = await saveTidalPlaylist(name, tracks);
			showToast(`Saved ${saved.name} to NOORwave and TIDAL`, 'success');
		} catch (reason) {
			saveError = `Could not save playlist: ${reason}`;
		} finally {
			saving = false;
		}
	}

	function menu() {
		if (loading || !tracks.length) return [];
		return buildTidalCollectionMenu({
			onPlay: () => play(),
			onShuffle: () => void shuffleTidalTracksNow(tracks, title),
			onSave: !saving && !saved && name.trim() ? () => void save() : undefined,
		});
	}

	function contextMenu(event: MouseEvent) {
		event.preventDefault();
		openContextMenu(event, menu(), title);
	}

	function moreActions(event: MouseEvent) {
		event.preventDefault();
		event.stopPropagation();
		openMenuAtElement(event.currentTarget as HTMLElement, menu(), title);
	}
</script>

<QuickViewPopup {title} {onClose}>
	{#snippet children(close)}
		<div class="collection-hero">
			<DetailHero {title} artwork={artworkUrl} fallbackText="MIX" oncontextmenu={contextMenu}>
				{#snippet meta()}
					<span>TIDAL {kind === 'mix' ? 'mix' : 'playlist'}</span>
					{#if !loading && !error}<span>{tracks.length} tracks</span>{/if}
				{/snippet}
				{#snippet details()}
					{#if subtitle}<p class="t-meta subtitle">{subtitle}</p>{/if}
				{/snippet}
				{#snippet actions()}
					<ActionBar
						onplay={!loading && tracks.length ? () => play() : undefined}
						onshuffle={!loading && tracks.length ? () => void shuffleTidalTracksNow(tracks, title) : undefined}
						onmore={!loading && tracks.length ? moreActions : undefined}
						moreLabel={`More ${kind} actions`}>
						{#if saved}
							<a class="btn btn-ghost" href={`/playlists/${saved.id}`} onclick={close}>Open saved playlist</a>
						{:else}
							<button class="btn btn-secondary" disabled={loading || saving || !tracks.length}
								title="Save these tracks to NOORwave and TIDAL" onclick={() => void save()}>{saving ? 'Saving…' : 'Save playlist'}</button>
						{/if}
						<button class="btn btn-icon" aria-label="Refresh from TIDAL" title="Refresh from TIDAL"
							disabled={loading || saving} onclick={() => void load()}>
							<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><path d="M20 7v5h-5M4 17v-5h5"/><path d="M6 7a7 7 0 0 1 12-1l2 6M4 12l2 6a7 7 0 0 0 12-1"/></svg>
						</button>
					</ActionBar>
				{/snippet}
			</DetailHero>
		</div>
		{#if saveError}<p class="status save-error t-body" role="alert">{saveError}</p>{/if}
		{#if loading && !tracks.length}
			<Skeleton rows={6} label="Loading tracks" />
		{:else if error && !tracks.length}
			<div class="status"><p class="t-body" role="alert">{error}</p><button class="btn btn-secondary" onclick={() => void load()}>Retry</button></div>
		{:else if !tracks.length}
			<p class="status t-body">No tracks in this {kind}.</p>
		{:else}
			{#if error}
				<div class="status"><p class="t-body" role="alert">{error}. Showing the previous track list.</p><button class="btn btn-secondary" onclick={() => void load()}>Retry</button></div>
			{/if}
			<ol class="track-list" class:refreshing={loading} aria-busy={loading}>
				{#each tracks as track, index (`${index}:${track.tidal_id}`)}
					<TidalTrackRow {track} {index} variant="numbered" isCurrent={$currentTrack?.tidal_id === track.tidal_id}
						isPlaying={$isPlaying} onRowClick={() => play(index)} />
				{/each}
			</ol>
		{/if}
	{/snippet}
</QuickViewPopup>

<style>
	.collection-hero { padding: var(--space-5) var(--space-5) var(--space-3); padding-right: var(--space-7); }
	.collection-hero :global(.detail-hero) { margin: 0; }
	.subtitle { margin: 0; }
	.status { padding: var(--space-4) var(--space-5); margin: 0; }
	.save-error { color: var(--state-error); }
	.track-list { list-style: none; min-height: 0; overflow-y: auto; overscroll-behavior: contain; padding: var(--space-2) var(--space-3) var(--space-4); margin: 0; scrollbar-width: thin; }
	.track-list.refreshing { opacity: 0.55; }
</style>
