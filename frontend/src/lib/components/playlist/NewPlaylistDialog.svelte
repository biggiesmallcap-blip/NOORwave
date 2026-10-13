<script lang="ts">
	// Create a regular playlist, the kind you fill by hand (as opposed to a
	// smart playlist built from rules). Opens the new playlist on success.
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { api, ApiError } from '$lib/api/client';
	import { invalidatePlaylistCaches } from '$lib/cache/ws_events';
	import { portal } from '$lib/actions/portal';

	let { onClose }: { onClose: () => void } = $props();

	let dialog: HTMLDialogElement;
	let name = $state('');
	let saving = $state(false);
	let error = $state<string | null>(null);

	onMount(() => dialog.showModal());

	function close() {
		dialog.close();
	}

	async function create(event: SubmitEvent) {
		event.preventDefault();
		const trimmed = name.trim();
		if (!trimmed || saving) return;
		saving = true;
		error = null;
		try {
			const { playlist } = await api.createPlaylist(trimmed);
			invalidatePlaylistCaches();
			close();
			await goto(`/playlists/${playlist.id}`);
		} catch (reason) {
			// The API's 4xx messages are written for people; anything else is not.
			error =
				reason instanceof ApiError && reason.status < 500
					? reason.message
					: "Couldn't create the playlist. Try again.";
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
	aria-labelledby="new-playlist-title"
	onclose={onClose}
	onclick={(event) => { if (event.target === dialog) close(); }}
>
	<form onsubmit={create}>
		<h2 id="new-playlist-title" class="t-section">New playlist</h2>
		<label class="field">
			<span class="t-meta">Name</span>
			<!-- svelte-ignore a11y_autofocus -->
			<input bind:value={name} maxlength="200" autocomplete="off" autofocus required />
		</label>
		{#if error}<p class="error t-body" role="alert">{error}</p>{/if}
		<div class="actions">
			<button type="button" class="btn btn-ghost" onclick={close}>Cancel</button>
			<button type="submit" class="btn btn-primary" disabled={saving || !name.trim()}>
				{saving ? 'Creating...' : 'Create'}
			</button>
		</div>
	</form>
</dialog>

<style>
	/* STYLING.md overlay treatment. The global reset drops the UA centring. */
	dialog {
		--dialog-width: 400px;
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

	/* Sunken field (STYLING.md "Surfaces"). */
	input {
		height: var(--control-h);
		padding: 0 var(--space-3);
		border: 0;
		border-radius: var(--radius-sm);
		background: var(--bg-surface);
		color: var(--text-primary);
		font: inherit;
	}

	input:focus-visible {
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
