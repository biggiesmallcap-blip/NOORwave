<script lang="ts">
	import { untrack } from 'svelte';
	import type { ArchiveResponse } from '$lib/api/video_explore_types';
	import Dropdown from '$lib/components/ui/Dropdown.svelte';
	import Segmented from '$lib/components/ui/Segmented.svelte';
	import EraRiver from './EraRiver.svelte';
	import {
		CUTS,
		CUT_LABELS,
		DEPTHS,
		LENGTHS,
		SORTS,
		SOURCES,
		activeLensCount,
		chipCount,
		clearLenses,
		hueColor,
		toggleLens,
		type ArchiveLenses,
	} from './lenses';

	// The Archive's lens bar. Every chip carries a live count from the facets
	// (computed under the other active lenses), so you can see how much is
	// behind a lens before you pick it. Order stays put while counts change:
	// scenes keep the order they first arrived in, biggest first.

	let {
		lenses,
		facets,
		total,
		onchange,
	}: {
		lenses: ArchiveLenses;
		facets: ArchiveResponse['facets'] | null;
		total: number | null;
		onchange: (next: ArchiveLenses) => void;
	} = $props();

	const SCENES_COLLAPSED = 14;

	let sceneOrder = $state<number[]>([]);
	let sceneMeta = $state<Record<number, { name: string; hue: number }>>({});
	let scenesOpen = $state(false);

	// Remember every scene we have seen, in first-seen biggest-first order,
	// with its name and hue, so a picked scene keeps its chip even when the
	// other lenses leave it no songs.
	$effect(() => {
		const list = facets?.scene ?? [];
		if (list.length === 0) return;
		untrack(() => {
			const known = new Set(sceneOrder);
			const fresh = list.filter((scene) => !known.has(scene.id)).sort((a, b) => b.count - a.count);
			if (fresh.length) sceneOrder = [...sceneOrder, ...fresh.map((scene) => scene.id)];
			const meta = { ...sceneMeta };
			for (const scene of list) meta[scene.id] = { name: scene.name, hue: scene.hue };
			sceneMeta = meta;
		});
	});

	let sceneInfo = $derived(new Map((facets?.scene ?? []).map((scene) => [scene.id, scene])));

	let scenes = $derived.by(() => {
		const ids = [...sceneOrder];
		if (lenses.scene != null && !ids.includes(lenses.scene)) ids.unshift(lenses.scene);
		return ids.map((id) => {
			const live = sceneInfo.get(id);
			const known = sceneMeta[id];
			return {
				id,
				name: live?.name ?? known?.name ?? `Scene ${id}`,
				hue: live?.hue ?? known?.hue ?? 0,
				count: live?.count ?? 0,
			};
		});
	});

	let shownScenes = $derived.by(() => {
		if (scenesOpen || scenes.length <= SCENES_COLLAPSED + 2) return scenes;
		const head = scenes.slice(0, SCENES_COLLAPSED);
		const picked = scenes.find((scene) => scene.id === lenses.scene);
		return picked && !head.includes(picked) ? [picked, ...head] : head;
	});

	let cutCounts = $derived(new Map((facets?.cut ?? []).map((row) => [row.cut, row.count])));
	let depthCounts = $derived(new Map((facets?.depth ?? []).map((row) => [row.depth, row.count])));
	let lengthCounts = $derived(new Map((facets?.length ?? []).map((row) => [row.length, row.count])));
	let shownCuts = $derived(CUTS.filter((cut) => (cutCounts.get(cut) ?? 0) > 0 || lenses.cut === cut));
	let active = $derived(activeLensCount(lenses));

	function count(map: Map<string, number>, key: string): number | null {
		return facets ? (map.get(key) ?? 0) : null;
	}
</script>

<div class="lens-bar" aria-label="Archive lenses" role="group">
	<div class="lens-row">
		<span class="lens-label">Scene</span>
		<div class="chips scenes">
			{#if facets === null && scenes.length === 0}
				{#each Array(8) as _, i (i)}<span class="chip-ghost" style:--w={`${70 + ((i * 37) % 60)}px`}></span>{/each}
			{/if}
			{#each shownScenes as scene (scene.id)}
				<button
					type="button"
					class="chip"
					class:zero={scene.count === 0 && lenses.scene !== scene.id}
					aria-pressed={lenses.scene === scene.id}
					onclick={() => onchange(toggleLens(lenses, 'scene', scene.id))}
				>
					<span class="hue" style:background={hueColor(scene.hue)}></span>
					<span class="chip-name">{scene.name}</span>
					{#if facets}<span class="chip-count">{chipCount(scene.count)}</span>{/if}
				</button>
			{/each}
			{#if scenes.length > SCENES_COLLAPSED + 2}
				<button type="button" class="more" onclick={() => (scenesOpen = !scenesOpen)}>
					{scenesOpen ? 'Fewer scenes' : `All ${scenes.length} scenes`}
				</button>
			{/if}
		</div>
	</div>

	<div class="lens-row">
		<span class="lens-label">Era</span>
		<div class="era">
			<EraRiver
				facet={facets?.decade ?? []}
				from={lenses.eraFrom}
				to={lenses.eraTo}
				onchange={(eraFrom, eraTo) => onchange({ ...lenses, eraFrom, eraTo })}
			/>
		</div>
	</div>

	<div class="lens-row">
		<span class="lens-label">Cut</span>
		<div class="chips">
			{#each shownCuts as cut (cut)}
				<button
					type="button"
					class="chip"
					aria-pressed={lenses.cut === cut}
					onclick={() => onchange(toggleLens(lenses, 'cut', cut))}
				>
					<span class="chip-name">{CUT_LABELS[cut]}</span>
					<span class="chip-count">{chipCount(cutCounts.get(cut) ?? 0)}</span>
				</button>
			{/each}
			{#if facets === null}
				{#each Array(6) as _, i (i)}<span class="chip-ghost" style:--w={`${64 + ((i * 23) % 30)}px`}></span>{/each}
			{/if}
		</div>
	</div>

	<div class="lens-row split">
		<span class="lens-label">Depth</span>
		<div class="chips">
			{#each DEPTHS as option (option.value)}
				{@const n = count(depthCounts, option.value)}
				<button
					type="button"
					class="chip"
					class:zero={n === 0 && lenses.depth !== option.value}
					aria-pressed={lenses.depth === option.value}
					onclick={() => onchange(toggleLens(lenses, 'depth', option.value))}
				>
					<span class="chip-name">{option.label}</span>
					{#if n !== null}<span class="chip-count">{chipCount(n)}</span>{/if}
				</button>
			{/each}
		</div>
		<span class="lens-label">Length</span>
		<div class="chips">
			{#each LENGTHS as option (option.value)}
				{@const n = count(lengthCounts, option.value)}
				<button
					type="button"
					class="chip"
					class:zero={n === 0 && lenses.length !== option.value}
					aria-pressed={lenses.length === option.value}
					onclick={() => onchange(toggleLens(lenses, 'length', option.value))}
				>
					<span class="chip-name">{option.label}</span>
					{#if n !== null}<span class="chip-count">{chipCount(n)}</span>{/if}
				</button>
			{/each}
		</div>
	</div>

	<div class="toolbar">
		<div class="status">
			<span class="total" aria-live="polite">
				{#if total === null}
					&nbsp;
				{:else}
					{total.toLocaleString()} {total === 1 ? 'song' : 'songs'}
				{/if}
			</span>
			{#if active > 0}
				<button type="button" class="clear" onclick={() => onchange(clearLenses(lenses))}>
					Clear {active === 1 ? 'lens' : `${active} lenses`}
				</button>
			{/if}
		</div>
		<div class="tools">
			<Segmented
				options={SOURCES}
				value={lenses.source}
				label="Source"
				onchange={(source) => onchange({ ...lenses, source })}
			/>
			<Dropdown
				options={SORTS}
				value={lenses.sort}
				label="Sort"
				onchange={(sort) => onchange({ ...lenses, sort })}
			/>
		</div>
	</div>
</div>

<style>
	/* On the ground, no box: a label column and the lens controls beside it,
	   then the toolbar row (count left, source and sort right). */
	.lens-bar {
		display: grid;
		gap: var(--space-3);
		min-width: 0;
	}

	.lens-row {
		display: grid;
		grid-template-columns: 64px minmax(0, 1fr);
		align-items: start;
		gap: var(--space-3);
		min-width: 0;
	}

	.lens-row.split {
		grid-template-columns: 64px auto 64px minmax(0, 1fr);
	}

	/* Column-header recipe: the label role in tertiary. */
	.lens-label {
		padding-top: 7px;
		color: var(--text-tertiary);
		font-size: var(--font-size-label);
		font-weight: var(--font-weight-semibold);
		letter-spacing: 0.12em;
		text-transform: uppercase;
		line-height: var(--line-height-snug);
	}

	.lens-row.split .lens-label:nth-of-type(2) {
		padding-left: var(--space-4);
	}

	.chips {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
		min-width: 0;
	}

	.era {
		max-width: 760px;
		min-width: 0;
	}

	.chip {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		height: var(--control-h);
		padding: 0 12px;
		border-radius: 999px;
		border: 1px solid var(--border-subtle);
		background: transparent;
		color: var(--text-secondary);
		font-family: inherit;
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-medium);
		line-height: 1;
		white-space: nowrap;
		cursor: pointer;
		transition:
			background var(--motion-fast),
			border-color var(--motion-fast),
			color var(--motion-fast),
			opacity var(--motion-fast);
	}

	.chip:hover {
		background: var(--bg-hover);
		color: var(--text-primary);
	}

	.chip[aria-pressed='true'] {
		background: var(--accent-soft);
		border-color: var(--accent-line);
		color: var(--accent-strong);
	}

	.chip.zero {
		opacity: 0.5;
	}

	.chip:focus-visible,
	.more:focus-visible,
	.clear:focus-visible {
		outline: 2px solid var(--accent-strong);
		outline-offset: 2px;
	}

	.chip-name {
		max-width: 22ch;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.chip-count {
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
		font-variant-numeric: tabular-nums;
	}

	.chip[aria-pressed='true'] .chip-count {
		color: inherit;
		opacity: 0.8;
	}

	.hue {
		width: 8px;
		height: 8px;
		border-radius: 50%;
		flex: 0 0 auto;
	}

	.chip-ghost {
		width: var(--w);
		height: var(--control-h);
		border-radius: 999px;
		background: var(--bg-surface);
		animation: ghost-pulse 1.4s ease-in-out infinite;
	}

	@keyframes ghost-pulse {
		50% { opacity: 0.5; }
	}

	.more,
	.clear {
		height: var(--control-h);
		padding: 0 10px;
		border: 0;
		border-radius: 999px;
		background: none;
		color: var(--accent-strong);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		cursor: pointer;
	}

	.more:hover,
	.clear:hover {
		background: var(--bg-hover);
	}

	.toolbar {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-3);
		flex-wrap: wrap;
		min-height: 36px;
		margin-top: var(--space-2);
	}

	.status {
		display: flex;
		align-items: center;
		gap: var(--space-2);
	}

	.total {
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
		font-variant-numeric: tabular-nums;
	}

	.tools {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		flex-wrap: wrap;
	}

	@media (max-width: 760px) {
		.lens-row,
		.lens-row.split {
			grid-template-columns: minmax(0, 1fr);
			gap: 6px;
		}

		.lens-label {
			padding-top: 0;
		}

		.lens-row.split .lens-label:nth-of-type(2) {
			padding-left: 0;
		}
	}

	@media (prefers-reduced-motion: reduce) {
		.chip-ghost {
			animation: none;
		}
	}
</style>
