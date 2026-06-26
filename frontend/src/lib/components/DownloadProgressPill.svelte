<script lang="ts">
	import { fly } from 'svelte/transition';
	import { quintOut } from 'svelte/easing';
	import { downloadProgress, cancelDownloads } from '$lib/stores/downloads';
</script>

{#if $downloadProgress}
	{@const p = $downloadProgress}
	<div
		class="dl-pill"
		role="status"
		aria-live="polite"
		in:fly={{ y: 12, duration: 180, easing: quintOut }}
		out:fly={{ y: 12, duration: 140, easing: quintOut }}
	>
		<span class="dl-pill-spinner" aria-hidden="true"></span>
		<div class="dl-pill-info">
			<div class="dl-pill-line">
				<span class="dl-pill-title">
					{p.currentTitle ? `Downloading ${p.currentTitle}` : 'Downloading'}
				</span>
				{#if p.total > 1}
					<span class="dl-pill-count">{p.done}/{p.total}</span>
				{/if}
			</div>
			{#if p.total > 1}
				<div class="dl-pill-bar">
					<div class="dl-pill-bar-fill" style:width="{(p.done / p.total) * 100}%"></div>
				</div>
			{/if}
		</div>
		<button type="button" class="dl-pill-cancel" onclick={() => void cancelDownloads()}>
			Cancel
		</button>
	</div>
{/if}

<style>
	.dl-pill {
		position: fixed;
		bottom: 148px; /* above the toast stack (96px) so the two never overlap */
		left: 50%;
		transform: translateX(-50%);
		z-index: var(--z-toast);
		display: flex;
		align-items: center;
		gap: 12px;
		min-width: 280px;
		max-width: 440px;
		padding: 10px 12px 10px 16px;
		background: var(--bg-elevated);
		border: 1px solid var(--accent-line);
		border-radius: 22px;
		box-shadow: 0 12px 32px -10px rgba(0, 0, 0, 0.55);
	}
	.dl-pill-spinner {
		flex: 0 0 auto;
		width: 16px;
		height: 16px;
		border-radius: 50%;
		border: 2px solid color-mix(in srgb, var(--accent) 35%, transparent);
		border-top-color: var(--accent);
		animation: dl-spin 700ms linear infinite;
	}
	.dl-pill-info {
		flex: 1 1 auto;
		min-width: 0;
		display: grid;
		gap: 6px;
	}
	.dl-pill-line {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: 10px;
	}
	.dl-pill-title {
		color: var(--text-primary);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-medium);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.dl-pill-count {
		flex: 0 0 auto;
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
	}
	.dl-pill-bar {
		height: 4px;
		border-radius: 2px;
		background: color-mix(in srgb, var(--text-secondary) 20%, transparent);
		overflow: hidden;
	}
	.dl-pill-bar-fill {
		height: 100%;
		background: var(--accent);
		border-radius: 2px;
		transition: width 220ms ease;
	}
	.dl-pill-cancel {
		flex: 0 0 auto;
		background: none;
		border: 1px solid var(--border-strong);
		border-radius: 14px;
		padding: 4px 12px;
		color: var(--text-secondary);
		font-family: inherit;
		font-size: var(--font-size-sm);
		cursor: pointer;
	}
	.dl-pill-cancel:hover {
		color: var(--state-error);
		border-color: var(--state-error);
	}
	@keyframes dl-spin {
		to {
			transform: rotate(360deg);
		}
	}
</style>
