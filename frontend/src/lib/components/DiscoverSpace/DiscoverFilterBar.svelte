<script lang="ts">
	import { discoverSpaceStore, setCoherence, setFilters } from './discover_space_store';
	import { isFilterNoop, type DiscoverFilters } from './discover_space_types';

	let expanded = $state(false);

	let filters = $derived($discoverSpaceStore.filters);
	let coherence = $derived($discoverSpaceStore.coherence);
	let activeCount = $derived(countActive($discoverSpaceStore.filters));
	let eraCoverage = $derived($discoverSpaceStore.lastDiagnostics?.era_filter_coverage ?? null);
	let filterDropped = $derived($discoverSpaceStore.lastDiagnostics?.filter_dropped_count ?? 0);

	function countActive(f: DiscoverFilters): number {
		let n = 0;
		if (f.bpm_min != null || f.bpm_max != null) n++;
		if (f.energy_min != null || f.energy_max != null) n++;
		if (f.key_compatible_only) n++;
		if (f.year_min != null || f.year_max != null) n++;
		if (f.exclude_in_library) n++;
		if (f.exclude_heard_session) n++;
		return n;
	}

	function patch(update: Partial<DiscoverFilters>): void {
		setFilters({ ...filters, ...update });
	}

	function numOrNull(value: string): number | null {
		const trimmed = value.trim();
		if (trimmed === '') return null;
		const parsed = Number(trimmed);
		return Number.isFinite(parsed) ? parsed : null;
	}

	function clearAll(): void {
		setFilters({});
	}
</script>

<div class="filter-bar" class:expanded>
	<div class="filter-row">
		<button
			class="toggle-btn"
			onclick={() => (expanded = !expanded)}
			aria-expanded={expanded}
		>
			Filters{activeCount > 0 ? ` (${activeCount})` : ''}
		</button>
		<div class="coherence-wrap" title="Left hugs the seed, right explores outward">
			<span class="coherence-label">Familiar</span>
			<input
				class="coherence-slider"
				type="range"
				min="0"
				max="1"
				step="0.05"
				value={1 - coherence}
				oninput={(e) => setCoherence(1 - Number(e.currentTarget.value))}
				aria-label="Coherence versus diversity"
			/>
			<span class="coherence-label">Adventurous</span>
		</div>
		{#if filterDropped > 0}
			<span class="drop-note">{filterDropped} hidden</span>
		{/if}
	</div>

	{#if expanded}
		<div class="filter-grid">
			<label class="field">
				<span>BPM</span>
				<span class="range-pair">
					<input
						type="number"
						placeholder="min"
						value={filters.bpm_min ?? ''}
						onchange={(e) => patch({ bpm_min: numOrNull(e.currentTarget.value) })}
					/>
					<input
						type="number"
						placeholder="max"
						value={filters.bpm_max ?? ''}
						onchange={(e) => patch({ bpm_max: numOrNull(e.currentTarget.value) })}
					/>
				</span>
			</label>
			<label class="field">
				<span>Energy</span>
				<span class="range-pair">
					<input
						type="number"
						min="0"
						max="1"
						step="0.1"
						placeholder="min"
						value={filters.energy_min ?? ''}
						onchange={(e) => patch({ energy_min: numOrNull(e.currentTarget.value) })}
					/>
					<input
						type="number"
						min="0"
						max="1"
						step="0.1"
						placeholder="max"
						value={filters.energy_max ?? ''}
						onchange={(e) => patch({ energy_max: numOrNull(e.currentTarget.value) })}
					/>
				</span>
			</label>
			<label class="field">
				<span>Era</span>
				<span class="range-pair">
					<input
						type="number"
						placeholder="from"
						value={filters.year_min ?? ''}
						onchange={(e) => patch({ year_min: numOrNull(e.currentTarget.value) })}
					/>
					<input
						type="number"
						placeholder="to"
						value={filters.year_max ?? ''}
						onchange={(e) => patch({ year_max: numOrNull(e.currentTarget.value) })}
					/>
				</span>
			</label>
			<label class="check">
				<input
					type="checkbox"
					checked={filters.key_compatible_only ?? false}
					onchange={(e) => patch({ key_compatible_only: e.currentTarget.checked })}
				/>
				<span>Compatible keys only</span>
				<span class="hint">hides unanalyzed and external tracks</span>
			</label>
			<label class="check">
				<input
					type="checkbox"
					checked={filters.exclude_in_library ?? false}
					onchange={(e) => patch({ exclude_in_library: e.currentTarget.checked })}
				/>
				<span>New to me only</span>
			</label>
			<label class="check">
				<input
					type="checkbox"
					checked={filters.exclude_heard_session ?? false}
					onchange={(e) => patch({ exclude_heard_session: e.currentTarget.checked })}
				/>
				<span>Skip heard this session</span>
			</label>
			{#if (filters.year_min != null || filters.year_max != null) && eraCoverage !== null && eraCoverage < 0.5}
				<div class="hint era-hint">
					Era data is sparse here ({Math.round(eraCoverage * 100)}% of results have a year);
					tracks without one stay visible.
				</div>
			{/if}
			{#if !isFilterNoop(filters)}
				<button class="clear-btn" onclick={clearAll}>Clear filters</button>
			{/if}
		</div>
	{/if}
</div>

<style>
	.filter-bar {
		display: flex;
		flex-direction: column;
		gap: 6px;
		background: rgba(0, 0, 0, 0.5);
		backdrop-filter: var(--blur-base);
		-webkit-backdrop-filter: var(--blur-base);
		border: 1px solid var(--panel-border);
		border-radius: 12px;
		padding: 6px 10px;
		max-width: 460px;
	}
	.filter-row {
		display: flex;
		align-items: center;
		gap: 10px;
	}
	.toggle-btn {
		padding: 4px 12px;
		border-radius: 999px;
		border: none;
		background: transparent;
		color: rgba(255, 255, 255, 0.7);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-medium);
		cursor: pointer;
		white-space: nowrap;
	}
	.toggle-btn:hover {
		color: rgba(255, 255, 255, 0.95);
	}
	.coherence-wrap {
		display: flex;
		align-items: center;
		gap: 6px;
		flex: 1;
		min-width: 0;
	}
	.coherence-label {
		font-size: var(--font-size-xs);
		color: rgba(255, 255, 255, 0.45);
		white-space: nowrap;
	}
	.coherence-slider {
		flex: 1;
		min-width: 60px;
		accent-color: rgba(124, 128, 255, 0.9);
	}
	.drop-note {
		font-size: var(--font-size-xs);
		color: rgba(255, 220, 120, 0.8);
		white-space: nowrap;
	}
	.filter-grid {
		display: flex;
		flex-wrap: wrap;
		gap: 8px 14px;
		padding-bottom: 4px;
	}
	.field {
		display: flex;
		align-items: center;
		gap: 6px;
		font-size: var(--font-size-xs);
		color: rgba(255, 255, 255, 0.65);
	}
	.range-pair {
		display: flex;
		gap: 4px;
	}
	.field input[type='number'] {
		width: 58px;
		padding: 3px 6px;
		border-radius: 6px;
		border: 1px solid var(--panel-border);
		background: rgba(255, 255, 255, 0.06);
		color: rgba(255, 255, 255, 0.9);
		font-size: var(--font-size-xs);
	}
	.check {
		display: flex;
		align-items: center;
		gap: 6px;
		font-size: var(--font-size-xs);
		color: rgba(255, 255, 255, 0.65);
		cursor: pointer;
	}
	.hint {
		color: rgba(255, 255, 255, 0.35);
	}
	.era-hint {
		flex-basis: 100%;
		font-size: var(--font-size-xs);
	}
	.clear-btn {
		padding: 3px 10px;
		border-radius: 999px;
		border: 1px solid var(--panel-border);
		background: transparent;
		color: rgba(255, 255, 255, 0.6);
		font-size: var(--font-size-xs);
		cursor: pointer;
	}
	.clear-btn:hover {
		color: rgba(255, 255, 255, 0.9);
	}
</style>
