<script lang="ts">
	import { onMount } from 'svelte';
	import { api, type Playlist } from '$lib/api/client';
	import { playlistPicker } from '$lib/stores/add_to_playlist';
	import { invalidatePlaylistCaches } from '$lib/cache/ws_events';
	import { showToast } from '$lib/stores/toast';
	import { portal } from '$lib/actions/portal';
	import Skeleton from '$lib/components/ui/Skeleton.svelte';

	let { title, getTrackIds }: { title: string; getTrackIds: () => Promise<number[]> } = $props();
	let playlists = $state<Playlist[]>([]);
	let loading = $state(true);
	let busy = $state(false);
	let error = $state<string | null>(null);
	let dialog: HTMLDialogElement;
	let disposed = false;
	let closing = false;

	onMount(() => {
		dialog.showModal();
		void load();
		return () => { disposed = true; };
	});
	function close() {
		if (disposed || closing) return;
		closing = true;
		if (dialog.open) dialog.close();
		playlistPicker.set(null);
	}
	function backdropClick(event: MouseEvent) {
		if (event.target !== dialog) return;
		const bounds = dialog.getBoundingClientRect();
		if (event.clientX < bounds.left || event.clientX > bounds.right
			|| event.clientY < bounds.top || event.clientY > bounds.bottom) close();
	}
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
			if (!disposed) close();
		} catch (reason) { error = `Could not add to ${playlist.name}: ${reason}`; }
		finally { busy = false; }
	}
</script>

<!-- Escape is handled by the native dialog's cancel event. -->
<!-- svelte-ignore a11y_click_events_have_key_events -->
<dialog bind:this={dialog} use:portal aria-label="Add to playlist" onclose={close}
	oncancel={(event) => { event.preventDefault(); close(); }} onclick={backdropClick}>
	<header>
		<div class="heading">
			<h2 class="t-section">Add to playlist</h2>
			<p class="subtitle t-meta" title={title}>{title}</p>
		</div>
		<button type="button" class="btn btn-icon" aria-label="Close playlist picker" onclick={close}>
			<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><path d="M18 6 6 18M6 6l12 12" /></svg>
		</button>
	</header>
	{#if error}<p class="error t-body" role="alert">{error}</p>{/if}
	{#if loading}<Skeleton rows={5} label="Loading playlists" />
	{:else if !playlists.length}
		<div class="empty">
			{#if error}<button class="btn btn-secondary" onclick={() => void load()}>Retry</button>
			{:else}<p class="t-body">No regular playlists yet.</p><a class="btn btn-secondary" href="/playlists" onclick={close}>Create a playlist</a>{/if}
		</div>
	{:else}
		<ul>{#each playlists as playlist (playlist.id)}
			<li><button class="playlist-choice" disabled={busy} onclick={() => void add(playlist)}>
				<span class="playlist-name t-row-title" title={playlist.name}>{playlist.name}</span><span class="subtitle t-meta">{playlist.tidal_uuid ? 'TIDAL' : 'Local'}</span>
			</button></li>
		{/each}</ul>
		{#if busy}<p class="status t-meta" role="status">Adding tracks…</p>{/if}
	{/if}
</dialog>

<style>
	dialog { position: fixed; inset: 0; margin: auto; width: min(440px, calc(100vw - var(--space-6))); max-height: min(80dvh, calc(100dvh - var(--space-6))); padding: 0; border: 1px solid var(--border-subtle); border-radius: var(--radius-md); background: color-mix(in srgb, var(--bg-elevated) 96%, transparent); color: var(--text-primary); box-shadow: var(--panel-shadow); overflow: hidden; }
	dialog[open] { display: flex; flex-direction: column; }
	dialog::backdrop { background: rgba(0, 0, 0, 0.55); backdrop-filter: var(--blur-modal); }
	header { display: flex; align-items: start; justify-content: space-between; gap: var(--space-3); padding: var(--space-5); flex-shrink: 0; }
	.heading { min-width: 0; }
	h2, .subtitle { margin: 0; }
	.heading .subtitle { margin-top: var(--space-2); }
	header button { flex-shrink: 0; }
	.error { color: var(--state-error); margin: 0; padding: 0 var(--space-5) var(--space-3); }
	.empty, .status { padding: 0 var(--space-5) var(--space-5); }
	ul { list-style: none; padding: 0 var(--space-3) var(--space-3); margin: 0; min-height: 0; overflow-y: auto; overscroll-behavior: contain; scrollbar-width: thin; }
	.playlist-choice { width: 100%; display: grid; grid-template-columns: minmax(0, 1fr) auto; align-items: center; gap: var(--space-3); padding: var(--space-3); border: 0; border-radius: var(--radius-sm); background: transparent; color: inherit; font: inherit; text-align: left; cursor: pointer; transition: background var(--motion-fast); }
	.playlist-name { min-width: 0; }
	.playlist-choice:hover { background: var(--bg-hover); }
</style>
