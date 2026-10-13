<script lang="ts">
	// The playlist picker opened by choosePlaylist(). Every way out (Escape, the
	// close button, a backdrop click, a finished add) goes through the native
	// dialog.close(), so the browser restores focus, and the close event clears
	// the store. The layout keys this component on the request, so it only
	// clears its own request: an add that finishes after the listener opened a
	// new picker must not dismiss that one.
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { api, type Playlist } from '$lib/api/client';
	import { playlistPicker } from '$lib/stores/add_to_playlist';
	import { invalidatePlaylistCaches } from '$lib/cache/ws_events';
	import { showToast } from '$lib/stores/toast';
	import { portal } from '$lib/actions/portal';
	import Skeleton from '$lib/components/ui/Skeleton.svelte';
	import ErrorState from '$lib/components/ui/ErrorState.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import { actionErrorMessage } from '$lib/utils/describe_error';

	let { title, getTrackIds }: { title: string; getTrackIds: () => Promise<number[]> } = $props();

	const request = get(playlistPicker);
	let dialog: HTMLDialogElement;
	let playlists = $state<Playlist[]>([]);
	let loading = $state(true);
	let loadError = $state<unknown>(null);
	let addError = $state<string | null>(null);
	let busy = $state(false);

	onMount(() => {
		dialog.showModal();
		void load();
	});

	function close() {
		dialog.close();
	}

	function clearRequest() {
		playlistPicker.update((current) => (current === request ? null : current));
	}

	async function load() {
		loading = true;
		loadError = null;
		try {
			playlists = (await api.getPlaylists()).playlists.filter((playlist) => !playlist.is_smart);
		} catch (reason) {
			loadError = reason;
		} finally {
			loading = false;
		}
	}

	async function add(playlist: Playlist) {
		if (busy) return;
		busy = true;
		addError = null;
		try {
			const { added } = await api.batchAddToPlaylist(playlist.id, await getTrackIds());
			invalidatePlaylistCaches();
			showToast(added ? `Added to ${playlist.name}` : `Already in ${playlist.name}`, added ? 'success' : 'info');
			close();
		} catch (reason) {
			console.error(`Could not add to playlist ${playlist.id}`, reason);
			addError = actionErrorMessage(reason, `Couldn't add to ${playlist.name}. Try again.`);
		} finally {
			busy = false;
		}
	}
</script>

<!-- Escape closes through the native cancel event; a backdrop click targets the dialog itself. -->
<!-- svelte-ignore a11y_click_events_have_key_events -->
<dialog
	bind:this={dialog}
	use:portal
	data-floating-layer
	aria-label="Add to playlist"
	onclose={clearRequest}
	onclick={(event) => { if (event.target === dialog) close(); }}
>
	<header>
		<div class="heading">
			<h2 class="t-section">Add to playlist</h2>
			<p class="t-meta" {title}>{title}</p>
		</div>
		<button type="button" class="btn btn-icon" aria-label="Close playlist picker" onclick={close}>
			<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><path d="M18 6 6 18M6 6l12 12" /></svg>
		</button>
	</header>

	<div class="body">
		{#if loading}
			<Skeleton rows={5} label="Loading playlists" />
		{:else if loadError}
			<ErrorState title="Couldn't load your playlists" error={loadError} onretry={() => void load()} />
		{:else if !playlists.length}
			<EmptyState title="No playlists yet" copy="Create a playlist, then add to it from here.">
				{#snippet actions()}
					<a class="btn btn-secondary" href="/playlists" onclick={close}>Create a playlist</a>
				{/snippet}
			</EmptyState>
		{:else}
			{#if addError}<p class="error t-body" role="alert">{addError}</p>{/if}
			<ul>
				{#each playlists as playlist (playlist.id)}
					<li>
						<button type="button" class="choice" disabled={busy} onclick={() => void add(playlist)}>
							<span class="t-row-title" title={playlist.name}>{playlist.name}</span>
							<span class="t-meta">{playlist.tidal_uuid ? 'TIDAL' : 'Local'}</span>
						</button>
					</li>
				{/each}
			</ul>
			{#if busy}<p class="t-meta" role="status">Adding tracks...</p>{/if}
		{/if}
	</div>
</dialog>

<style>
	/* STYLING.md overlay treatment. The global reset drops the UA centring, so
	   inset and auto margins put it back. */
	dialog {
		--picker-width: 440px;
		position: fixed;
		inset: 0;
		margin: auto;
		width: min(var(--picker-width), calc(100vw - var(--space-6)));
		max-height: min(80dvh, calc(100dvh - var(--space-6)));
		padding: 0;
		overflow: hidden;
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-md);
		background: color-mix(in srgb, var(--bg-elevated) 96%, transparent);
		color: var(--text-primary);
		box-shadow: var(--panel-shadow);
	}

	dialog[open] {
		display: flex;
		flex-direction: column;
		animation: picker-in var(--motion-base) both;
	}

	dialog::backdrop {
		background: rgba(0, 0, 0, 0.55);
		backdrop-filter: var(--blur-modal);
	}

	header {
		display: flex;
		align-items: start;
		justify-content: space-between;
		gap: var(--space-3);
		padding: var(--space-5);
	}

	.heading {
		display: grid;
		gap: var(--space-2);
		min-width: 0;
	}

	h2,
	p {
		margin: 0;
	}

	.body {
		min-height: 0;
		overflow-y: auto;
		overscroll-behavior: contain;
		scrollbar-width: thin;
		padding: 0 var(--space-5) var(--space-5);
	}

	.error {
		color: var(--state-error);
		padding-bottom: var(--space-3);
	}

	ul {
		list-style: none;
		display: grid;
		gap: var(--row-gap);
		margin: 0 calc(-1 * var(--space-3));
		padding: 0;
	}

	.choice {
		width: 100%;
		display: grid;
		grid-template-columns: minmax(0, 1fr) auto;
		align-items: center;
		gap: var(--space-3);
		padding: var(--space-3);
		border: 0;
		border-radius: var(--radius-sm);
		background: transparent;
		color: inherit;
		font: inherit;
		text-align: left;
		cursor: pointer;
		transition: background var(--motion-fast);
	}

	.choice:hover:not(:disabled) {
		background: var(--bg-hover);
	}

	@keyframes picker-in {
		from { opacity: 0; transform: translateY(12px); }
		to { opacity: 1; transform: none; }
	}
</style>
