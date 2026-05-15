<script lang="ts">
	import { goto } from '$app/navigation';
	import { tap } from '$lib/remote/tap';
	import type { Snippet } from 'svelte';

	// Backdrop and disconnect banner are owned by /remote/+layout.svelte so
	// they persist across page navigations without re-installing.
	let {
		title = '',
		children
	}: {
		title?: string;
		children: Snippet;
	} = $props();

	function onBack() {
		// `history.length` is 1 on a cold deep-link (or 2 on some browsers); fall
		// back to the remote home in that case so the back button never feels
		// dead-ended.
		if (typeof history !== 'undefined' && history.length > 1) {
			history.back();
		} else {
			void goto('/remote');
		}
	}
</script>

<header class="remote-shell-head">
	<button type="button" class="remote-shell-back" aria-label="Back" use:tap={onBack}>
		<svg viewBox="0 0 24 24" aria-hidden="true" focusable="false">
			<path d="M15 6l-6 6 6 6" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" />
		</svg>
	</button>
	{#if title}
		<h1>{title}</h1>
	{/if}
	<span class="remote-shell-spacer" aria-hidden="true"></span>
</header>

<main class="remote-shell-main">
	{@render children()}
</main>

<style>
	.remote-shell-head {
		position: sticky;
		top: 0;
		z-index: 5;
		display: grid;
		grid-template-columns: 44px 1fr 44px;
		align-items: center;
		gap: 8px;
		padding: max(10px, env(safe-area-inset-top)) 16px 10px;
		/* No backdrop-filter on iOS PWA — it creates a stacking context that
		   has been observed to drop touch events on descendant controls in
		   standalone mode. A solid-ish gradient gives the same visual read
		   without the input glitch. */
		background: linear-gradient(
			180deg,
			color-mix(in oklab, var(--bg-base) 96%, transparent) 0%,
			color-mix(in oklab, var(--bg-base) 82%, transparent) 70%,
			color-mix(in oklab, var(--bg-base) 60%, transparent) 100%
		);
	}

	.remote-shell-back {
		width: 44px;
		height: 44px;
		display: grid;
		place-items: center;
		border-radius: 999px;
		background: var(--surface-1);
		color: var(--text-primary);
		/* Kill iOS Safari's 300ms tap-delay and double-tap zoom on this
		   button so rapid back taps fire reliably. */
		touch-action: manipulation;
		-webkit-tap-highlight-color: transparent;
	}

	.remote-shell-back:active {
		background: var(--surface-2);
	}

	.remote-shell-back svg {
		width: 22px;
		height: 22px;
		/* iOS Safari can deliver the tap to the inner SVG instead of the
		   button when the parent has a backdrop-filter stacking context.
		   Send pointer events straight to the button. */
		pointer-events: none;
	}

	.remote-shell-head h1 {
		margin: 0;
		text-align: center;
		color: var(--text-primary);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.remote-shell-spacer {
		width: 44px;
		height: 44px;
	}

	.remote-shell-main {
		position: relative;
		z-index: 1;
		min-height: 100svh;
		padding: 4px 16px max(22px, env(safe-area-inset-bottom));
		color: var(--text-primary);
		display: grid;
		gap: 18px;
		align-content: start;
		/* iOS PWA rubber-band overscroll can silently lock the scroll
		   position on long lists after a hard reversal. `contain` keeps
		   the chain inside the main area without disabling normal scroll
		   momentum. */
		overscroll-behavior-y: contain;
	}
</style>
