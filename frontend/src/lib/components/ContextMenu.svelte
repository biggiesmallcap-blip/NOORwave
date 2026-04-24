<script lang="ts">
	import { contextMenu, closeContextMenu, type MenuItem } from '$lib/stores/context_menu';

	let menuEl = $state<HTMLDivElement | null>(null);
	let openSubmenu = $state<number | null>(null);

	// Derived position keeps the menu inside the viewport.
	const MENU_W = 240;
	const MENU_H_ESTIMATE = 320;

	let position = $derived.by(() => {
		if (!$contextMenu.open) return { left: 0, top: 0 };
		const vw = typeof window !== 'undefined' ? window.innerWidth : 1920;
		const vh = typeof window !== 'undefined' ? window.innerHeight : 1080;
		const left = Math.min($contextMenu.x, vw - MENU_W - 8);
		const top = Math.min($contextMenu.y, vh - MENU_H_ESTIMATE - 8);
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
			closeContextMenu();
		}
	}

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
		style="left: {position.left}px; top: {position.top}px;"
		role="menu"
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
							{#if item.icon}
								<span class="context-menu-icon" aria-hidden="true">{item.icon}</span>
							{/if}
							<span class="context-menu-label">{item.label}</span>
							{#if item.submenu && item.submenu.length > 0}
								<span class="context-menu-caret" aria-hidden="true">›</span>
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
												{#if child.icon}
													<span class="context-menu-icon" aria-hidden="true">{child.icon}</span>
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
		z-index: 120;
		min-width: 220px;
		max-width: 280px;
		padding: 6px;
		background: color-mix(in srgb, var(--bg-surface-strong, #14162a) 94%, transparent);
		backdrop-filter: blur(18px) saturate(140%);
		-webkit-backdrop-filter: blur(18px) saturate(140%);
		border: 1px solid var(--border-subtle, rgba(255, 255, 255, 0.08));
		border-radius: 14px;
		box-shadow:
			0 18px 40px -12px rgba(0, 0, 0, 0.55),
			0 2px 6px rgba(0, 0, 0, 0.25);
		color: var(--text-primary);
		font-size: 0.85rem;
		animation: context-menu-pop 120ms ease-out;
	}

	@keyframes context-menu-pop {
		from {
			opacity: 0;
			transform: translateY(-4px) scale(0.98);
		}
		to {
			opacity: 1;
			transform: translateY(0) scale(1);
		}
	}

	.context-menu-title {
		padding: 6px 10px 4px;
		margin: 0;
		font-size: 0.68rem;
		font-weight: 600;
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
		color: var(--danger, #f87171);
	}

	.context-menu-item.danger:hover {
		background: color-mix(in srgb, var(--danger, #f87171) 16%, transparent);
	}

	.context-menu-icon {
		width: 18px;
		text-align: center;
		font-size: 0.95rem;
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
		font-size: 0.72rem;
		color: var(--text-tertiary, rgba(255, 255, 255, 0.45));
	}

	.context-menu-separator {
		height: 1px;
		margin: 4px 6px;
		background: var(--border-subtle, rgba(255, 255, 255, 0.08));
	}

	.context-submenu {
		position: absolute;
		top: 0;
		left: calc(100% - 4px);
		min-width: 200px;
		padding: 6px;
		list-style: none;
		background: color-mix(in srgb, var(--bg-surface-strong, #14162a) 94%, transparent);
		backdrop-filter: blur(18px) saturate(140%);
		-webkit-backdrop-filter: blur(18px) saturate(140%);
		border: 1px solid var(--border-subtle, rgba(255, 255, 255, 0.08));
		border-radius: 14px;
		box-shadow:
			0 18px 40px -12px rgba(0, 0, 0, 0.55),
			0 2px 6px rgba(0, 0, 0, 0.25);
		margin: 0;
		z-index: 1;
	}
</style>
