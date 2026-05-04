<script lang="ts">
	import { get } from 'svelte/store';
	import type { Track } from '$lib/api/client';
	import { contextMenu, closeContextMenu } from '$lib/stores/context_menu';

	const SHUFFLE_LABELS: Record<string, string> = {
		off: 'Shuffle off',
		genre: 'Genre mix',
		weighted: 'Smart shuffle',
		true: 'True shuffle'
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
		onOpenMore
	}: {
		track: Track | null;
		isPlaying: boolean;
		shuffleMode: string;
		repeatMode: string;
		favoritePending?: boolean;
		onToggleFavorite: () => void;
		onCycleShuffle: () => void;
		onPrev: () => void;
		onPlayPause: () => void;
		onNext: () => void;
		onCycleRepeat: () => void;
		onOpenMore: (anchor: HTMLElement) => void;
	} = $props();

	let playPauseLabel = $derived(isPlaying ? 'Pause' : 'Play');

	function handleMoreClick(e: MouseEvent) {
		e.stopPropagation();
		if (get(contextMenu).open) {
			closeContextMenu();
		} else {
			onOpenMore(e.currentTarget as HTMLElement);
		}
	}
</script>

<div class="transport" aria-label="Playback controls">
	<div class="transport-group transport-group-secondary" role="group" aria-label="Track and shuffle controls">
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
		<button
			class:active={shuffleMode !== 'off'}
			class="tp-btn tp-mode-btn"
			title={SHUFFLE_LABELS[shuffleMode]}
			aria-label={SHUFFLE_LABELS[shuffleMode]}
			onclick={onCycleShuffle}
		>
			{SHUFFLE_ICONS[shuffleMode]}
		</button>
	</div>

	<div class="transport-group transport-group-playback" role="group" aria-label="Previous, play, and next">
		<button class="tp-btn" onclick={onPrev} aria-label="Previous" title="Previous track">⏮</button>
		<button class="tp-play" onclick={onPlayPause} aria-label={playPauseLabel} title={playPauseLabel}>
			{isPlaying ? '⏸' : '▶'}
		</button>
		<button class="tp-btn" onclick={onNext} aria-label="Next" title="Next track">⏭</button>
	</div>

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
</div>

<style>
	.transport {
		display: flex;
		align-items: center;
		justify-content: center;
		gap: 12px;
	}

	.transport-group {
		position: relative;
		display: flex;
		align-items: center;
		gap: 8px;
	}

	.transport-group + .transport-group::before {
		content: '';
		width: 1px;
		height: 24px;
		margin-right: 4px;
		background: color-mix(in srgb, var(--instrument-border) 54%, transparent);
	}

	.transport-group-playback {
		gap: 10px;
	}

	.tp-btn,
	.tp-play {
		width: 36px;
		height: 36px;
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

	.tp-like-btn {
		font-size: 18px;
		color: var(--text-secondary);
		transition:
			transform var(--motion-fast),
			background var(--motion-fast),
			border-color var(--motion-fast),
			color var(--motion-fast),
			box-shadow var(--motion-fast);
	}

	.tp-like-btn:active {
		transform: scale(0.92);
	}

	.tp-like-btn:disabled,
	.tp-btn:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}

	.tp-like-btn.active {
		color: #ff4d6d;
		background: color-mix(in srgb, #ff4d6d 15%, transparent);
		border-color: color-mix(in srgb, #ff4d6d 40%, transparent);
		box-shadow: 0 0 12px color-mix(in srgb, #ff4d6d 30%, transparent);
	}

	.tp-play {
		background: var(--accent);
		color: #fff;
		width: 42px;
		height: 42px;
		box-shadow: 0 10px 26px var(--accent-glow);
	}

	@media (max-width: 760px) {
		.transport {
			gap: 8px;
		}

		.transport-group {
			gap: 6px;
		}

		.transport-group + .transport-group::before {
			display: none;
		}
	}
</style>
