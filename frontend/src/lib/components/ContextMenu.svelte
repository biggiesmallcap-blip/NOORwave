<script lang="ts">
	import {
		contextMenu,
		closeContextMenu,
		cancelContextMenuClose,
		menuIconForDisplay,
		type MenuItem
	} from '$lib/stores/context_menu';

	let menuEl = $state<HTMLDivElement | null>(null);
	let openSubmenu = $state<number | null>(null);

	// Derived position keeps the menu inside the viewport.
	const MENU_W = 240;
	const MENU_H_ESTIMATE = 480;

	let position = $derived.by(() => {
		if (!$contextMenu.open) return { left: 0, top: 0 };
		const vw = typeof window !== 'undefined' ? window.innerWidth : 1920;
		const vh = typeof window !== 'undefined' ? window.innerHeight : 1080;
		const menuHeight = menuEl?.offsetHeight ?? MENU_H_ESTIMATE;
		const left = Math.min($contextMenu.x, vw - MENU_W - 8);
		const top = Math.min($contextMenu.y, vh - menuHeight - 8);
		return { left: Math.max(8, left), top: Math.max(8, top) };
	});

	$effect(() => {
		if (!$contextMenu.open) {
			openSubmenu = null;
		}
	});

	function handleWindowClick(event: MouseEvent) {
		if (!$contextMenu.open) return;
		if (menuEl && event.target instanceof Node && menuEl.contains(event.target)) return;
		closeContextMenu();
	}

	function handleKey(event: KeyboardEvent) {
		if (event.key === 'Escape' && $contextMenu.open) {
			event.preventDefault();
			event.stopPropagation();
			closeContextMenu();
		}
	}

	function handlePointerLeave() {
		if (!$contextMenu.open) return;
		closeContextMenu();
	}

	function handlePointerEnter() {
		if (!$contextMenu.open) return;
		cancelContextMenuClose();
	}

	// Position is a snapshot taken at open time, so any scroll desyncs the menu
	// from its trigger. A transformed/will-change ancestor can also turn
	// `position: fixed` into ancestor-relative, making the menu drift visually.
	// In both cases, closing on scroll matches typical app behaviour.
	$effect(() => {
		if (!$contextMenu.open) return;
		const onScroll = () => closeContextMenu();
		// Scroll events do not bubble, so listen in the capture phase to catch
		// any nested scroller without needing per-target wiring.
		window.addEventListener('scroll', onScroll, { capture: true, passive: true });
		return () => window.removeEventListener('scroll', onScroll, { capture: true });
	});

	async function activate(item: MenuItem, index: number) {
		if (item.disabled) return;
		if (item.submenu && item.submenu.length > 0) {
			openSubmenu = openSubmenu === index ? null : index;
			return;
		}
		if (item.onSelect) {
			try {
				await item.onSelect();
			} catch (error) {
				console.error('Context menu action failed:', error);
			}
		}
		closeContextMenu();
	}
</script>

<svelte:window onclick={handleWindowClick} oncontextmenu={handleWindowClick} onkeydown={handleKey} />

{#if $contextMenu.open}
	<div
		bind:this={menuEl}
		class="context-menu"
		class:closing={$contextMenu.closing}
		style="left: {position.left}px; top: {position.top}px;"
		role="menu"
		tabindex="-1"
		onpointerenter={handlePointerEnter}
		onpointerleave={handlePointerLeave}
	>
		{#if $contextMenu.title}
			<p class="context-menu-title">{$contextMenu.title}</p>
		{/if}
		<ul class="context-menu-list">
			{#each $contextMenu.items as item, index (index)}
				{#if item.separator}
					<li class="context-menu-separator" role="separator"></li>
				{:else}
					<li class="context-menu-item-wrap" class:has-submenu-open={openSubmenu === index}>
						<button
							type="button"
							role="menuitem"
							class="context-menu-item"
							class:danger={item.danger}
							class:disabled={item.disabled}
							disabled={item.disabled}
							onclick={() => void activate(item, index)}
						>
							{#if menuIconForDisplay(item.icon)}
								<span class="context-menu-icon" aria-hidden="true">{menuIconForDisplay(item.icon)}</span>
							{/if}
							<span class="context-menu-label">{item.label}</span>
							{#if item.submenu && item.submenu.length > 0}
								<span
									class="context-menu-caret"
									class:open={openSubmenu === index}
									aria-hidden="true">›</span>
							{:else if item.hint}
								<span class="context-menu-hint">{item.hint}</span>
							{/if}
						</button>

						{#if item.submenu && openSubmenu === index}
							<ul class="context-submenu" role="menu">
								{#each item.submenu as child, childIndex (childIndex)}
									{#if child.separator}
										<li class="context-menu-separator" role="separator"></li>
									{:else}
										<li>
											<button
												type="button"
												role="menuitem"
												class="context-menu-item"
												class:danger={child.danger}
												class:disabled={child.disabled}
												disabled={child.disabled}
												onclick={() => void activate(child, -1)}
											>
												{#if menuIconForDisplay(child.icon)}
													<span class="context-menu-icon" aria-hidden="true">{menuIconForDisplay(child.icon)}</span>
												{/if}
												<span class="context-menu-label">{child.label}</span>
												{#if child.hint}
													<span class="context-menu-hint">{child.hint}</span>
												{/if}
											</button>
										</li>
									{/if}
								{/each}
							</ul>
						{/if}
					</li>
				{/if}
			{/each}
		</ul>
	</div>
{/if}

<style>
	.context-menu {
		position: fixed;
		z-index: var(--z-toast);
		min-width: 220px;
		max-width: 280px;
		max-height: calc(100dvh - 16px);
		overflow-y: auto;
		padding: 6px;
		background: color-mix(in srgb, var(--bg-surface-strong, #14162a) 94%, transparent);
		backdrop-filter: var(--blur-modal);
		-webkit-backdrop-filter: var(--blur-modal);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-md);
		box-shadow:
			0 18px 40px -12px rgba(0, 0, 0, 0.55),
			0 2px 6px rgba(0, 0, 0, 0.25);
		color: var(--text-primary);
		font-size: var(--font-size-sm);
		scrollbar-width: thin;
		will-change: opacity, transform, filter;
		animation: context-menu-enter 160ms cubic-bezier(0.2, 0.9, 0.25, 1) both;
	}

	.context-menu.closing {
		animation: context-menu-exit 160ms ease-in both;
	}

	@keyframes context-menu-enter {
		from {
			opacity: 0;
			filter: blur(6px);
			transform: translateY(-5px) scale(0.98);
		}
		to {
			opacity: 1;
			filter: blur(0);
			transform: translateY(0) scale(1);
		}
	}

	@keyframes context-menu-exit {
		from {
			opacity: 1;
			filter: blur(0);
			transform: translateY(0) scale(1);
		}
		to {
			opacity: 0;
			filter: blur(5px);
			transform: translateY(-3px) scale(0.985);
		}
	}

	.context-menu-title {
		padding: 6px 10px 4px;
		margin: 0;
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		letter-spacing: 0.08em;
		text-transform: uppercase;
		color: var(--text-tertiary, rgba(255, 255, 255, 0.45));
	}

	.context-menu-list {
		list-style: none;
		padding: 0;
		margin: 0;
	}

	.context-menu-item-wrap {
		position: relative;
	}

	.context-menu-item {
		all: unset;
		box-sizing: border-box;
		display: flex;
		align-items: center;
		gap: 10px;
		width: 100%;
		padding: 8px 10px;
		border-radius: 8px;
		cursor: pointer;
		transition: background 80ms ease;
	}

	.context-menu-item:hover,
	.context-menu-item:focus-visible {
		background: color-mix(in srgb, var(--accent-strong, #6366f1) 18%, transparent);
	}

	.context-menu-item.disabled,
	.context-menu-item:disabled {
		opacity: 0.45;
		cursor: not-allowed;
	}

	.context-menu-item.disabled:hover {
		background: transparent;
	}

	.context-menu-item.danger {
		color: var(--state-error);
	}

	.context-menu-item.danger:hover {
		background: color-mix(in srgb, var(--state-error) 16%, transparent);
	}

	.context-menu-icon {
		width: 18px;
		text-align: center;
		font-size: var(--font-size-md);
		opacity: 0.85;
	}

	.context-menu-label {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.context-menu-caret,
	.context-menu-hint {
		margin-left: 8px;
		font-size: var(--font-size-xs);
		color: var(--text-tertiary, rgba(255, 255, 255, 0.45));
	}

	.context-menu-caret {
		transition: transform 120ms ease;
	}

	.context-menu-caret.open {
		transform: rotate(90deg);
	}

	.context-menu-separator {
		height: 1px;
		margin: 4px 6px;
		background: var(--border-subtle, rgba(255, 255, 255, 0.08));
	}

	/* Submenus expand inline (indented), not as a floating flyout. A flyout gets
	   clipped by the menu's own scroll container and the viewport edge in small
	   windows; inline expansion always fits and just scrolls with the menu. */
	.context-submenu {
		list-style: none;
		margin: 2px 0 2px 18px;
		padding: 0 0 0 4px;
		border-left: 1px solid var(--border-subtle);
	}

	.context-submenu .context-menu-item {
		padding-left: 8px;
	}
</style>
