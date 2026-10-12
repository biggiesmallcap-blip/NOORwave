<script lang="ts">
	import { onMount } from 'svelte';
	import { api, type TidalPlayable, type Playlist } from '$lib/api/client';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import TidalTrackRow from '$lib/components/TidalTrackRow.svelte';
	import { portal } from '$lib/actions/portal';
	import { currentTrack, isPlaying, playTidalTracksNow, shuffleTidalTracksNow } from '$lib/stores/player';
	import { showToast } from '$lib/stores/toast';
	import { contextMenu as activeMenu, closeContextMenu, openContextMenu, openMenuAtElement } from '$lib/stores/context_menu';
	import { buildTidalCollectionMenu } from '$lib/player/tidal_collection_menu';
	import { saveTidalPlaylist } from '$lib/player/save_tidal_playlist';
	import { playlistPicker } from '$lib/stores/add_to_playlist';

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
	let panel: HTMLDivElement;
	let loadSeq = 0;

	onMount(() => {
		name = title;
		const previousFocus = document.activeElement;
		panel.focus();
		void load();
		return () => {
			loadSeq++;
			if (previousFocus instanceof HTMLElement && previousFocus.isConnected) previousFocus.focus();
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

	function backdropClick(event: MouseEvent) {
		if (event.target === event.currentTarget) onClose();
	}

	function handleKey(event: KeyboardEvent) {
		if ($playlistPicker) return;
		if (event.key === 'Escape') {
			if ($activeMenu.open) { closeContextMenu(); return; }
			onClose();
		}
		if (event.key !== 'Tab' || $activeMenu.open) return;
		const focusable = [...panel.querySelectorAll<HTMLElement>('button:not(:disabled), a[href], input:not(:disabled), [tabindex="0"]')];
		const first = focusable[0];
		const last = focusable.at(-1);
		if (event.shiftKey && (document.activeElement === first || document.activeElement === panel)) { event.preventDefault(); last?.focus(); }
		else if (!event.shiftKey && (document.activeElement === last || document.activeElement === panel)) { event.preventDefault(); first?.focus(); }
	}
</script>

<svelte:window onkeydown={handleKey} />
<!-- svelte-ignore a11y_click_events_have_key_events -->
<div use:portal class="popup-backdrop" role="presentation" onclick={backdropClick}>
<div bind:this={panel} class="collection-popup" role="dialog" aria-modal="true" tabindex="-1" aria-label={title}>
	<button class="close-btn icon-btn" aria-label="Close track list" onclick={onClose}>×</button>
	<!-- svelte-ignore a11y_no_static_element_interactions -->
	<header oncontextmenu={contextMenu}>
		<div class="cover"><ArtworkImage src={artworkUrl} alt={title} size={640} fallbackText="MIX" className="cover-art" /></div>
		<div class="info">
			<p class="eyebrow">TIDAL {kind === 'mix' ? 'mix' : 'playlist'}</p>
			<h2>{title}</h2>
			{#if subtitle}<p class="subtitle">{subtitle}</p>{/if}
			{#if !loading && !error}<p class="subtitle">{tracks.length} tracks</p>{/if}
			<div class="actions">
				<button class="btn btn-primary" disabled={loading || !tracks.length} onclick={() => play()}>▶ Play</button>
				<button class="btn btn-glass" disabled={loading || !tracks.length} onclick={() => void shuffleTidalTracksNow(tracks, title)}>Shuffle</button>
				<button class="btn btn-glass" disabled={loading || saving} onclick={() => void load()}>Refresh from TIDAL</button>
				<button class="icon-btn" aria-label="More mix actions" disabled={loading || !tracks.length}
					onclick={(event) => openMenuAtElement(event.currentTarget as HTMLElement, menu(), title)}>⋯</button>
			</div>
		</div>
	</header>
	{#if loading}
		<p class="status" role="status">Loading tracks…</p>
	{:else if error && !tracks.length}
		<div class="status"><p role="alert">{error}</p><button class="btn btn-glass" onclick={() => void load()}>Retry</button></div>
	{:else if !tracks.length}
		<p class="status">No tracks in this {kind}.</p>
	{:else}
		{#if error}
			<div class="status"><p role="alert">{error}. Showing the previous track list.</p><button class="btn btn-glass" onclick={() => void load()}>Retry</button></div>
		{/if}
		<div class="save-row">
			{#if saved}
				<p role="status">Saved to NOORwave and TIDAL.</p>
				<a class="btn btn-glass" href={`/playlists/${saved.id}`} onclick={onClose}>Open saved playlist</a>
			{:else}
				<label>Playlist name<input class="input" bind:value={name} disabled={saving} /></label>
				<button class="btn btn-glass" disabled={saving || !name.trim()} onclick={() => void save()}>{saving ? 'Saving…' : 'Save playlist'}</button>
				<p class="save-copy">Save these tracks to NOORwave and TIDAL.</p>
			{/if}
			{#if saveError}<p class="save-error" role="alert">{saveError}</p>{/if}
		</div>
		<ol class="track-list">
			{#each tracks as track, index (`${index}:${track.tidal_id}`)}
				<TidalTrackRow {track} {index} variant="numbered" isCurrent={$currentTrack?.tidal_id === track.tidal_id}
					isPlaying={$isPlaying} onRowClick={() => play(index)} />
			{/each}
		</ol>
	{/if}
</div>
</div>

<style>
	.popup-backdrop { position: fixed; inset: 0; z-index: var(--z-modal); display: grid; place-items: center; background: rgba(0, 0, 0, 0.55); backdrop-filter: blur(4px); }
	.collection-popup { position: relative; width: min(960px, calc(100vw - var(--space-6))); max-height: 85dvh; padding: 0; border: 1px solid var(--panel-border); border-radius: var(--radius-lg); background: var(--bg-base); color: var(--text-primary); overflow: auto; }
	.close-btn { position: absolute; right: var(--space-3); top: var(--space-3); }
	header { display: flex; gap: var(--space-6); padding: var(--space-6); padding-right: var(--space-7); background: var(--bg-raised); }
	.cover { width: clamp(100px, 18vw, 180px); flex-shrink: 0; aspect-ratio: 1; border-radius: var(--radius-md); overflow: hidden; }
	.cover :global(.cover-art) { width: 100%; height: 100%; object-fit: cover; }
	.info { min-width: 0; }
	h2 { font-family: var(--font-display); font-size: var(--font-size-2xl); font-weight: var(--font-weight-semibold); line-height: var(--line-height-tight); margin: var(--space-2) 0; overflow-wrap: anywhere; }
	.eyebrow { font-size: var(--font-size-xs); color: var(--text-muted); text-transform: uppercase; }
	.subtitle, .save-copy { color: var(--text-secondary); font-size: var(--font-size-sm); }
	.actions { display: flex; flex-wrap: wrap; gap: var(--space-2); margin-top: var(--space-4); }
	.status { padding: var(--space-6); }
	.save-row { display: flex; flex-wrap: wrap; align-items: end; gap: var(--space-3); padding: var(--space-4) var(--space-6); border-bottom: 1px solid var(--panel-border); }
	.save-row label { display: grid; gap: var(--space-2); flex: 1; font-size: var(--font-size-sm); }
	.save-copy, .save-error { flex-basis: 100%; margin: 0; }
	.save-error { color: var(--state-error); }
	.track-list { list-style: none; padding: var(--space-3); margin: 0; }
	@media (max-width: 600px) { header { gap: var(--space-3); padding: var(--space-6); padding-right: var(--space-7); } h2 { font-size: var(--font-size-xl); } .save-row { padding: var(--space-4); } }
</style>
