<script lang="ts">
	import { get } from 'svelte/store';
	import type { Track } from '$lib/api/client';
	import { contextMenu, closeContextMenu } from '$lib/stores/context_menu';

	const SHUFFLE_LABELS: Record<string, string> = {
		off: 'Shuffle off',
		genre: 'Genre mix',
		weighted: 'Smart shuffle',
		true: 'True random'
	};

	const SHUFFLE_ICONS: Record<string, string> = {
		off: '⇄',
		genre: '◆',
		weighted: '◉',
		true: '⤮'
	};

	const REPEAT_LABELS: Record<string, string> = {
		off: 'Repeat off',
		all: 'Repeat all',
		one: 'Repeat one'
	};

	const REPEAT_ICONS: Record<string, string> = {
		off: '↻',
		all: '↺',
		one: '⊙'
	};

	let {
		layout = 'side',
		track,
		isPlaying,
		shuffleMode,
		repeatMode,
		favoritePending = false,
		onToggleFavorite,
		onCycleShuffle,
		onPrev,
		onPlayPause,
		onNext,
		onCycleRepeat,
		onOpenMore,
		onEnterQuietMode
	}: {
		layout?: 'side' | 'bottom';
		track: Track | null;
		isPlaying: boolean;
		shuffleMode: string;
		repeatMode: string;
		favoritePending?: boolean;
		/** Optional. When omitted, the inline favorite button is hidden. Both
		 * the desktop panel and QuietMode pass it: the heart used to float over
		 * the desktop artwork, which collided with everything else once the
		 * queue expanded, so it lives here now. */
		onToggleFavorite?: () => void;
		onCycleShuffle: () => void;
		onPrev: () => void;
		onPlayPause: () => void;
		onNext: () => void;
		onCycleRepeat: () => void;
		onOpenMore: (anchor: HTMLElement) => void;
		onEnterQuietMode?: () => void;
	} = $props();

	let playPauseLabel = $derived(isPlaying ? 'Pause' : 'Play');
	let bottomActionsOpen = $state(false);
	let bottomActionsToggle = $state<HTMLButtonElement | null>(null);

	function handleMoreClick(e: MouseEvent) {
		e.stopPropagation();
		if (get(contextMenu).open) {
			closeContextMenu();
		} else {
			onOpenMore(e.currentTarget as HTMLElement);
		}
	}
</script>

<svelte:window
	onkeydown={(event) => {
		if (event.key === 'Escape' && bottomActionsOpen) {
			bottomActionsOpen = false;
			bottomActionsToggle?.focus();
		}
	}}
	onclick={(event) => {
		if (bottomActionsOpen && !(event.target as Element).closest('.bottom-actions-toggle, .bottom-actions-menu')) bottomActionsOpen = false;
	}}
/>

<div class="transport" class:bottom={layout === 'bottom'} aria-label="Playback controls">
	{#if layout === 'side'}
	<div class="transport-group transport-group-secondary" role="group" aria-label="Track and shuffle controls">
		{#if onToggleFavorite}
			<button
				class:active={track?.is_favorite}
				class="tp-btn tp-like-btn"
				title={track?.is_favorite ? 'Remove from favorites' : 'Add to favorites'}
				aria-label={track?.is_favorite ? 'Remove from favorites' : 'Add to favorites'}
				onclick={onToggleFavorite}
				disabled={favoritePending || !track}
			>
				{track?.is_favorite ? '♥' : '♡'}
			</button>
		{/if}
		<button
			class:active={shuffleMode !== 'off'}
			class="tp-btn tp-mode-btn"
			title={SHUFFLE_LABELS[shuffleMode]}
			aria-label={SHUFFLE_LABELS[shuffleMode]}
			onclick={onCycleShuffle}
			disabled={!track}
		>
			{SHUFFLE_ICONS[shuffleMode]}
		</button>
	</div>
	{/if}

	<div class="transport-group transport-group-playback" role="group" aria-label="Previous, play, and next">
		<button class="tp-btn" onclick={onPrev} aria-label="Previous" title="Previous track">⏮</button>
		<button class="tp-play" onclick={onPlayPause} aria-label={playPauseLabel} title={playPauseLabel}>
			{isPlaying ? '⏸' : '▶'}
		</button>
		<button class="tp-btn" onclick={onNext} aria-label="Next" title="Next track">⏭</button>
	</div>

	{#if layout === 'side'}
	<div class="transport-group transport-group-secondary" role="group" aria-label="Repeat and overflow controls">
		<button
			class:active={repeatMode !== 'off'}
			class="tp-btn tp-mode-btn"
			title={REPEAT_LABELS[repeatMode]}
			aria-label={REPEAT_LABELS[repeatMode]}
			onclick={onCycleRepeat}
		>
			{REPEAT_ICONS[repeatMode]}
		</button>
		<button
			class="tp-btn"
			title="More actions: song radio, play album, shuffle album"
			aria-label="More actions"
			onclick={handleMoreClick}
			disabled={!track}
		>⋯</button>
	</div>
	{:else}
		<button
			class="tp-btn bottom-actions-toggle"
			bind:this={bottomActionsToggle}
			type="button"
			aria-label="More playback controls"
			aria-haspopup="menu"
			aria-expanded={bottomActionsOpen}
			onclick={() => { bottomActionsOpen = !bottomActionsOpen; }}
		>⋯</button>
		{#if bottomActionsOpen}
			<div class="bottom-actions-menu" role="menu" aria-label="More playback controls">
				<button role="menuitem" disabled={!track || favoritePending} onclick={() => { onToggleFavorite?.(); bottomActionsOpen = false; }}>{track?.is_favorite ? 'Remove favorite' : 'Add favorite'}</button>
				<button role="menuitem" disabled={!track} onclick={() => { onCycleShuffle(); bottomActionsOpen = false; }}>{SHUFFLE_LABELS[shuffleMode]}</button>
				<button role="menuitem" disabled={!track} onclick={() => { onCycleRepeat(); bottomActionsOpen = false; }}>{REPEAT_LABELS[repeatMode]}</button>
				{#if onEnterQuietMode}<button role="menuitem" disabled={!track} onclick={() => { onEnterQuietMode?.(); bottomActionsOpen = false; }}>Quiet mode</button>{/if}
				<button role="menuitem" disabled={!track} onclick={(event) => { onOpenMore(bottomActionsToggle ?? (event.currentTarget as HTMLElement)); bottomActionsOpen = false; }}>Track actions</button>
			</div>
		{/if}
	{/if}
</div>

<style>
	.transport {
		display: grid;
		grid-template-columns: 1fr 1fr;
		align-items: center;
		column-gap: 8px;
		row-gap: 8px;
	}

	.transport:not(.bottom) .transport-group-playback {
		grid-column: 1 / -1;
		grid-row: 1;
		justify-content: center;
	}

	.transport:not(.bottom) .transport-group-secondary { grid-row: 2; }

	.transport.bottom {
		display: flex;
		gap: 8px;
		justify-content: center;
	}

	.bottom-actions-toggle {
		width: 40px;
		height: 40px;
	}

	.bottom-actions-menu {
		position: absolute;
		z-index: var(--z-overlay);
		bottom: calc(100% + 8px);
		right: 0;
		width: 184px;
		display: flex;
		flex-direction: column;
		padding: 6px;
		border: 1px solid var(--border-strong);
		border-radius: var(--radius-md);
		background: var(--bg-surface-strong);
		box-shadow: var(--panel-shadow);
	}

	.bottom-actions-menu button {
		min-height: 38px;
		padding: 8px 10px;
		border: 0;
		border-radius: var(--radius-sm);
		background: transparent;
		color: var(--text-primary);
		text-align: left;
		font: inherit;
		cursor: pointer;
	}

	.bottom-actions-menu button:hover,
	.bottom-actions-menu button:focus-visible { background: var(--accent-soft); }
	.bottom-actions-menu button:disabled { opacity: 0.45; cursor: default; }

	.transport-group {
		position: relative;
		display: flex;
		align-items: center;
		gap: 6px;
	}

	.transport > .transport-group-secondary:first-child {
		justify-self: start;
	}

	.transport > .transport-group-secondary:last-child {
		justify-self: end;
	}

	.transport-group-playback {
		gap: 8px;
	}

	.tp-btn,
	.tp-play {
		width: 40px;
		height: 40px;
		border-radius: 50%;
		display: grid;
		place-items: center;
		background: color-mix(in srgb, var(--instrument-surface) 82%, transparent);
		border: 1px solid color-mix(in srgb, var(--instrument-border) 58%, transparent);
		color: var(--text-primary);
		transition:
			transform var(--motion-fast),
			background var(--motion-fast),
			border-color var(--motion-fast),
			box-shadow var(--motion-fast);
	}

	.tp-btn:hover,
	.tp-play:hover {
		transform: translateY(-1px);
	}

	.tp-btn.active {
		background: var(--accent-soft);
		border-color: var(--accent-line);
		color: var(--accent-strong);
		box-shadow: 0 0 14px color-mix(in srgb, var(--accent-glow) 70%, transparent);
	}

	.tp-mode-btn {
		position: relative;
	}

	.tp-btn:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}

	.tp-play {
		background: var(--accent);
		color: #fff;
		width: 44px;
		height: 44px;
		box-shadow: 0 10px 26px var(--accent-glow);
	}

	@media (max-width: 760px) {
		.transport {
			gap: 8px;
		}

		.transport-group {
			gap: 6px;
		}
	}
</style>
