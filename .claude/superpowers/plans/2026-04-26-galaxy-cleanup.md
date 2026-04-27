# Galaxy Cleanup & Panel Simplification Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Strip the galaxy toolbar to Search + Labels + Auto drift, remove Bridges/co-listening entirely, and replace the verbose genre panel with a compact layout that shows track count + listen time in the header, nearby chips, and a collapsible track list.

**Architecture:** Three files are touched: `+page.svelte` loses 7 toolbar controls, several dead state variables, and the co-occurrence fetch pipeline. `galaxyBuilder.ts` loses the `buildCoListeningEdges` function and its call site. `GenrePanel.svelte` loses the Lineage/Momentum blocks and gains a subtitle line plus a `showTracks` toggle.

**Tech Stack:** Svelte 5 (runes), SvelteKit, TypeScript. No new dependencies. Type-check with `cd frontend && npx svelte-check --tsconfig ./tsconfig.json`. Visual check with `cd frontend && npx vite dev`.

---

### Task 1: Strip toolbar to three controls

**Files:**
- Modify: `frontend/src/routes/genres/+page.svelte:637-678`

- [ ] **Step 1: Replace the control-dock contents**

In `+page.svelte`, find the `.control-dock` div (line 637) and replace everything inside it with just search + Labels + Auto drift:

```svelte
<div class="control-dock glass-panel">
	<form class="search-shell" onsubmit={(event) => void handleSearchSubmit(event)}>
		<input
			type="search"
			bind:value={searchQuery}
			placeholder="Search genres"
			aria-label="Search mapped genres"
		/>
	</form>

	<button class:active={labelsEnabled} class="dock-btn" onclick={() => (labelsEnabled = !labelsEnabled)}>
		Labels
	</button>
	<button class:active={autoDrift} class="dock-btn" onclick={() => (autoDrift = !autoDrift)}>
		Auto drift
	</button>
</div>
```

- [ ] **Step 2: Type-check**

```
cd frontend && npx svelte-check --tsconfig ./tsconfig.json 2>&1 | tail -5
```

Expected: errors about unused `handleJumpToFamily`, `handleBackOut`, `listeningDriven`, `showCohorts`, `showCoListening`, `heatEnabled` (we fix these in Tasks 2–3). Zero errors about the toolbar markup itself.

- [ ] **Step 3: Commit**

```bash
git add frontend/src/routes/genres/+page.svelte
git commit -m "feat(galaxy): strip toolbar to search, labels, auto-drift"
```

---

### Task 2: Remove dead state, functions, and HUD stats from `+page.svelte`

**Files:**
- Modify: `frontend/src/routes/genres/+page.svelte`

- [ ] **Step 1: Remove dead state declarations**

Remove these four lines (around lines 38–40):

```svelte
let listeningDriven = $state(false);
let showCohorts = $state(true);
let showCoListening = $state(true);
```

Change `heatEnabled` initial value from `true` to `false`:

```svelte
let heatEnabled = $state(false);
```

Remove the `coOccurrences` state (line 16):

```svelte
let coOccurrences = $state<GenreCoOccurrence[]>([]);
```

- [ ] **Step 2: Update the `buildGalaxyData` derived call**

Find the `galaxyData` derived block (lines 48–57) and replace:

```svelte
let galaxyData = $derived(
	taxonomy.length > 0
		? buildGalaxyData(taxonomy, heat, {
				coOccurrences: showCoListening ? coOccurrences : [],
				cohorts: showCohorts ? cohorts : [],
				evolution,
				listeningDriven
			})
		: { nodes: [], edges: [] }
);
```

With:

```svelte
let galaxyData = $derived(
	taxonomy.length > 0
		? buildGalaxyData(taxonomy, heat, {
				cohorts: [],
				evolution
			})
		: { nodes: [], edges: [] }
);
```

- [ ] **Step 3: Remove dead derived variables**

Delete these three derived blocks (around lines 83–93):

```svelte
let liveBridgeCount = $derived(
	rootStats.filter((root) => root.listens > 0).length > 1
		? rootStats.filter((root) => root.listens > 0).length - 1
		: 0
);
let coListeningBridgeCount = $derived(
	coOccurrences.filter((p) => p.jaccard > 0.1).length
);
let activeCohortCount = $derived(
	cohorts.filter((c) => c.genre_ids.length > 0).length
);
```

- [ ] **Step 4: Simplify `activeModeCopy`**

Replace the entire `activeModeCopy` derived block (lines 94–121) with a version that does not branch on `listeningDriven`:

```svelte
let activeModeCopy = $derived.by(() => {
	switch (viewMode) {
		case 'constellations': return 'Editorial scene clusters and orbit guides.';
		case 'mood': return 'Emotional field overlay across the taxonomy.';
		case 'heat': return 'Momentum halos reflecting recent listening.';
		case 'paths': return 'Lineage routes and bridge emphasis.';
		default: return 'Canonical galaxy map of your library.';
	}
});
```

- [ ] **Step 5: Remove `handleJumpToFamily` and `handleBackOut`**

Delete both functions (around lines 425–437):

```svelte
async function handleJumpToFamily(familyId: number) {
	if (!familyId) return;
	handleSelect(familyId);
	await focusNode(familyId);
}

function handleBackOut() {
	if (selectedId !== null) {
		handleSelect(null);
		return;
	}
	resetViewToken += 1;
}
```

- [ ] **Step 6: Remove `GenreCoOccurrence` from the import**

Line 4 — remove `GenreCoOccurrence` from the destructured import:

```svelte
import { api, type Genre, type GenreHeat, type GenreCohort, type GenreEvolutionPoint, type Track } from '$lib/api/client';
```

- [ ] **Step 7: Update `GalaxySnapshot` type**

Remove the `coOccurrences` field from the local `GalaxySnapshot` type (around line 172):

```ts
type GalaxySnapshot = {
	genres: Genre[];
	heat: GenreHeat[];
	cohorts: GenreCohort[];
	evolution: GenreEvolutionPoint[];
};
```

- [ ] **Step 8: Update `fetchGalaxySnapshot`**

Remove the `coOccurrencesResp` entry from the `Promise.allSettled` call and from the return object:

```svelte
async function fetchGalaxySnapshot(): Promise<GalaxySnapshot> {
	const genreResponse = await api.getGenres();
	const heatResponse = await api.getGenreHeat(90).catch((reason) => {
		if (isNotFoundError(reason)) {
			console.warn('Genre heat endpoint unavailable; rendering galaxy with zero heat.');
			return { heat: buildZeroHeat(genreResponse.genres) };
		}
		throw reason;
	});

	const [cohortsResp, evolutionResp] = await Promise.allSettled([
		api.getGenreCohorts(90),
		api.getGenreEvolution(90)
	]);

	return {
		genres: genreResponse.genres,
		heat: heatResponse.heat,
		cohorts: cohortsResp.status === 'fulfilled' ? cohortsResp.value.cohorts : [],
		evolution: evolutionResp.status === 'fulfilled' ? evolutionResp.value.evolution : []
	};
}
```

- [ ] **Step 9: Update `applyGalaxySnapshot`**

Remove the `coOccurrences = snapshot.coOccurrences;` assignment (around line 222). Keep the rest as-is.

- [ ] **Step 10: Update `refreshHeat`**

Remove the `coOccurrencesResp` entry from `Promise.allSettled` and its assignment:

```svelte
async function refreshHeat() {
	refreshingHeat = true;
	try {
		const [heatResp, cohortResp, evolResp] = await Promise.allSettled([
			api.getGenreHeat(90),
			api.getGenreCohorts(90),
			api.getGenreEvolution(90)
		]);
		if (heatResp.status === 'fulfilled') heat = heatResp.value.heat;
		else if (isNotFoundError(heatResp.reason)) heat = buildZeroHeat(taxonomy);
		if (cohortResp.status === 'fulfilled') cohorts = cohortResp.value.cohorts;
		if (evolResp.status === 'fulfilled') evolution = evolResp.value.evolution;
	} catch (reason) {
		console.error('Failed to refresh galaxy data', reason);
	} finally {
		refreshingHeat = false;
	}
}
```

- [ ] **Step 11: Simplify HUD stats**

Find the `hud-stats` block (around lines 605–616) and replace the conditional with a flat list:

```svelte
<div class="hud-stats compact">
	<div><strong>{taxonomy.length}</strong><span>families</span></div>
	<div><strong>{galaxyData.nodes.length}</strong><span>mapped</span></div>
	<div><strong>{activeThisMonthCount}</strong><span>active</span></div>
	<div><strong>{rediscoveryCount}</strong><span>rediscovery</span></div>
</div>
```

- [ ] **Step 12: Type-check**

```
cd frontend && npx svelte-check --tsconfig ./tsconfig.json 2>&1 | tail -5
```

Expected: 0 errors.

- [ ] **Step 13: Commit**

```bash
git add frontend/src/routes/genres/+page.svelte
git commit -m "feat(galaxy): remove co-listening, gravity, clusters state + HUD cleanup"
```

---

### Task 3: Remove co-listening edge logic from `galaxyBuilder.ts`

**Files:**
- Modify: `frontend/src/lib/components/Genre/galaxyBuilder.ts`

- [ ] **Step 1: Remove `buildCoListeningEdges` function**

Delete the entire function and its JSDoc comment (lines ~176–207):

```ts
/**
 * Build co-listening edges from backend co-occurrence data.
 */
function buildCoListeningEdges(
	nodes: GalaxyNode[],
	coOccurrences: GenreCoOccurrence[]
): GalaxyEdge[] {
	// ... entire body
}
```

- [ ] **Step 2: Remove co-listening block from `buildGalaxyData`**

Delete these lines (around lines 336–340):

```ts
// Phase 2: Add co-listening edges (emergent cross-genre bridges)
if (coOccurrences.length > 0) {
	const coEdges = buildCoListeningEdges(nodes, coOccurrences);
	edges.push(...coEdges);
}
```

- [ ] **Step 3: Remove `coOccurrences` from `buildGalaxyData` options**

In the options parameter type and destructuring (around lines 210–224), remove the `coOccurrences` entry:

```ts
export function buildGalaxyData(
	genres: Genre[],
	heat: GenreHeat[],
	options: {
		cohorts?: GenreCohort[];
		evolution?: GenreEvolutionPoint[];
		metrics?: GenreAudioMetrics[];
		listeningDriven?: boolean;
	} = {}
): GalaxyData {
```

And in the destructure line:

```ts
const { cohorts = [], evolution = [], metrics = [], listeningDriven = false } = options;
```

- [ ] **Step 4: Remove `GenreCoOccurrence` import**

Find the import at the top of `galaxyBuilder.ts` that includes `GenreCoOccurrence` and remove just that type from it.

- [ ] **Step 5: Type-check**

```
cd frontend && npx svelte-check --tsconfig ./tsconfig.json 2>&1 | tail -5
```

Expected: 0 errors.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/lib/components/Genre/galaxyBuilder.ts
git commit -m "feat(galaxy): remove buildCoListeningEdges + co-occurrence pipeline"
```

---

### Task 4: Simplify `GenrePanel.svelte` template

**Files:**
- Modify: `frontend/src/lib/components/Genre/GenrePanel.svelte`

- [ ] **Step 1: Add `showTracks` state and remove `visibleTracks`**

In the script block, after line 62, add:

```ts
let showTracks = $state(false);
```

Delete the `visibleTracks` derived (line 61):

```ts
let visibleTracks = $derived(tracks.slice(0, 20));
```

- [ ] **Step 2: Replace the description `<p>` with a subtitle**

Find the `panel-copy` div (lines 67–76). Replace the `<p>` description:

```svelte
<p>{node.depth === 0 ? 'Primary anchor in the live taxonomy atlas.' : `Depth ${node.depth} branch in the live taxonomy atlas.`}</p>
```

With:

```svelte
<p class="panel-subtitle">{node.trackCount.toLocaleString()} tracks{listenedTime > 0 ? ` · ${formatListenTime(listenedTime)}` : ''}</p>
```

- [ ] **Step 3: Remove `panel-action-copy`**

Delete this line from the `panel-actions` block (line 86):

```svelte
<p class="panel-action-copy">Queue replaces with this branch, then genre shuffle + automix take over.</p>
```

- [ ] **Step 4: Remove the breadcrumb row**

Delete the entire `{#if lineage.length > 0}` block (lines 89–98):

```svelte
{#if lineage.length > 0}
	<div class="breadcrumb-row" aria-label="Lineage">
		{#each lineage as step, index}
			<span class="breadcrumb-chip">{step}</span>
			{#if index < lineage.length - 1}
				<span class="breadcrumb-sep">/</span>
			{/if}
		{/each}
	</div>
{/if}
```

- [ ] **Step 5: Remove the `signal-grid` block**

Delete the entire `<div class="signal-grid">` block (lines 100–132) containing both the Lineage and Momentum `signal-block` sections.

- [ ] **Step 6: Remove the "Nearby scenes" label**

In the `nearby-block` (lines 134–143), delete just the label line:

```svelte
<p class="signal-label">Nearby scenes</p>
```

Keep the `<div class="nearby-chips">` and its `{#each}` loop.

- [ ] **Step 7: Replace the `track-section` with a toggle pattern**

Delete the entire existing `<div class="track-section">` block (lines 145–190) and replace it with:

```svelte
<div class="track-section">
	<button class="tracks-toggle" onclick={() => (showTracks = !showTracks)}>
		{showTracks ? '▲ Hide tracks' : `See all ${node.trackCount.toLocaleString()} tracks ▼`}
	</button>
	{#if showTracks}
		{#if loading}
			<EmptyState title="Loading tracks" copy={`Pulling ${node.name} tracks for the panel.`} />
		{:else if error}
			<EmptyState title="Tracks could not load" copy={error} />
		{:else if tracks.length === 0}
			<EmptyState title="No tracks in this branch" copy="This node does not currently resolve to any playable tracks." />
		{:else}
			<div class="track-list">
				{#each tracks as track (track.id)}
					<div
						class="track-row"
						role="button"
						tabindex="0"
						onclick={() => void handleTrackPlay(track.id)}
						onkeydown={(event) => runOnActivation(event, () => void handleTrackPlay(track.id))}
					>
						<div class="track-main">
							<strong>{track.title}</strong>
							<p>
								{track.artist_name ?? 'Unknown artist'}
								{#if track.album_title}
									<span> · {track.album_title}</span>
								{/if}
							</p>
						</div>
						<div class="track-side">
							{#if track.best_quality}
								<span class={`quality-badge ${getQualityClass(track.best_quality)}`}>
									{track.best_quality.replaceAll('_', ' ')}
								</span>
							{/if}
							<span>{formatDuration(track.duration_ms)}</span>
							<button class="queue-btn" onclick={(event) => void handleQueueTrack(track.id, event)}>+</button>
						</div>
					</div>
				{/each}
			</div>
		{/if}
	{/if}
</div>
```

- [ ] **Step 8: Type-check**

```
cd frontend && npx svelte-check --tsconfig ./tsconfig.json 2>&1 | tail -5
```

Expected: 0 errors.

- [ ] **Step 9: Commit**

```bash
git add frontend/src/lib/components/Genre/GenrePanel.svelte
git commit -m "feat(panel): compact layout — subtitle, remove lineage/momentum, track toggle"
```

---

### Task 5: Update `GenrePanel.svelte` styles

**Files:**
- Modify: `frontend/src/lib/components/Genre/GenrePanel.svelte` (style block)

- [ ] **Step 1: Remove dead CSS rules**

Delete these rule blocks from the `<style>` section (they are no longer referenced in the template):

- `.breadcrumb-row` (the full block)
- `.breadcrumb-chip, .nearby-chip` — change to just `.nearby-chip` (keep it, remove `breadcrumb-chip` from the selector)
- `.breadcrumb-sep`
- `.signal-grid`
- `.signal-block`
- `.signal-label`
- `.signal-row`
- `.signal-row span`
- `.signal-row strong`
- `.track-heading` and `.track-heading h3`
- `.panel-copy p` — this was styling the old description paragraph; change the selector to `.panel-subtitle` so it targets the new subtitle:

  Change:
  ```css
  .panel-copy p,
  .family-name,
  .track-heading span,
  .panel-action-copy,
  .track-main p,
  .track-side span {
  	color: var(--signal-text);
  }
  ```

  To:
  ```css
  .panel-subtitle,
  .family-name,
  .track-main p,
  .track-side span {
  	color: var(--signal-text);
  }
  ```

- [ ] **Step 2: Add `.tracks-toggle` style and cap `.track-list` height**

Add after the `.panel-actions` block:

```css
.tracks-toggle {
	width: 100%;
	background: color-mix(in srgb, var(--instrument-surface) 84%, transparent);
	border: 1px solid color-mix(in srgb, var(--instrument-border) 58%, transparent);
	border-radius: var(--radius);
	padding: 8px 12px;
	font-size: 0.78rem;
	color: var(--signal-text);
	text-align: left;
	cursor: pointer;
	transition:
		background var(--motion-fast),
		border-color var(--motion-fast);
}

.tracks-toggle:hover {
	background: color-mix(in srgb, var(--instrument-surface-strong) 88%, transparent);
	border-color: color-mix(in srgb, var(--instrument-border) 86%, transparent);
}
```

Update `.track-list` to cap height when expanded:

```css
.track-list {
	display: flex;
	flex-direction: column;
	gap: 8px;
	overflow-y: auto;
	max-height: 50vh;
	padding-right: 4px;
}
```

- [ ] **Step 3: Type-check**

```
cd frontend && npx svelte-check --tsconfig ./tsconfig.json 2>&1 | tail -5
```

Expected: 0 errors.

- [ ] **Step 4: Visual check**

Start the dev server (`cd frontend && npx vite dev`) and open the Galaxy page. Verify:

1. Toolbar shows only: search input, Labels button, Auto drift button
2. Clicking a genre node opens the panel showing: family badge, name, `N tracks · Xh Ym` subtitle, Start mix + Lock as seed, nearby chips
3. No Lineage block, no Momentum block, no breadcrumb row
4. "See all N tracks ▼" button is visible at the bottom of the panel
5. Clicking it expands the scrollable track list; clicking again collapses it

- [ ] **Step 5: Commit**

```bash
git add frontend/src/lib/components/Genre/GenrePanel.svelte
git commit -m "feat(panel): clean up dead styles, add tracks-toggle, cap track-list height"
```
