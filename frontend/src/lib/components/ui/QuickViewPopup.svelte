<script lang="ts">
	// The container shared by the album and TIDAL collection quick views
	// (STYLING.md "Album quick view and album page"). It owns the overlay
	// surface, focus, and dismissal: Escape, the close button, a click outside,
	// or a wheel over the page behind. Content comes in two parts: `hero`
	// (fixed at the top, room left for the close button) and `children` (the
	// body, which scrolls on its own). `busy` is the refresh state from STYLING.md
	// "States": a 2px bar under the hero; callers dim their own content.
	//
	// The backdrop never takes pointer events, so the page stays the wheel
	// target and keeps scrolling straight through a scroll-dismiss. (Chromium
	// latches a wheel gesture to its first target; a backdrop that vanished
	// mid-gesture would freeze page scrolling until the mouse moved.)
	//
	// Layers stacked above the quick view (context menu, toasts, the playlist
	// picker) mark themselves with data-floating-layer. Events inside them are
	// theirs, not outside interactions.
	import { onMount, type Snippet } from 'svelte';
	import { portal } from '$lib/actions/portal';
	import { contextMenu, closeContextMenu } from '$lib/stores/context_menu';

	let { title, onClose, busy = false, hero, children }: {
		title: string;
		onClose: () => void;
		busy?: boolean;
		/** Both snippets receive requestClose, so links can play the exit as they navigate. */
		hero: Snippet<[requestClose: () => void]>;
		children: Snippet<[requestClose: () => void]>;
	} = $props();

	const WHEEL_DISMISS_THRESHOLD = 6;
	// Completes the close if animationend never arrives (interrupted animation).
	const EXIT_FALLBACK_MS = 240;
	const FOCUSABLE = 'button:not(:disabled), a[href], input:not(:disabled), summary, [tabindex="0"]';

	let panel = $state<HTMLDivElement>();
	let closing = $state(false);
	let closed = false;
	let exitTimer: ReturnType<typeof setTimeout> | undefined;

	onMount(() => {
		const previousFocus = document.activeElement;
		panel?.focus();
		return () => {
			clearTimeout(exitTimer);
			if (previousFocus instanceof HTMLElement && previousFocus.isConnected) previousFocus.focus();
		};
	});

	function requestClose() {
		if (closing) return;
		closing = true;
		closeContextMenu();
		exitTimer = setTimeout(finishClose, EXIT_FALLBACK_MS);
	}

	function finishClose() {
		if (closed) return;
		closed = true;
		clearTimeout(exitTimer);
		onClose();
	}

	function inFloatingLayer(target: EventTarget | null) {
		return target instanceof Element && target.closest('[data-floating-layer]') !== null;
	}

	function isOutside(target: EventTarget | null) {
		return target instanceof Node && !panel?.contains(target) && !inFloatingLayer(target);
	}

	function handleWheel(event: WheelEvent) {
		if (closing || Math.abs(event.deltaY) < WHEEL_DISMISS_THRESHOLD || !isOutside(event.target)) return;
		requestClose();
	}

	// Close on an outside click and swallow it, so a card behind the popup is
	// not activated by the same click.
	function handleOutsideClick(event: MouseEvent) {
		if (closing || !isOutside(event.target)) return;
		event.preventDefault();
		event.stopPropagation();
		requestClose();
	}

	function handleKey(event: KeyboardEvent) {
		if (closing || event.defaultPrevented || inFloatingLayer(event.target)) return;
		if (event.key === 'Escape') {
			event.preventDefault();
			if ($contextMenu.open) closeContextMenu();
			else requestClose();
		} else if (event.key === 'Tab' && !$contextMenu.open) {
			trapFocus(event);
		}
	}

	function trapFocus(event: KeyboardEvent) {
		if (!panel) return;
		const focusable = panel.querySelectorAll<HTMLElement>(FOCUSABLE);
		const edge = event.shiftKey ? focusable[0] : focusable[focusable.length - 1];
		const wrapTo = event.shiftKey ? focusable[focusable.length - 1] : focusable[0];
		if (document.activeElement === edge || document.activeElement === panel) {
			event.preventDefault();
			wrapTo?.focus();
		}
	}
</script>

<svelte:window onkeydown={handleKey} onwheel={handleWheel} onclickcapture={handleOutsideClick} />

<div use:portal class="quick-view-backdrop" role="presentation">
	<div
		bind:this={panel}
		class="quick-view"
		class:closing
		role="dialog"
		aria-modal="true"
		aria-label={title}
		tabindex="-1"
		onanimationend={(event) => { if (closing && event.target === event.currentTarget) finishClose(); }}
	>
		<button type="button" class="close btn btn-icon" aria-label="Close" onclick={requestClose}>
			<svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><path d="M18 6 6 18M6 6l12 12" /></svg>
		</button>
		<div class="hero">{@render hero(requestClose)}</div>
		<div class="refresh-bar" class:active={busy} aria-hidden="true"></div>
		<div class="body">{@render children(requestClose)}</div>
	</div>
</div>

<style>
	.quick-view-backdrop {
		position: fixed;
		inset: 0;
		z-index: var(--z-modal);
		display: grid;
		place-items: center;
		pointer-events: none;
	}

	/* STYLING.md "Surfaces and colour roles": the overlay treatment. */
	.quick-view {
		position: relative;
		display: flex;
		flex-direction: column;
		width: min(var(--measure-form), calc(100vw - var(--space-6)));
		max-height: 86dvh;
		overflow: hidden;
		pointer-events: auto;
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-md);
		background: color-mix(in srgb, var(--bg-elevated) 96%, transparent);
		color: var(--text-primary);
		backdrop-filter: var(--blur-modal);
		box-shadow: var(--panel-shadow);
		animation: quick-view-in var(--motion-base) both;
	}

	.quick-view.closing {
		animation: quick-view-out var(--motion-exit) forwards;
		pointer-events: none;
	}

	.hero {
		padding: var(--space-5) var(--space-7) var(--space-3) var(--space-5);
	}

	.hero :global(.detail-hero) {
		margin: 0;
	}

	.refresh-bar {
		position: relative;
		height: 2px;
		flex-shrink: 0;
		overflow: hidden;
	}

	.refresh-bar.active::after {
		content: '';
		position: absolute;
		inset: 0 auto 0 0;
		width: 40%;
		background: var(--accent);
		animation: refresh-sweep 1.1s var(--ease-standard) infinite;
	}

	.body {
		min-height: 0;
		overflow-y: auto;
		overscroll-behavior: contain;
		scrollbar-width: thin;
		padding: var(--space-2) var(--space-5) var(--space-4);
	}

	.close {
		position: absolute;
		top: var(--space-3);
		right: var(--space-3);
		z-index: var(--z-raised);
		background: color-mix(in srgb, var(--bg-elevated) 55%, transparent);
	}

	/* STYLING.md "Motion": dialogs move by opacity and a 12px translate. Reduced
	   motion is handled globally in app.css; it shortens these to 1ms, so
	   animationend still completes the close. */
	@keyframes quick-view-in {
		from { opacity: 0; transform: translateY(12px); }
		to { opacity: 1; transform: none; }
	}

	@keyframes refresh-sweep {
		from { transform: translateX(-100%); }
		to { transform: translateX(250%); }
	}

	@keyframes quick-view-out {
		from { opacity: 1; transform: none; }
		to { opacity: 0; transform: translateY(12px); }
	}
</style>
