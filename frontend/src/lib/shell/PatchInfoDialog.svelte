<!--
	Details for a pending desktop patch, opened from the sidebar version badge,
	the tray or Settings. The layout owns the update state and the install call.
-->
<script lang="ts">
	import type { DesktopUpdateInfo } from '$lib/desktop/update_state';

	interface Props {
		update: DesktopUpdateInfo;
		/** An install is starting; the dialog cannot be dismissed meanwhile. */
		busy: boolean;
		oninstall: () => void;
		onclose: () => void;
	}

	let { update, busy, oninstall, onclose }: Props = $props();
	let installButton = $state<HTMLButtonElement | null>(null);

	$effect(() => {
		installButton?.focus();
	});

	function handleKeydown(event: KeyboardEvent) {
		if (event.key === 'Escape') onclose();
	}
</script>

<svelte:window onkeydown={handleKeydown} />

<div class="patch-info-backdrop">
	<button
		type="button"
		class="patch-info-dismiss"
		aria-label="Close patch information"
		onclick={onclose}
	></button>
	<div
		class="patch-info-dialog glass-panel"
		role="dialog"
		aria-modal="true"
		aria-labelledby="patch-info-title"
		aria-describedby="patch-info-summary"
	>
		<header class="patch-info-header">
			<div class="patch-info-heading">
				<span class="patch-info-icon" aria-hidden="true">
					<svg viewBox="0 0 24 24" focusable="false">
						<path d="M12 3v12m0 0 5-5m-5 5-5-5M5 20h14" />
					</svg>
				</span>
				<div>
					<span class="patch-info-eyebrow">Patch available</span>
					<h2 id="patch-info-title">NOORwave v{update.version}</h2>
				</div>
			</div>
			<button
				type="button"
				class="patch-info-close"
				aria-label="Close patch information"
				disabled={busy}
				onclick={onclose}
			>
				<svg viewBox="0 0 24 24" aria-hidden="true" focusable="false">
					<path d="m6 6 12 12M18 6 6 18" />
				</svg>
			</button>
		</header>

		<p id="patch-info-summary" class="patch-info-summary">
			The engineers insist this version is better. Update?
		</p>

		<section class="patch-notes" aria-labelledby="patch-notes-title">
			<h3 id="patch-notes-title">What changed</h3>
			{#if update.notes}
				<div class="patch-notes-copy">{update.notes}</div>
			{:else}
				<p class="patch-notes-empty">Release notes were not included with this patch.</p>
			{/if}
		</section>

		<footer class="patch-info-actions">
			<button type="button" class="btn btn-glass" disabled={busy} onclick={onclose}>
				I Know Better
			</button>
			<button
				type="button"
				class="btn btn-primary patch-install-button"
				disabled={busy}
				bind:this={installButton}
				onclick={oninstall}
			>
				{busy ? 'Starting…' : 'Trust the Engineers'}
			</button>
		</footer>
	</div>
</div>

<style>
	.patch-info-backdrop {
		position: fixed;
		inset: 0;
		z-index: var(--z-modal, 80);
		display: grid;
		place-items: center;
		padding: 20px;
		background: rgba(0, 0, 0, 0.62);
		backdrop-filter: blur(10px);
	}

	.patch-info-dismiss {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
		border: 0;
		background: transparent;
		cursor: default;
	}

	.patch-info-dialog {
		position: relative;
		width: min(100%, 560px);
		max-height: min(720px, calc(100svh - 40px));
		display: flex;
		flex-direction: column;
		overflow: hidden;
		border-radius: 8px;
		box-shadow: 0 24px 70px rgba(0, 0, 0, 0.48);
	}

	.patch-info-header {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: 16px;
		padding: 24px 24px 14px;
	}

	.patch-info-heading {
		display: flex;
		align-items: center;
		gap: 12px;
		min-width: 0;
	}

	.patch-info-icon {
		width: 38px;
		height: 38px;
		flex: 0 0 38px;
		display: grid;
		place-items: center;
		border: 1px solid color-mix(in srgb, var(--state-warning, #ffcc66) 58%, transparent);
		border-radius: 50%;
		background: color-mix(in srgb, var(--state-warning, #ffcc66) 18%, transparent);
		color: color-mix(in srgb, var(--state-warning, #ffcc66) 84%, white);
	}

	.patch-info-icon svg,
	.patch-info-close svg {
		width: 20px;
		height: 20px;
		fill: none;
		stroke: currentColor;
		stroke-width: 1.8;
		stroke-linecap: round;
		stroke-linejoin: round;
	}

	.patch-info-eyebrow {
		display: block;
		margin-bottom: 2px;
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
	}

	.patch-info-dialog h2 {
		margin: 0;
		font-size: var(--font-size-xl);
		letter-spacing: 0;
	}

	.patch-info-close {
		width: 34px;
		height: 34px;
		flex: 0 0 34px;
		display: grid;
		place-items: center;
		border: 0;
		border-radius: 6px;
		background: transparent;
		color: var(--text-tertiary);
		cursor: pointer;
	}

	.patch-info-close:hover,
	.patch-info-close:focus-visible {
		background: var(--bg-hover);
		color: var(--text-primary);
		outline: none;
	}

	.patch-info-summary {
		margin: 0;
		padding: 0 24px 20px;
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
		line-height: var(--line-height-normal);
	}

	.patch-notes {
		min-height: 120px;
		overflow-y: auto;
		padding: 18px 24px;
		border-block: 1px solid var(--border-subtle);
		background: color-mix(in srgb, var(--instrument-surface) 45%, transparent);
	}

	.patch-notes h3 {
		margin: 0 0 10px;
		font-size: var(--font-size-sm);
		letter-spacing: 0;
	}

	.patch-notes-copy,
	.patch-notes-empty {
		margin: 0;
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
		line-height: var(--line-height-normal);
		white-space: pre-wrap;
		overflow-wrap: anywhere;
	}

	.patch-info-actions {
		display: flex;
		justify-content: flex-end;
		gap: 10px;
		padding: 18px 24px 22px;
	}

	.patch-install-button {
		min-width: 176px;
	}

	@media (max-width: 560px) {
		.patch-info-backdrop {
			padding: 12px;
		}

		.patch-info-dialog {
			max-height: calc(100svh - 24px);
		}

		.patch-info-header {
			padding: 20px 18px 12px;
		}

		.patch-info-summary,
		.patch-notes {
			padding-inline: 18px;
		}

		.patch-info-actions {
			padding: 16px 18px 18px;
		}

		.patch-install-button {
			min-width: 0;
		}
	}

	@media (max-width: 420px) {
		.patch-info-actions {
			flex-direction: column;
		}

		.patch-info-actions .btn {
			width: 100%;
		}
	}
</style>
