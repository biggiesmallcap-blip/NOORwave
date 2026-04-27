# Discovery Phase 3a — Make Tracks Identifiable Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a hover tooltip showing track info, and make the seed track visually distinct (bigger, with a "Playing/Locked" pill and inline title).

**Architecture:** Three frontend-only files. New `DiscoverHoverCard.svelte` component renders a fixed-positioned tooltip outside the canvas. `DiscoverSpace.svelte` gains a `seedTrackId` prop, a seed-distinct render branch, and an `onHoverPosition` callback. `+page.svelte` wires hover state and renders the card alongside the existing canvas + panel layout.

**Tech Stack:** Svelte 5 (runes) + TypeScript. No backend changes. Type-check with `cd frontend && npx svelte-check --tsconfig ./tsconfig.json`.

> **Repo convention:** Do NOT add any `Co-Authored-By` trailer to commits.

---

## File responsibilities

| File | Role | Touched in |
|---|---|---|
| `frontend/src/lib/components/Discover/DiscoverHoverCard.svelte` | New — fixed tooltip rendered above canvas | T1 |
| `frontend/src/lib/components/Discover/DiscoverSpace.svelte` | Add `seedTrackId` prop, seed render branch, `onHoverPosition` callback | T2, T3 |
| `frontend/src/routes/discover/+page.svelte` | Wire hover state + render card | T4 |

---

### Task 1: Create `DiscoverHoverCard.svelte`

**Files:**
- Create: `frontend/src/lib/components/Discover/DiscoverHoverCard.svelte`

- [ ] **Step 1: Write the component**

Create the file with this content:

```svelte
<script lang="ts">
	import type { DiscoverTrackNode } from './discover.types';

	let {
		node = null,
		mouseX = 0,
		mouseY = 0,
		seedTrackId = null,
		isLocked = false,
	}: {
		node?: DiscoverTrackNode | null;
		mouseX?: number;
		mouseY?: number;
		seedTrackId?: number | null;
		isLocked?: boolean;
	} = $props();

	const CARD_WIDTH = 260;
	const CARD_OFFSET = 12;

	let cardEl: HTMLDivElement | null = $state(null);

	let isSeed = $derived(node !== null && node.track_id === seedTrackId);

	// Compute placement: above cursor by default, flip below if too close to top.
	// Anchor right edge if too close to viewport right.
	let placement = $derived.by(() => {
		if (!node) return { left: 0, top: 0, anchor: 'top-left' as const };
		const cardHeight = 130; // approx
		const vw = typeof window !== 'undefined' ? window.innerWidth : 1024;
		const vh = typeof window !== 'undefined' ? window.innerHeight : 768;

		let left = mouseX + CARD_OFFSET;
		let top = mouseY - cardHeight - CARD_OFFSET;

		if (top < 8) top = mouseY + CARD_OFFSET; // flip below
		if (left + CARD_WIDTH > vw - 8) left = mouseX - CARD_WIDTH - CARD_OFFSET; // flip left
		if (top + cardHeight > vh - 8) top = vh - cardHeight - 8;

		return { left, top, anchor: 'top-left' as const };
	});

	let chips = $derived.by(() => {
		if (!node) return [] as string[];
		const out: string[] = [];
		if (node.bpm != null) out.push(`${Math.round(node.bpm)} BPM`);
		if (node.camelot_key) out.push(node.camelot_key);
		else if (node.key_signature) out.push(node.key_signature);
		if (node.energy != null) out.push(`${Math.round(node.energy * 100)}% energy`);
		if (node.top_genre) out.push(node.top_genre);
		return out;
	});
</script>

{#if node}
	<div
		bind:this={cardEl}
		class="hover-card"
		style="left: {placement.left}px; top: {placement.top}px; width: {CARD_WIDTH}px"
	>
		{#if isSeed}
			<div class="seed-pill">
				{isLocked ? '🔒 Locked seed' : '▶ Playing'}
			</div>
		{:else if node.source === 'external'}
			<div class="source-tag">EXTERNAL · TIDAL</div>
		{/if}
		<div class="title">{node.title}</div>
		<div class="meta">
			{node.artist_name}{#if node.album_title}<span class="dot"> · </span>{node.album_title}{/if}
		</div>
		{#if chips.length > 0}
			<div class="chips">
				{#each chips as chip}
					<span class="chip">{chip}</span>
				{/each}
			</div>
		{/if}
	</div>
{/if}

<style>
	.hover-card {
		position: fixed;
		background: rgba(13, 13, 26, 0.95);
		backdrop-filter: blur(8px);
		border: 1px solid #3a3a5c;
		border-radius: 8px;
		padding: 12px 14px;
		z-index: 100;
		box-shadow: 0 12px 32px rgba(0, 0, 0, 0.5);
		pointer-events: none;
		font-family: inherit;
		color: #e8e8f0;
		animation: hover-card-fade-in 100ms ease-out;
	}

	@keyframes hover-card-fade-in {
		from { opacity: 0; transform: translateY(2px); }
		to { opacity: 1; transform: translateY(0); }
	}

	.seed-pill {
		display: inline-block;
		background: #5b4ef8;
		color: #fff;
		font-size: 9px;
		font-weight: 700;
		letter-spacing: 1px;
		padding: 3px 8px;
		border-radius: 999px;
		margin-bottom: 8px;
	}

	.source-tag {
		font-size: 10px;
		color: #5b4ef8;
		letter-spacing: 1px;
		font-weight: 600;
		margin-bottom: 6px;
	}

	.title {
		font-size: 14px;
		font-weight: 700;
		margin-bottom: 2px;
		line-height: 1.3;
	}

	.meta {
		font-size: 12px;
		color: #a0a0c0;
		margin-bottom: 10px;
		line-height: 1.4;
	}

	.dot {
		color: #5b5b7a;
	}

	.chips {
		display: flex;
		flex-wrap: wrap;
		gap: 5px;
	}

	.chip {
		background: #1e1e35;
		border: 1px solid #3a3a5c;
		border-radius: 999px;
		padding: 3px 9px;
		font-size: 10px;
		color: #c0c0d8;
	}
</style>
```

- [ ] **Step 2: Type-check**

Run: `cd E:/NOORwave/frontend && npx svelte-check --tsconfig ./tsconfig.json 2>&1 | tail -8`
Expected: 0 NEW errors. The component is unused so far, so an "unused export" warning may appear — that's fine; Task 4 wires it up.

- [ ] **Step 3: Commit**

```bash
git -C E:/NOORwave add frontend/src/lib/components/Discover/DiscoverHoverCard.svelte
git -C E:/NOORwave commit -m "feat(discover): DiscoverHoverCard component for hover tooltips"
```

---

### Task 2: Add `seedTrackId` prop + `onHoverPosition` callback to `DiscoverSpace.svelte`

**Files:**
- Modify: `frontend/src/lib/components/Discover/DiscoverSpace.svelte`

- [ ] **Step 1: Add new props**

Find the `let { ... } = $props();` block at the top of the script (around lines 8–26). The current props are: `nodes`, `artists`, `edges`, `mode`, `currentTrackId`, `onHover`, `onSelect`, `onNewNodes`.

Replace the props block with:

```svelte
	let {
		nodes = [],
		artists = [],
		edges = [],
		mode = 'radio',
		currentTrackId = null,
		seedTrackId = null,
		isSeedLocked = false,
		onHover = (_node: DiscoverTrackNode | null) => {},
		onHoverPosition = (_node: DiscoverTrackNode | null, _x: number, _y: number) => {},
		onSelect = (_node: DiscoverTrackNode) => {},
		onNewNodes = (_nodes: DiscoverTrackNode[]) => {},
	}: {
		nodes?: DiscoverTrackNode[];
		artists?: DiscoverArtistNode[];
		edges?: DiscoverEdge[];
		mode?: DiscoverViewMode;
		currentTrackId?: number | null;
		seedTrackId?: number | null;
		isSeedLocked?: boolean;
		onHover?: (node: DiscoverTrackNode | null) => void;
		onHoverPosition?: (node: DiscoverTrackNode | null, x: number, y: number) => void;
		onSelect?: (node: DiscoverTrackNode) => void;
		onNewNodes?: (nodes: DiscoverTrackNode[]) => void;
	} = $props();
```

- [ ] **Step 2: Update `onMouseMove` to fire `onHoverPosition`**

Find the `onMouseMove` function (around line 456). It currently sets `hoveredNode = found` and calls `onHover(found)`. Add the new callback after `onHover(found)`:

```svelte
	function onMouseMove(e: MouseEvent) {
		if (!canvas) return;
		const rect = canvas.getBoundingClientRect();
		const mx = (e.clientX - rect.left - canvas.offsetWidth / 2) / camera.zoom + camera.x;
		const my = (e.clientY - rect.top - canvas.offsetHeight / 2) / camera.zoom + camera.y;

		// Hover detection
		let found: DiscoverTrackNode | null = null;
		for (const node of nodes) {
			const dx = mx - node.x;
			const dy = my - node.y;
			if (dx * dx + dy * dy < node.radius * node.radius) {
				found = node;
				break;
			}
		}
		hoveredNode = found;
		onHover(found);
		onHoverPosition(found, e.clientX, e.clientY);
		if (canvas) canvas.style.cursor = found ? 'pointer' : 'grab';

		if (isDragging) {
			camera.x = cameraStart.x - (e.clientX - dragStart.x) / camera.zoom;
			camera.y = cameraStart.y - (e.clientY - dragStart.y) / camera.zoom;
		}
	}
```

- [ ] **Step 3: Type-check**

Run: `cd E:/NOORwave/frontend && npx svelte-check --tsconfig ./tsconfig.json 2>&1 | tail -8`
Expected: 0 errors.

- [ ] **Step 4: Commit**

```bash
git -C E:/NOORwave add frontend/src/lib/components/Discover/DiscoverSpace.svelte
git -C E:/NOORwave commit -m "feat(discover): add seedTrackId/isSeedLocked props + onHoverPosition callback"
```

---

### Task 3: Seed-distinct rendering in `DiscoverSpace.svelte`

**Files:**
- Modify: `frontend/src/lib/components/Discover/DiscoverSpace.svelte`

- [ ] **Step 1: Add a seed render branch in the node loop**

Inside `DiscoverSpace.svelte`, find the per-node render section (the for-loop drawing each node — around lines 290-370). After the existing per-node draw closes (around line 370 where the `}` ends the for-loop body) but BEFORE the "Currently playing node indicator" block (around line 372), add a new pass that draws the seed prominently:

```svelte
		// ── Seed-distinct rendering ──────────────────────────────────────
		if (seedTrackId != null) {
			const seedNode = nodes.find(n => n.track_id === seedTrackId);
			if (seedNode) {
				const t = (Date.now() % 3000) / 3000;
				const seedRadius = seedNode.radius * 1.5;

				// Big purple halo
				const haloR = seedRadius * 2.4;
				const haloGrad = ctx.createRadialGradient(seedNode.x, seedNode.y, 0, seedNode.x, seedNode.y, haloR);
				haloGrad.addColorStop(0, 'rgba(91, 78, 248, 0.4)');
				haloGrad.addColorStop(0.5, 'rgba(91, 78, 248, 0.15)');
				haloGrad.addColorStop(1, 'transparent');
				ctx.beginPath();
				ctx.arc(seedNode.x, seedNode.y, haloR, 0, Math.PI * 2);
				ctx.fillStyle = haloGrad;
				ctx.fill();

				// Heartbeat ring (slower than currentTrackId pulse)
				const heartbeatR = seedRadius + 8 + Math.sin(t * Math.PI * 2) * 3;
				ctx.beginPath();
				ctx.arc(seedNode.x, seedNode.y, heartbeatR, 0, Math.PI * 2);
				ctx.strokeStyle = 'rgba(91, 78, 248, 0.7)';
				ctx.lineWidth = 2;
				ctx.stroke();

				// Bright filled core overriding the regular node
				ctx.beginPath();
				ctx.arc(seedNode.x, seedNode.y, seedRadius, 0, Math.PI * 2);
				const coreGrad = ctx.createRadialGradient(seedNode.x, seedNode.y, 0, seedNode.x, seedNode.y, seedRadius);
				coreGrad.addColorStop(0, '#ffffff');
				coreGrad.addColorStop(0.4, '#a89cff');
				coreGrad.addColorStop(1, '#5b4ef8');
				ctx.fillStyle = coreGrad;
				ctx.fill();
				ctx.strokeStyle = '#ffffff';
				ctx.lineWidth = 2;
				ctx.stroke();

				// Inline label
				ctx.save();
				ctx.fillStyle = '#ffffff';
				ctx.font = '600 13px system-ui, sans-serif';
				ctx.textAlign = 'center';
				ctx.textBaseline = 'top';
				ctx.fillText(seedNode.title, seedNode.x, seedNode.y + seedRadius + 8);
				if (seedNode.artist_name) {
					ctx.fillStyle = 'rgba(160, 160, 192, 0.9)';
					ctx.font = '11px system-ui, sans-serif';
					ctx.fillText(seedNode.artist_name, seedNode.x, seedNode.y + seedRadius + 24);
				}
				// Lock indicator
				if (isSeedLocked) {
					ctx.fillStyle = '#5b4ef8';
					ctx.font = '14px system-ui, sans-serif';
					ctx.textAlign = 'left';
					ctx.fillText('🔒', seedNode.x + seedRadius * 0.7, seedNode.y - seedRadius);
				}
				ctx.restore();
			}
		}
```

- [ ] **Step 2: Type-check**

Run: `cd E:/NOORwave/frontend && npx svelte-check --tsconfig ./tsconfig.json 2>&1 | tail -8`
Expected: 0 errors.

- [ ] **Step 3: Commit**

```bash
git -C E:/NOORwave add frontend/src/lib/components/Discover/DiscoverSpace.svelte
git -C E:/NOORwave commit -m "feat(discover): seed-distinct rendering with halo + label + lock indicator"
```

---

### Task 4: Wire hover state + render `DiscoverHoverCard` in `+page.svelte`

**Files:**
- Modify: `frontend/src/routes/discover/+page.svelte`

- [ ] **Step 1: Add hover state and import the card**

Open `frontend/src/routes/discover/+page.svelte`. Find the existing import line for components (around line 6-11):

```svelte
	import DiscoverSpace from '$lib/components/Discover/DiscoverSpace.svelte';
	import DiscoverFilters from '$lib/components/Discover/DiscoverFilters.svelte';
	import DiscoverPanel from '$lib/components/Discover/DiscoverPanel.svelte';
	import PlaylistBuilder from '$lib/components/Discover/PlaylistBuilder.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
```

Add the new import:

```svelte
	import DiscoverHoverCard from '$lib/components/Discover/DiscoverHoverCard.svelte';
```

Find the existing state declarations in the script (after the imports). They include `selectedNodes`, `panelNode`, `searchQuery`, `isSearching`. Add new state for hover:

```svelte
	let hoveredNode = $state<DiscoverTrackNode | null>(null);
	let hoverX = $state(0);
	let hoverY = $state(0);
```

- [ ] **Step 2: Replace `handleHover` with the new wiring**

Find the existing `function handleHover(_node: DiscoverTrackNode | null) {}`. Replace it with:

```svelte
	function handleHover(node: DiscoverTrackNode | null) {
		hoveredNode = node;
	}

	function handleHoverPosition(node: DiscoverTrackNode | null, x: number, y: number) {
		hoveredNode = node;
		hoverX = x;
		hoverY = y;
	}
```

- [ ] **Step 3: Pass new props to `<DiscoverSpace>` and render the card**

Find the existing `<DiscoverSpace ... />` element in the template. The current props are roughly:

```svelte
				<DiscoverSpace
					nodes={$discoverSpace.nodes}
					edges={$discoverSpace.edges}
					mode={$discoverSpace.mode}
					currentTrackId={$currentTrack?.id ?? null}
					onHover={handleHover}
					onSelect={handleSelect}
					onNewNodes={handleNewNodes}
				/>
```

Replace with:

```svelte
				<DiscoverSpace
					nodes={$discoverSpace.nodes}
					edges={$discoverSpace.edges}
					mode={$discoverSpace.mode}
					currentTrackId={$currentTrack?.id ?? null}
					seedTrackId={$discoverSpace.activeSeedId}
					isSeedLocked={$discoverSpace.lockedSeedId !== null}
					onHover={handleHover}
					onHoverPosition={handleHoverPosition}
					onSelect={handleSelect}
					onNewNodes={handleNewNodes}
				/>
```

Then, immediately after the closing of the `<div class="discover-layout">` (the outer layout div that contains canvas + sidebars), add the hover card so it overlays everything:

Find the existing line `</div>` that closes the outer `discover-page` div (last element in the template before `<style>`). Right before that closing `</div>`, insert:

```svelte
	<DiscoverHoverCard
		node={hoveredNode}
		mouseX={hoverX}
		mouseY={hoverY}
		seedTrackId={$discoverSpace.activeSeedId}
		isLocked={$discoverSpace.lockedSeedId !== null}
	/>
```

- [ ] **Step 4: Type-check**

Run: `cd E:/NOORwave/frontend && npx svelte-check --tsconfig ./tsconfig.json 2>&1 | tail -8`
Expected: 0 NEW errors.

- [ ] **Step 5: Commit**

```bash
git -C E:/NOORwave add frontend/src/routes/discover/+page.svelte
git -C E:/NOORwave commit -m "feat(discover): wire hover state + render DiscoverHoverCard"
```

---

### Task 5: Smoke test

**Files:** none modified.

- [ ] **Step 1: Run dev server**

`cd E:/NOORwave/frontend && npx vite dev`. Open the discover page in a browser with a track playing.

- [ ] **Step 2: Visual checks**

1. The seed track is **bigger** than other nodes, with a purple halo, white-purple gradient core, slow heartbeat ring, and the title rendered in white text below it.
2. Hover any external node — a tooltip pops within ~100ms showing title, artist, album, and chips for available audio fields.
3. Hover the seed — tooltip shows a "▶ Playing" pill at the top.
4. Lock the seed via the Phase 2 pill — the seed gets a 🔒 lock icon, and tooltip pill changes to "🔒 Locked seed".
5. Hover near top of canvas — tooltip flips below the cursor.
6. Hover near right edge — tooltip flips to the left of the cursor.
7. Move quickly between nodes — no flicker, no stale data.

- [ ] **Step 3: No regression**

- Click-to-select still opens `DiscoverPanel` on the right.
- Drag-to-pan still works.
- Wheel zoom still works.
- Hyperspace search animation still works.
- Currently-playing pulse ring (the existing one) still appears on whatever is playing, in addition to the new seed visual.
