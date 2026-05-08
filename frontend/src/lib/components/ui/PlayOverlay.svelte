<script lang="ts">
	type State = 'play' | 'pause' | 'loading';
	type Position = 'center' | 'corner';

	type Props = {
		position?: Position;
		state?: State;
		label?: string;
		size?: 'sm' | 'md' | 'lg';
		forceVisible?: boolean;
		disabled?: boolean;
		onclick?: (e: MouseEvent) => void;
	};

	let {
		position = 'center',
		state = 'play',
		label = 'Play',
		size = 'md',
		forceVisible = false,
		disabled = false,
		onclick,
	}: Props = $props();
</script>

{#snippet icon()}
	{#if state === 'loading'}
		<span class="po-spinner" aria-hidden="true"></span>
	{:else if state === 'pause'}
		<svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">
			<rect x="3" y="3" width="3.5" height="10" fill="currentColor" rx="0.5" />
			<rect x="9.5" y="3" width="3.5" height="10" fill="currentColor" rx="0.5" />
		</svg>
	{:else}
		<svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">
			<path d="M5 3l8 5-8 5V3z" fill="currentColor" />
		</svg>
	{/if}
{/snippet}

{#if onclick}
	<button
		type="button"
		class="play-overlay"
		class:position-center={position === 'center'}
		class:position-corner={position === 'corner'}
		class:size-sm={size === 'sm'}
		class:size-md={size === 'md'}
		class:size-lg={size === 'lg'}
		class:force-visible={forceVisible}
		{disabled}
		aria-label={label}
		{onclick}
	>
		{@render icon()}
	</button>
{:else}
	<div
		class="play-overlay decorative"
		class:position-center={position === 'center'}
		class:position-corner={position === 'corner'}
		class:size-sm={size === 'sm'}
		class:size-md={size === 'md'}
		class:size-lg={size === 'lg'}
		class:force-visible={forceVisible}
		aria-hidden="true"
	>
		{@render icon()}
	</div>
{/if}

<style>
	.play-overlay {
		position: absolute;
		display: grid;
		place-items: center;
		border-radius: 50%;
		background: rgba(0, 0, 0, 0.55);
		color: #fff;
		border: 1px solid rgba(255, 255, 255, 0.15);
		backdrop-filter: var(--blur-base);
		-webkit-backdrop-filter: var(--blur-base);
		cursor: pointer;
		opacity: 0;
		transform: translateY(4px);
		transition:
			opacity var(--motion-base),
			transform var(--motion-base),
			background var(--motion-base);
		box-shadow: 0 6px 16px -4px rgba(0, 0, 0, 0.5);
		padding: 0;
		font: inherit;
	}

	.size-sm { width: 32px;  height: 32px; }
	.size-md { width: 44px;  height: 44px; }
	.size-lg { width: 56px;  height: 56px; }

	.size-sm svg { width: 12px; height: 12px; }
	.size-lg svg { width: 20px; height: 20px; }

	.position-center {
		inset: 0;
		margin: auto;
	}

	.position-corner {
		right: 8px;
		bottom: 8px;
	}

	.play-overlay:hover:not(:disabled) {
		background: rgba(0, 0, 0, 0.72);
		transform: translateY(0) scale(1.06);
	}

	.play-overlay:focus-visible,
	.play-overlay.force-visible {
		opacity: 1;
		transform: translateY(0);
	}

	.play-overlay:disabled {
		cursor: default;
		opacity: 0.5;
		transform: none;
	}

	.play-overlay.decorative {
		pointer-events: none;
	}

	.po-spinner {
		width: 16px;
		height: 16px;
		border: 2px solid rgba(255, 255, 255, 0.28);
		border-top-color: #fff;
		border-radius: 50%;
		animation: po-spin 0.8s linear infinite;
	}
	.size-sm .po-spinner { width: 12px; height: 12px; border-width: 1.5px; }
	.size-lg .po-spinner { width: 20px; height: 20px; }

	@keyframes po-spin {
		to { transform: rotate(360deg); }
	}
</style>
