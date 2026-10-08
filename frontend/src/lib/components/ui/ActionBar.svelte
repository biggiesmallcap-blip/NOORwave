<script lang="ts">
	// The action row of a detail hero (STYLING.md "ActionBar"). Play is the
	// labelled primary, Shuffle and Radio are labelled secondaries, Like and
	// More are icons. Leave a handler out to drop its button. Below 1100px of
	// DetailHero width the labels are visually hidden and the title tooltip
	// carries them. Extra page actions go in children, after More.
	import type { Snippet } from 'svelte';

	let {
		playing = false,
		onplay,
		playLabel = 'Play',
		onshuffle,
		shuffleHint = 'Shuffle',
		onradio,
		radioHint,
		radioLabel = 'Radio',
		radioPending = false,
		liked = false,
		onlike,
		likeLabel = 'Like',
		unlikeLabel = 'Unlike',
		likePending = false,
		onmore,
		moreLabel = 'More options',
		children,
	}: {
		playing?: boolean;
		onplay?: () => void;
		playLabel?: string;
		onshuffle?: () => void;
		/** Tooltip for Shuffle; explains what it shuffles. */
		shuffleHint?: string;
		onradio?: () => void;
		/** Tooltip for Radio; defaults to the radio label. */
		radioHint?: string;
		radioLabel?: string;
		radioPending?: boolean;
		liked?: boolean;
		onlike?: () => void;
		likeLabel?: string;
		unlikeLabel?: string;
		likePending?: boolean;
		onmore?: (event: MouseEvent) => void;
		moreLabel?: string;
		children?: Snippet;
	} = $props();
</script>

<div class="action-bar">
	{#if onplay}
		<button type="button" class="ab-btn primary" title={playing ? 'Pause' : playLabel} onclick={onplay}>
			{#if playing}
				<svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true"><rect x="6" y="5" width="4" height="14" rx="1" fill="currentColor" /><rect x="14" y="5" width="4" height="14" rx="1" fill="currentColor" /></svg>
			{:else}
				<svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true"><path d="M8 5.5v13a1 1 0 001.5.87l11-6.5a1 1 0 000-1.74l-11-6.5A1 1 0 008 5.5z" fill="currentColor" /></svg>
			{/if}
			<span class="label">{playing ? 'Pause' : playLabel}</span>
		</button>
	{/if}

	{#if onshuffle}
		<button type="button" class="ab-btn secondary" title={shuffleHint} onclick={onshuffle}>
			<svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true"><path d="M16 3h5v5M4 20l17-17M21 16v5h-5M4 4l5 5m6 6l6 6" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round" /></svg>
			<span class="label">Shuffle</span>
		</button>
	{/if}

	{#if onradio}
		<button
			type="button"
			class="ab-btn secondary"
			title={radioHint ?? radioLabel}
			disabled={radioPending}
			aria-busy={radioPending || undefined}
			onclick={onradio}
		>
			{#if radioPending}
				<span class="spinner" aria-hidden="true"></span>
			{:else}
				<svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true"><circle cx="12" cy="12" r="3" fill="currentColor" /><path d="M8.5 8.5a5 5 0 000 7M15.5 8.5a5 5 0 010 7M5.5 5.5a9 9 0 000 13M18.5 5.5a9 9 0 010 13" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" /></svg>
			{/if}
			<span class="label">{radioLabel}</span>
		</button>
	{/if}

	{#if onlike}
		<button
			type="button"
			class="ab-btn icon"
			class:liked
			title={liked ? unlikeLabel : likeLabel}
			aria-label={liked ? unlikeLabel : likeLabel}
			aria-pressed={liked}
			disabled={likePending}
			onclick={onlike}
		>
			<svg viewBox="0 0 24 24" width="20" height="20" aria-hidden="true"><path d="M12 21.35l-1.45-1.32C5.4 15.36 2 12.28 2 8.5 2 5.42 4.42 3 7.5 3c1.74 0 3.41.81 4.5 2.09C13.09 3.81 14.76 3 16.5 3 19.58 3 22 5.42 22 8.5c0 3.78-3.4 6.86-8.55 11.54L12 21.35z" fill={liked ? 'currentColor' : 'none'} stroke="currentColor" stroke-width={liked ? 0 : 2} /></svg>
		</button>
	{/if}

	{#if onmore}
		<button type="button" class="ab-btn icon" title={moreLabel} aria-label={moreLabel} aria-haspopup="menu" onclick={onmore}>
			<svg viewBox="0 0 24 24" width="20" height="20" aria-hidden="true"><circle cx="5" cy="12" r="1.8" fill="currentColor" /><circle cx="12" cy="12" r="1.8" fill="currentColor" /><circle cx="19" cy="12" r="1.8" fill="currentColor" /></svg>
		</button>
	{/if}

	{@render children?.()}
</div>

<style>
	.action-bar {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--space-2);
	}

	.ab-btn {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		gap: 8px;
		height: 40px;
		min-width: 40px;
		padding: 0 18px;
		border: 0;
		border-radius: 999px;
		background: transparent;
		color: var(--text-secondary);
		font-family: inherit;
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		white-space: nowrap;
		cursor: pointer;
		transition:
			background var(--motion-fast),
			color var(--motion-fast),
			filter var(--motion-fast);
	}

	.ab-btn:active:not(:disabled) {
		transform: scale(0.97);
		transition-duration: 90ms;
	}

	.ab-btn:disabled {
		cursor: progress;
		opacity: 0.7;
	}

	.ab-btn:focus-visible {
		outline: 2px solid var(--accent-strong);
		outline-offset: 2px;
	}

	.primary {
		background: var(--accent);
		color: var(--text-on-accent);
		padding-left: 16px;
	}

	.primary:hover:not(:disabled) {
		filter: brightness(1.1);
	}

	.secondary {
		background: var(--bg-surface);
		color: var(--text-primary);
		padding-left: 14px;
	}

	.secondary:hover:not(:disabled) {
		background: var(--bg-hover);
	}

	.icon {
		width: 40px;
		padding: 0;
	}

	.icon:hover:not(:disabled) {
		background: var(--bg-hover);
		color: var(--text-primary);
	}

	.icon.liked {
		color: var(--accent);
	}

	.icon.liked:hover:not(:disabled) {
		color: var(--accent-strong);
	}

	.spinner {
		width: 16px;
		height: 16px;
		border: 2px solid currentColor;
		border-right-color: transparent;
		border-radius: 50%;
		animation: spin 0.7s linear infinite;
	}

	@keyframes spin {
		to {
			transform: rotate(360deg);
		}
	}

	@container detail-hero (max-width: 1100px) {
		.label {
			position: absolute;
			width: 1px;
			height: 1px;
			overflow: hidden;
			clip-path: inset(50%);
			white-space: nowrap;
		}

		.primary,
		.secondary {
			width: 40px;
			padding: 0;
		}
	}

	@media (prefers-reduced-motion: reduce) {
		.spinner {
			animation-duration: 2s;
		}
	}
</style>
