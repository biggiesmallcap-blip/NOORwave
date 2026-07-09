<script lang="ts">
	// Shared heterogeneous search-result row. Renders the canonical row recipe
	// (art / meta / kind badge / library indicator / more button) used by the
	// command palette and any future search surface. Routes the more-button and
	// right-click through the app-owned context-menu subsystem. Deliberately NOT
	// TrackRow.svelte, which models a single-kind local track with inline
	// transport controls; these rows are navigation-first and menu-delegated.
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import { openContextMenu, openMenuAtElement, type MenuItem } from '$lib/stores/context_menu';

	interface Props {
		art?: string | string[] | null;
		artShape?: 'square' | 'circle';
		fallbackText: string;
		title: string;
		subtitle?: string | null;
		kind?: string;
		kindTone?: 'muted' | 'spotify';
		inLibrary?: boolean;
		active?: boolean;
		href?: string | null;
		onActivate?: () => void;
		menuItems?: MenuItem[] | (() => MenuItem[]);
		menuTitle?: string;
		el?: HTMLElement | null;
	}

	let {
		art = null,
		artShape = 'square',
		fallbackText,
		title,
		subtitle = null,
		kind = undefined,
		kindTone = 'muted',
		inLibrary = false,
		active = false,
		href = null,
		onActivate = undefined,
		menuItems = undefined,
		menuTitle = undefined,
		el = $bindable(),
	}: Props = $props();

	const hasMenu = $derived(!!menuItems);

	function resolveMenu(): MenuItem[] {
		if (!menuItems) return [];
		return typeof menuItems === 'function' ? menuItems() : menuItems;
	}

	function handleContext(event: MouseEvent) {
		if (!hasMenu) return;
		event.preventDefault();
		event.stopPropagation();
		openContextMenu(event, resolveMenu(), menuTitle);
	}

	function openMore(event: MouseEvent) {
		event.stopPropagation();
		openMenuAtElement(event.currentTarget as HTMLElement, resolveMenu(), menuTitle);
	}

	function moreKeydown(event: KeyboardEvent) {
		if (event.key === 'Enter' || event.key === ' ') {
			event.preventDefault();
			event.stopPropagation();
			openMenuAtElement(event.currentTarget as HTMLElement, resolveMenu(), menuTitle);
		}
	}
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<li bind:this={el} class="srr-wrap" class:srr-wrap--active={active} oncontextmenu={handleContext}>
	<svelte:element
		this={href ? 'a' : 'button'}
		class="srr-row"
		href={href ?? undefined}
		type={href ? undefined : 'button'}
		onclick={() => onActivate?.()}
	>
		<ArtworkImage
			className={artShape === 'circle' ? 'row-art row-art--circle' : 'row-art'}
			src={art}
			alt={title}
			size={320}
			fallbackText={fallbackText}
		/>
		<span class="srr-meta">
			<span class="srr-title">{title}</span>
			{#if subtitle}<span class="srr-sub">{subtitle}</span>{/if}
		</span>
		{#if kind}<span class="srr-kind" class:srr-kind--spotify={kindTone === 'spotify'}>{kind}</span>{/if}
		{#if inLibrary}<span class="srr-lib" aria-label="In library">✓</span>{/if}
	</svelte:element>
	{#if hasMenu}
		<button
			class="srr-more"
			aria-label="Open actions"
			tabindex={-1}
			onclick={openMore}
			onkeydown={moreKeydown}
		>⋯</button>
	{/if}
</li>

<style>
	.srr-wrap {
		display: flex;
		align-items: stretch;
		position: relative;
	}
	.srr-wrap:hover,
	.srr-wrap--active {
		background: var(--bg-hover);
	}
	.srr-row {
		flex: 1;
		min-width: 0;
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 8px 18px;
		background: none;
		border: none;
		color: var(--text-primary);
		font-family: inherit;
		font-size: var(--font-size-sm);
		text-align: left;
		text-decoration: none;
		cursor: pointer;
	}
	.srr-meta {
		display: flex;
		flex-direction: column;
		gap: 1px;
		min-width: 0;
		flex: 1;
	}
	.srr-title {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		min-width: 0;
	}
	.srr-sub {
		font-size: var(--font-size-xs);
		color: var(--text-muted);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.srr-kind {
		font-size: var(--font-size-2xs);
		color: var(--text-muted);
		margin-left: auto;
		flex-shrink: 0;
	}
	.srr-kind--spotify {
		color: var(--service-spotify);
		font-weight: var(--font-weight-semibold);
	}
	.srr-lib {
		font-size: var(--font-size-2xs);
		color: var(--accent);
		flex-shrink: 0;
	}
	.srr-more {
		flex-shrink: 0;
		width: 32px;
		display: grid;
		place-items: center;
		background: none;
		border: none;
		color: var(--text-secondary);
		font-size: var(--font-size-md);
		cursor: pointer;
		opacity: 0;
		padding: 0 14px 0 4px;
		transition: opacity var(--motion-fast), color var(--motion-fast);
	}
	.srr-wrap:hover .srr-more,
	.srr-wrap--active .srr-more,
	.srr-more:hover,
	.srr-more:focus-visible {
		opacity: 1;
	}
	.srr-more:hover {
		color: var(--text-primary);
	}
</style>
