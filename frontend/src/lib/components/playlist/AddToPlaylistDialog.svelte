<script lang="ts">
	import { onMount } from 'svelte';
	import { api, type Playlist } from '$lib/api/client';
	import { playlistPicker } from '$lib/stores/add_to_playlist';
	import { invalidatePlaylistCaches } from '$lib/cache/ws_events';
	import { showToast } from '$lib/stores/toast';
	import { portal } from '$lib/actions/portal';

	let { title, getTrackIds }: { title: string; getTrackIds: () => Promise<number[]> } = $props();
	let playlists = $state<Playlist[]>([]);
	let loading = $state(true);
	let busy = $state(false);
	let error = $state<string | null>(null);
	let dialog: HTMLDialogElement;

	onMount(() => {
		dialog.showModal();
		void load();
	});
	function close() { playlistPicker.set(null); }
	async function load() {
		loading = true;
		error = null;
		try { playlists = (await api.getPlaylists()).playlists.filter((playlist) => !playlist.is_smart); }
		catch (reason) { error = `Could not load playlists: ${reason}`; }
		finally { loading = false; }
	}
	async function add(playlist: Playlist) {
		if (busy) return;
		busy = true;
		error = null;
		try {
			const { added } = await api.batchAddToPlaylist(playlist.id, await getTrackIds());
			invalidatePlaylistCaches();
			showToast(added ? `Added to ${playlist.name}` : `Already in ${playlist.name}`, added ? 'success' : 'info');
			close();
		} catch (reason) { error = `Could not add to ${playlist.name}: ${reason}`; }
		finally { busy = false; }
	}
</script>

<dialog bind:this={dialog} use:portal aria-label="Add to playlist" onclose={close} oncancel={close}>
	<button class="icon-btn close-btn" aria-label="Close playlist picker" onclick={close}>×</button>
	<h2>Add to playlist</h2>
	<p class="subtitle">{title}</p>
	{#if error}<p class="error" role="alert">{error}</p>{/if}
	{#if loading}<p role="status">Loading playlists…</p>
	{:else if !playlists.length}
		{#if error}<button class="btn btn-glass" onclick={() => void load()}>Retry</button>
		{:else}<p>No regular playlists yet.</p><a class="btn btn-glass" href="/playlists" onclick={close}>Create a playlist</a>{/if}
	{:else}
		<ul>{#each playlists as playlist (playlist.id)}
			<li><button class="playlist-choice" disabled={busy} onclick={() => void add(playlist)}>
				<span>{playlist.name}</span><span class="subtitle">{playlist.tidal_uuid ? 'TIDAL' : 'Local'}</span>
			</button></li>
		{/each}</ul>
		{#if busy}<p role="status">Adding tracks…</p>{/if}
	{/if}
</dialog>

<style>
	dialog { position: fixed; width: min(440px, calc(100vw - var(--space-6))); max-height: 80dvh; padding: var(--space-6); border: 1px solid var(--panel-border); border-radius: var(--radius-lg); background: var(--bg-base); color: var(--text-primary); overflow: auto; }
	dialog::backdrop { background: rgba(0, 0, 0, 0.55); backdrop-filter: blur(4px); }
	.close-btn { position: absolute; right: var(--space-3); top: var(--space-3); }
	h2 { font-family: var(--font-display); font-size: var(--font-size-xl); font-weight: var(--font-weight-semibold); }
	.subtitle { color: var(--text-secondary); font-size: var(--font-size-sm); }
	.error { color: var(--state-error); }
	ul { list-style: none; padding: 0; }
	.playlist-choice { width: 100%; display: flex; justify-content: space-between; gap: var(--space-3); padding: var(--space-3); border: 0; border-radius: var(--radius-sm); background: transparent; color: inherit; font: inherit; text-align: left; cursor: pointer; }
	.playlist-choice:hover { background: var(--bg-hover); }
</style>
