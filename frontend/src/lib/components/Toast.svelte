<script lang="ts">
	import { fly } from 'svelte/transition';
	import { quintOut } from 'svelte/easing';
	import { toasts, dismissToast } from '$lib/stores/toast';

	function runAction(toastId: number, onClick: () => void) {
		onClick();
		dismissToast(toastId);
	}
</script>

<div class="toast-stack" aria-live="polite" aria-atomic="false">
	{#each $toasts as toast (toast.id)}
		<div
			class="toast toast--{toast.kind}"
			in:fly={{ y: 12, duration: 180, easing: quintOut }}
			out:fly={{ y: 12, duration: 140, easing: quintOut }}
		>
			<button type="button" class="toast-body" onclick={() => dismissToast(toast.id)}>
				{toast.message}
			</button>
			{#if toast.actions && toast.actions.length}
				<div class="toast-actions">
					{#each toast.actions as action (action.label)}
						<button
							type="button"
							class="toast-action"
							onclick={() => runAction(toast.id, action.onClick)}
						>
							{action.label}
						</button>
					{/each}
				</div>
			{/if}
		</div>
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
		z-index: var(--z-toast);
		pointer-events: none;
	}
	.toast {
		pointer-events: auto;
		display: flex;
		align-items: center;
		gap: 10px;
		background: var(--bg-elevated);
		color: var(--text-primary);
		border: 1px solid var(--border-strong);
		border-radius: 22px;
		padding: 7px 10px 7px 18px;
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-medium);
		box-shadow: 0 12px 32px -10px rgba(0, 0, 0, 0.55);
		font-family: inherit;
	}
	.toast-body {
		background: none;
		border: none;
		color: inherit;
		font: inherit;
		cursor: pointer;
		padding: 2px 0;
	}
	.toast-actions {
		display: flex;
		gap: 6px;
	}
	.toast-action {
		background: var(--accent-soft);
		color: var(--accent-strong);
		border: 1px solid var(--accent-line);
		border-radius: 14px;
		padding: 4px 12px;
		font: inherit;
		font-weight: var(--font-weight-medium);
		cursor: pointer;
		white-space: nowrap;
	}
	.toast-action:hover {
		background: var(--accent);
		color: #fff;
	}
	.toast--success { border-color: var(--state-success); }
	.toast--error {
		border-color: var(--state-error);
		color: var(--state-error);
	}
	.toast:hover { background: var(--bg-raised); }
</style>
