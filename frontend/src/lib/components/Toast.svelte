<script lang="ts">
	import { fly } from 'svelte/transition';
	import { quintOut } from 'svelte/easing';
	import { toasts, dismissToast } from '$lib/stores/toast';
</script>

<div class="toast-stack" aria-live="polite" aria-atomic="false">
	{#each $toasts as toast (toast.id)}
		<button
			type="button"
			class="toast toast--{toast.kind}"
			onclick={() => dismissToast(toast.id)}
			in:fly={{ y: 12, duration: 180, easing: quintOut }}
			out:fly={{ y: 12, duration: 140, easing: quintOut }}
		>
			{toast.message}
		</button>
	{/each}
</div>

<style>
	.toast-stack {
		position: fixed;
		bottom: 96px; /* clear of the player bar */
		left: 50%;
		transform: translateX(-50%);
		display: flex;
		flex-direction: column;
		gap: 8px;
		z-index: 1200;
		pointer-events: none;
	}
	.toast {
		pointer-events: auto;
		background: var(--bg-elevated);
		color: var(--text-primary);
		border: 1px solid var(--border-strong);
		border-radius: 22px;
		padding: 9px 18px;
		font-size: 13px;
		font-weight: 500;
		box-shadow: 0 12px 32px -10px rgba(0, 0, 0, 0.55);
		cursor: pointer;
		font-family: inherit;
	}
	.toast--success { border-color: var(--state-success); }
	.toast--error {
		border-color: var(--state-error);
		color: var(--state-error);
	}
	.toast:hover { background: var(--bg-raised); }
</style>
