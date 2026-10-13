<script lang="ts">
	import { untrack } from 'svelte';
	import type { ArchiveResponse } from '$lib/api/video_explore_types';
	import Dropdown from '$lib/components/ui/Dropdown.svelte';
	import Segmented from '$lib/components/ui/Segmented.svelte';
	import EraBars from './EraBars.svelte';
	import LensPopover from './LensPopover.svelte';
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
		eraLabel,
		hueColor,
		toggleLens,
		type ArchiveLenses,
	} from './lenses';

	// The Archive's lens row: one line on the ground, the lenses on the left
	// (Scene, Era, Cut, Depth, Length), the count, source and sort on the
	// right. Each lens is a pill that opens a small panel of choices; a picked
	// lens shows its value in the pill with an x to put it down. Every choice
	// carries a live count from the facets (computed under the other active
	// lenses), so you can see how much is behind it before you pick it.

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

	let sceneQuery = $state('');

	// Scene names and hues outlive the facets that carried them, so a picked
	// scene keeps its name when the other lenses leave it no songs.
	let sceneMeta = $state<Record<number, { name: string; hue: number }>>({});
	$effect(() => {
		const list = facets?.scene ?? [];
		if (list.length === 0) return;
		untrack(() => {
			const meta = { ...sceneMeta };
			for (const scene of list) meta[scene.id] = { name: scene.name, hue: scene.hue };
			sceneMeta = meta;
		});
	});

	/** Biggest first under the current lenses; the picked one leads, empty
	 *  ones sink to the end, and the search narrows by name. */
	let scenes = $derived.by(() => {
		const live = facets?.scene ?? [];
		const needle = sceneQuery.trim().toLowerCase();
		const list = live
			.map((scene) => ({ id: scene.id, name: scene.name, hue: scene.hue, count: scene.count }))
			.filter((scene) => !needle || scene.name.toLowerCase().includes(needle));
		list.sort((a, b) => {
			if (a.id === lenses.scene) return -1;
			if (b.id === lenses.scene) return 1;
			return b.count - a.count || a.name.localeCompare(b.name);
		});
		return list;
	});

	let picked = $derived(lenses.scene != null ? (sceneMeta[lenses.scene] ?? null) : null);
	let cutCounts = $derived(new Map((facets?.cut ?? []).map((row) => [row.cut, row.count])));
	let depthCounts = $derived(new Map((facets?.depth ?? []).map((row) => [row.depth, row.count])));
	let lengthCounts = $derived(new Map((facets?.length ?? []).map((row) => [row.length, row.count])));
	let shownCuts = $derived(CUTS.filter((cut) => (cutCounts.get(cut) ?? 0) > 0 || lenses.cut === cut));
	let active = $derived(activeLensCount(lenses));
	let eraSet = $derived(lenses.eraFrom != null || lenses.eraTo != null);

	function count(map: Map<string, number>, key: string): string {
		return facets ? chipCount(map.get(key) ?? 0) : '';
	}
</script>

<div class="lens-bar" role="group" aria-label="Archive lenses">
	<div class="lenses">
		<LensPopover
			label="Scene"
			value={lenses.scene != null ? (picked?.name ?? `Scene ${lenses.scene}`) : null}
			hue={picked ? hueColor(picked.hue) : null}
			width={560}
			onclear={() => onchange({ ...lenses, scene: null })}
		>
			{#snippet panel(close)}
				<input
					class="find"
					type="search"
					placeholder="Find a scene"
					aria-label="Find a scene"
					autocomplete="off"
					spellcheck="false"
					data-autofocus
					bind:value={sceneQuery}
				/>
				{#if facets === null}
					<p class="note">Counting scenes...</p>
				{:else if scenes.length === 0}
					<p class="note">No scene by that name</p>
				{:else}
					<div class="options two">
						{#each scenes as scene (scene.id)}
							<button
								type="button"
								class="option"
								class:zero={scene.count === 0 && lenses.scene !== scene.id}
								aria-pressed={lenses.scene === scene.id}
								onclick={() => {
									onchange(toggleLens(lenses, 'scene', scene.id));
									close();
								}}
							>
								<span class="dot" style:background={hueColor(scene.hue)}></span>
								<span class="name">{scene.name}</span>
								<span class="count">{chipCount(scene.count)}</span>
							</button>
						{/each}
					</div>
				{/if}
			{/snippet}
		</LensPopover>

		<LensPopover
			label="Era"
			value={eraSet ? eraLabel(lenses.eraFrom, lenses.eraTo) : null}
			width={460}
			onclear={() => onchange({ ...lenses, eraFrom: null, eraTo: null })}
		>
			{#snippet panel()}
				<EraBars
					facet={facets?.decade ?? []}
					from={lenses.eraFrom}
					to={lenses.eraTo}
					onchange={(eraFrom, eraTo) => onchange({ ...lenses, eraFrom, eraTo })}
				/>
			{/snippet}
		</LensPopover>

		<LensPopover
			label="Cut"
			value={lenses.cut ? CUT_LABELS[lenses.cut] : null}
			width={260}
			onclear={() => onchange({ ...lenses, cut: null })}
		>
			{#snippet panel(close)}
				<div class="options">
					{#each shownCuts as cut (cut)}
						<button
							type="button"
							class="option"
							aria-pressed={lenses.cut === cut}
							data-autofocus={lenses.cut === cut || undefined}
							onclick={() => {
								onchange(toggleLens(lenses, 'cut', cut));
								close();
							}}
						>
							<span class="name">{CUT_LABELS[cut]}</span>
							<span class="count">{chipCount(cutCounts.get(cut) ?? 0)}</span>
						</button>
					{/each}
				</div>
			{/snippet}
		</LensPopover>

		<LensPopover
			label="Depth"
			value={lenses.depth ? (DEPTHS.find((d) => d.value === lenses.depth)?.label ?? null) : null}
			width={240}
			onclear={() => onchange({ ...lenses, depth: null })}
		>
			{#snippet panel(close)}
				<div class="options">
					{#each DEPTHS as option (option.value)}
						<button
							type="button"
							class="option"
							class:zero={facets !== null && (depthCounts.get(option.value) ?? 0) === 0 && lenses.depth !== option.value}
							aria-pressed={lenses.depth === option.value}
							onclick={() => {
								onchange(toggleLens(lenses, 'depth', option.value));
								close();
							}}
						>
							<span class="name">{option.label}</span>
							<span class="count">{count(depthCounts, option.value)}</span>
						</button>
					{/each}
				</div>
			{/snippet}
		</LensPopover>

		<LensPopover
			label="Length"
			value={lenses.length ? (LENGTHS.find((l) => l.value === lenses.length)?.label ?? null) : null}
			width={240}
			onclear={() => onchange({ ...lenses, length: null })}
		>
			{#snippet panel(close)}
				<div class="options">
					{#each LENGTHS as option (option.value)}
						<button
							type="button"
							class="option"
							class:zero={facets !== null && (lengthCounts.get(option.value) ?? 0) === 0 && lenses.length !== option.value}
							aria-pressed={lenses.length === option.value}
							onclick={() => {
								onchange(toggleLens(lenses, 'length', option.value));
								close();
							}}
						>
							<span class="name">{option.label}</span>
							<span class="count">{count(lengthCounts, option.value)}</span>
						</button>
					{/each}
				</div>
			{/snippet}
		</LensPopover>

		{#if active > 1}
			<button type="button" class="clear" onclick={() => onchange(clearLenses(lenses))}>Clear all</button>
		{/if}
	</div>

	<div class="tools">
		<span class="total" aria-live="polite">
			{#if total !== null}{total.toLocaleString()} {total === 1 ? 'song' : 'songs'}{/if}
		</span>
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

<style>
	/* One line on the ground, no box: lenses left, count and order right.
	   It wraps into two lines only when the window is narrow. */
	.lens-bar {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-3) var(--space-4);
		flex-wrap: wrap;
		min-width: 0;
	}

	.lenses,
	.tools {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		flex-wrap: wrap;
		min-width: 0;
	}

	.total {
		margin-right: var(--space-2);
		color: var(--text-tertiary);
		font-size: var(--font-size-sm);
		font-variant-numeric: tabular-nums;
		white-space: nowrap;
	}

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

	.clear:hover {
		background: var(--bg-hover);
	}

	.clear:focus-visible,
	.option:focus-visible {
		outline: 2px solid var(--accent-strong);
		outline-offset: -2px;
	}

	.find:focus {
		outline: none;
		border-color: var(--accent-line);
	}

	/* Panel bodies (rendered into the lens panel). */
	.find {
		width: 100%;
		height: 34px;
		margin-bottom: 8px;
		padding: 0 12px;
		border: 1px solid var(--border-subtle);
		border-radius: 999px;
		background: var(--bg-surface);
		color: var(--text-primary);
		font: inherit;
		font-size: var(--font-size-sm);
	}

	.options {
		display: grid;
		gap: 1px;
	}

	.options.two {
		grid-template-columns: repeat(2, minmax(0, 1fr));
		column-gap: 6px;
	}

	.option {
		display: flex;
		align-items: center;
		gap: 9px;
		min-width: 0;
		padding: 7px 10px;
		border: 0;
		border-radius: 8px;
		background: none;
		color: var(--text-secondary);
		font-family: inherit;
		font-size: var(--font-size-sm);
		text-align: left;
		cursor: pointer;
	}

	.option:hover {
		background: var(--bg-hover);
		color: var(--text-primary);
	}

	.option[aria-pressed='true'] {
		background: var(--accent-soft);
		color: var(--accent-strong);
		font-weight: var(--font-weight-semibold);
	}

	.option.zero {
		opacity: 0.45;
	}

	.dot {
		width: 7px;
		height: 7px;
		border-radius: 50%;
		flex: 0 0 auto;
		opacity: 0.85;
	}

	.name {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.count {
		flex: none;
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
		font-variant-numeric: tabular-nums;
	}

	.option[aria-pressed='true'] .count {
		color: inherit;
		opacity: 0.8;
	}

	.note {
		margin: 0;
		padding: 10px;
		color: var(--text-tertiary);
		font-size: var(--font-size-sm);
	}

	@media (max-width: 560px) {
		.options.two {
			grid-template-columns: minmax(0, 1fr);
		}
	}
</style>
