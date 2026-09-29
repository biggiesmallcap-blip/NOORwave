<script lang="ts">
	import { playerPlacement, type EffectivePlayerLayout, type PlayerPlacement } from '$lib/stores/playerLayout';

	let { effective } = $props<{ effective: EffectivePlayerLayout }>();
	let open = $state(false);
	let root = $state<HTMLDivElement | null>(null);
	let trigger = $state<HTMLButtonElement | null>(null);
	let fallback = $derived(effective !== 'mobile' && effective !== $playerPlacement);
	let effectiveLabel = $derived(effective === 'bottom' ? 'bottom' : effective === 'left' ? 'left' : 'right');

	const positions: { id: PlayerPlacement; label: string }[] = [
		{ id: 'right', label: 'Right side' },
		{ id: 'left', label: 'Left side' },
		{ id: 'bottom', label: 'Bottom' }
	];

	$effect(() => {
		if (!open) return;
		const closeOutside = (event: PointerEvent) => {
			if (root && !root.contains(event.target as Node)) open = false;
		};
		const closeOnEscape = (event: KeyboardEvent) => {
			if (event.key !== 'Escape') return;
			open = false;
			trigger?.focus();
		};
		window.addEventListener('pointerdown', closeOutside);
		window.addEventListener('keydown', closeOnEscape);
		return () => {
			window.removeEventListener('pointerdown', closeOutside);
			window.removeEventListener('keydown', closeOnEscape);
		};
	});

	function choose(position: PlayerPlacement) {
		playerPlacement.set(position);
		open = false;
		trigger?.focus();
	}
</script>

<div class="player-layout-select" class:opens-up={effective === 'bottom'} bind:this={root}>
	<button
		bind:this={trigger}
		class="corner-tab"
		class:active={open}
		type="button"
		aria-label={`Player position: ${effectiveLabel}`}
		aria-expanded={open}
		aria-controls="player-position-options"
		title={fallback ? `Bottom player until this window is wider; preferred: ${$playerPlacement}` : 'Change player position'}
		onclick={() => { open = !open; }}
	>
		<svg viewBox="0 0 20 20" aria-hidden="true" focusable="false">
			<rect x="2.5" y="3" width="15" height="14" rx="2" />
			{#if effective === 'bottom'}
				<path d="M2.5 12h15" />
			{:else if effective === 'left'}
				<path d="M8 3v14" />
			{:else}
				<path d="M12 3v14" />
			{/if}
		</svg>
		{#if fallback}<span class="fallback-dot" aria-hidden="true"></span>{/if}
	</button>
	{#if open}
		<div id="player-position-options" class="layout-menu" role="group" aria-label="Player position choices">
			<span class="menu-heading">Player position</span>
			{#each positions as position (position.id)}
				<button
					type="button"
					class="layout-option"
					class:selected={$playerPlacement === position.id}
					aria-pressed={$playerPlacement === position.id}
					onclick={() => choose(position.id)}
				>
					<span>{position.label}</span>
					<span class="check" aria-hidden="true">{$playerPlacement === position.id ? '✓' : ''}</span>
				</button>
			{/each}
			{#if fallback}<p class="fallback-note">Bottom until the window is wider</p>{/if}
		</div>
	{/if}
</div>

<style>
	.player-layout-select {
		position: relative;
		z-index: 2;
		flex: none;
		width: 32px;
		height: 32px;
	}

	.corner-tab {
		position: relative;
		display: grid;
		place-items: center;
		width: 32px;
		height: 32px;
		padding: 0;
		border: 1px solid transparent;
		border-radius: 3px 3px 3px 11px;
		background: transparent;
		color: var(--text-tertiary);
		cursor: pointer;
	}

	.corner-tab::before {
		content: '';
		position: absolute;
		top: 0;
		right: 0;
		width: 8px;
		height: 8px;
		border-left: 1px solid var(--border-subtle);
		border-bottom: 1px solid var(--border-subtle);
		border-bottom-left-radius: 2px;
		background: var(--player-surface);
		opacity: 0.7;
		pointer-events: none;
	}

	.corner-tab:hover,
	.corner-tab.active {
		border-color: var(--border-subtle);
		background: var(--bg-hover);
		color: var(--text-primary);
	}

	.corner-tab:focus-visible {
		outline: 2px solid var(--accent);
		outline-offset: 2px;
	}

	.corner-tab svg {
		width: 16px;
		height: 16px;
		fill: none;
		stroke: currentColor;
		stroke-width: 1.4;
	}

	.fallback-dot {
		position: absolute;
		right: 2px;
		bottom: 2px;
		width: 4px;
		height: 4px;
		border-radius: 50%;
		background: var(--accent);
		pointer-events: none;
	}

	.layout-menu {
		position: absolute;
		top: calc(100% + 8px);
		right: 0;
		z-index: var(--z-overlay);
		width: 188px;
		padding: 6px;
		border: 1px solid var(--border-strong);
		border-radius: var(--radius-md);
		background: var(--bg-surface-strong);
		box-shadow: var(--panel-shadow);
	}

	.opens-up .layout-menu {
		top: auto;
		bottom: calc(100% + 8px);
	}

	.menu-heading {
		display: block;
		padding: 5px 9px 7px;
		color: var(--text-tertiary);
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-bold);
		letter-spacing: 0.08em;
		text-transform: uppercase;
	}

	.layout-option {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 12px;
		width: 100%;
		min-height: 34px;
		padding: 6px 9px;
		border: 0;
		border-radius: var(--radius-sm);
		background: transparent;
		color: var(--text-secondary);
		font: inherit;
		font-size: var(--font-size-xs);
		text-align: left;
		white-space: nowrap;
		cursor: pointer;
	}

	.layout-option:hover,
	.layout-option:focus-visible,
	.layout-option.selected {
		background: var(--accent-soft);
		color: var(--text-primary);
	}

	.layout-option:focus-visible { outline: 2px solid var(--accent); outline-offset: -2px; }
	.check { width: 14px; color: var(--accent-strong); text-align: right; }
	.fallback-note { margin: 6px 9px 3px; color: var(--text-tertiary); font-size: var(--font-size-2xs); line-height: 1.3; }
</style>
