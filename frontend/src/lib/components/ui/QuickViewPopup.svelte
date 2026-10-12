<script lang="ts">
	import { onMount, type Snippet } from 'svelte';
	import { portal } from '$lib/actions/portal';
	import { contextMenu, closeContextMenu } from '$lib/stores/context_menu';
	import { playlistPicker } from '$lib/stores/add_to_playlist';

	let { title, onClose, children }: {
		title: string;
		onClose: () => void;
		children: Snippet<[() => void]>;
	} = $props();
	let panel = $state<HTMLDivElement>();
	let closing = $state(false);
	let closed = false;
	let closeTimer: ReturnType<typeof setTimeout>;

	onMount(() => {
		const previousFocus = document.activeElement;
		panel?.focus();
		return () => {
			clearTimeout(closeTimer);
			if (previousFocus instanceof HTMLElement && previousFocus.isConnected) previousFocus.focus();
		};
	});

	function finishClose() {
		if (closed) return;
		closed = true;
		onClose();
	}

	function requestClose() {
		if (closing) return;
		closing = true;
		closeContextMenu();
		// Also complete when reduced motion or interruption suppresses animationend.
		closeTimer = setTimeout(finishClose, 240);
	}

	function isInside(target: EventTarget | null) {
		return target instanceof Node && (panel?.contains(target)
			|| (target instanceof Element && target.closest('.context-menu')));
	}

	function handleWheel(event: WheelEvent) {
		if (closing || $playlistPicker || isInside(event.target) || Math.abs(event.deltaY) < 6) return;
		requestClose();
	}

	function handleOutsideClick(event: MouseEvent) {
		if (closing || $playlistPicker || isInside(event.target)) return;
		event.preventDefault();
		event.stopPropagation();
		requestClose();
	}

	function handleKey(event: KeyboardEvent) {
		if ($playlistPicker || event.defaultPrevented) return;
		if (event.key === 'Escape') {
			event.preventDefault();
			if ($contextMenu.open) closeContextMenu();
			else requestClose();
		}
		if (event.key !== 'Tab' || $contextMenu.open || !panel) return;
		const focusable = [...panel.querySelectorAll<HTMLElement>('button:not(:disabled), a[href], input:not(:disabled), [tabindex="0"]')];
		const first = focusable[0];
		const last = focusable.at(-1);
		if (event.shiftKey && (document.activeElement === first || document.activeElement === panel)) {
			event.preventDefault(); last?.focus();
		} else if (!event.shiftKey && (document.activeElement === last || document.activeElement === panel)) {
			event.preventDefault(); first?.focus();
		}
	}
</script>

<svelte:window onkeydown={handleKey} onwheel={handleWheel} onclickcapture={handleOutsideClick} />

<div use:portal class="popup-backdrop" role="presentation">
	<!-- Keyboard dismissal and focus trapping are handled on window above. -->
	<!-- svelte-ignore a11y_click_events_have_key_events -->
	<div class="popup-panel" class:closing bind:this={panel} role="dialog" tabindex="-1"
		aria-modal="true" aria-label={title}
		onclick={(event) => event.stopPropagation()}
		onanimationend={(event) => { if (closing && event.target === event.currentTarget) finishClose(); }}>
		<button class="popup-close btn btn-icon" aria-label="Close" onclick={requestClose}>
			<svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><path d="M18 6 6 18M6 6l12 12" /></svg>
		</button>
		{@render children(requestClose)}
	</div>
</div>

<style>
	.popup-backdrop {
		position: fixed;
		inset: 0;
		z-index: var(--z-modal);
		display: grid;
		place-items: center;
		/* Keep the page as the wheel target, so scrolling continues through dismissal. */
		pointer-events: none;
	}
	.popup-panel {
		position: relative;
		width: min(820px, 92vw);
		max-height: 86dvh;
		pointer-events: auto;
		display: flex;
		flex-direction: column;
		border-radius: var(--radius-md);
		border: 1px solid var(--border-subtle);
		background: color-mix(in srgb, var(--bg-elevated) 96%, transparent);
		color: var(--text-primary);
		backdrop-filter: var(--blur-modal);
		box-shadow: var(--panel-shadow);
		animation: popup-bloom var(--motion-base) both;
		overflow: hidden;
	}
	.popup-panel.closing {
		animation: popup-collapse var(--motion-exit) forwards;
		pointer-events: none;
	}
	.popup-close {
		position: absolute;
		top: var(--space-3);
		right: var(--space-3);
		z-index: var(--z-raised);
		background: color-mix(in srgb, var(--bg-elevated) 55%, transparent);
	}
	@keyframes popup-bloom {
		from { opacity: 0; transform: scale(0.975) translateY(10px); }
		to { opacity: 1; transform: scale(1) translateY(0); }
	}
	@keyframes popup-collapse {
		from { opacity: 1; transform: scale(1) translateY(0); }
		to { opacity: 0; transform: scale(0.965) translateY(16px); }
	}
	@media (prefers-reduced-motion: reduce) {
		.popup-panel, .popup-panel.closing { animation: none; }
	}
	:global([data-motion="reduce"]) .popup-panel { animation: none; }
</style>
