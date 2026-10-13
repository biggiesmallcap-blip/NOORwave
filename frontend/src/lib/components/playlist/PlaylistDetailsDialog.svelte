<script lang="ts">
	// Name and description of a regular playlist, for creating one (no
	// `playlist`) or editing one. Creating opens the new playlist; editing calls
	// `onSaved`. A TIDAL playlist is renamed on TIDAL first by the server.
	import { onMount, untrack } from 'svelte';
	import { goto } from '$app/navigation';
	import { api, ApiError, type Playlist } from '$lib/api/client';
	import { invalidatePlaylistCaches } from '$lib/cache/ws_events';
	import { portal } from '$lib/actions/portal';

	let { playlist = null, onClose, onSaved }: {
		playlist?: Playlist | null;
		onClose: () => void;
		onSaved?: (playlist: Playlist) => void;
	} = $props();

	// The dialog edits a snapshot: it opens on one playlist and closes after.
	const initial = untrack(() => playlist);
	const editing = initial !== null;
	let dialog: HTMLDialogElement;
	let name = $state(initial?.name ?? '');
	let description = $state(initial?.description ?? '');
	let saving = $state(false);
	let error = $state<string | null>(null);

	onMount(() => dialog.showModal());

	function close() {
		dialog.close();
	}

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		const trimmed = name.trim();
		if (!trimmed || saving) return;
		saving = true;
		error = null;
		try {
			const details = description.trim() || null;
			if (initial) {
				const result = await api.updatePlaylist(initial.id, trimmed, details);
				invalidatePlaylistCaches();
				onSaved?.(result.playlist);
				close();
			} else {
				const result = await api.createPlaylist(trimmed, details);
				invalidatePlaylistCaches();
				close();
				await goto(`/playlists/${result.playlist.id}`);
			}
		} catch (reason) {
			// The API's 4xx messages are written for people; anything else is not.
			error =
				reason instanceof ApiError && reason.status !== 500 && !/^API error/i.test(reason.message)
					? reason.message
					: `Couldn't ${editing ? 'save' : 'create'} the playlist. Try again.`;
		} finally {
			saving = false;
		}
	}
</script>

<!-- Escape closes through the native cancel event; a backdrop click targets the dialog itself. -->
<!-- svelte-ignore a11y_click_events_have_key_events -->
<dialog
	bind:this={dialog}
	use:portal
	aria-labelledby="playlist-details-title"
	onclose={onClose}
	onclick={(event) => { if (event.target === dialog) close(); }}
>
	<form onsubmit={submit}>
		<h2 id="playlist-details-title" class="t-section">{editing ? 'Edit details' : 'New playlist'}</h2>
		<label class="field">
			<span class="t-meta">Name</span>
			<!-- svelte-ignore a11y_autofocus -->
			<input bind:value={name} maxlength="200" autocomplete="off" autofocus required />
		</label>
		<label class="field">
			<span class="t-meta">Description (optional)</span>
			<textarea bind:value={description} maxlength="1000" rows="3"></textarea>
		</label>
		{#if error}<p class="error t-body" role="alert">{error}</p>{/if}
		<div class="actions">
			<button type="button" class="btn btn-ghost" onclick={close}>Cancel</button>
			<button type="submit" class="btn btn-primary" disabled={saving || !name.trim()}>
				{#if editing}{saving ? 'Saving...' : 'Save'}{:else}{saving ? 'Creating...' : 'Create'}{/if}
			</button>
		</div>
	</form>
</dialog>

<style>
	/* STYLING.md overlay treatment. The global reset drops the UA centring. */
	dialog {
		--dialog-width: 440px;
		position: fixed;
		inset: 0;
		margin: auto;
		width: min(var(--dialog-width), calc(100vw - var(--space-6)));
		padding: var(--space-5);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-md);
		background: color-mix(in srgb, var(--bg-elevated) 96%, transparent);
		color: var(--text-primary);
		box-shadow: var(--panel-shadow);
	}

	dialog[open] {
		animation: dialog-in var(--motion-base) both;
	}

	dialog::backdrop {
		background: rgba(0, 0, 0, 0.55);
		backdrop-filter: var(--blur-modal);
	}

	form {
		display: grid;
		gap: var(--space-4);
	}

	h2,
	p {
		margin: 0;
	}

	.field {
		display: grid;
		gap: var(--space-2);
	}

	/* Sunken fields (STYLING.md "Surfaces"). */
	input,
	textarea {
		padding: var(--space-2) var(--space-3);
		border: 0;
		border-radius: var(--radius-sm);
		background: var(--bg-surface);
		color: var(--text-primary);
		font: inherit;
	}

	input {
		height: var(--control-h);
	}

	textarea {
		resize: vertical;
	}

	input:focus-visible,
	textarea:focus-visible {
		outline: 2px solid var(--accent-strong);
		outline-offset: 2px;
	}

	.error {
		color: var(--state-error);
	}

	.actions {
		display: flex;
		justify-content: flex-end;
		gap: var(--space-2);
	}

	@keyframes dialog-in {
		from { opacity: 0; transform: translateY(12px); }
		to { opacity: 1; transform: none; }
	}
</style>
