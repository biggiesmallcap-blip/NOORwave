<script lang="ts">
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import { lazyTidalArt, type LazyTidalArtKind } from '$lib/actions/lazy-tidal-art';

	export type ChartMuralAccent = 'accent' | 'lastfm';

	export type ChartMuralItem = {
		id: string;
		title: string;
		subtitle: string;
		artwork: string | null;
		fallbackText: string;
		tileLabel: string;
		tileTitle: string;
		lazy?: {
			enabled: boolean;
			kind?: LazyTidalArtKind;
			query: {
				artist: string | null;
				title: string;
			};
			onResolve: (url: string) => void;
		};
	};

	type Props = {
		items?: ChartMuralItem[];
		currentIndex?: number;
		ariaLabel: string;
		title: string;
		subtitle: string;
		metric?: string;
		actionLabel?: string;
		actionDisabled?: boolean;
		accent?: ChartMuralAccent;
		loading?: boolean;
		loadingLabel?: string;
		onSelect?: (index: number) => void;
		onJump?: (delta: number) => void;
		onPlay?: () => void | Promise<void>;
		onItemActivate?: (index: number) => void | Promise<void>;
		onCardContext?: (event: MouseEvent) => void | Promise<void>;
		onItemContext?: (event: MouseEvent, index: number) => void | Promise<void>;
		onPauseChange?: (paused: boolean) => void;
	};

	let {
		items = [],
		currentIndex = 0,
		ariaLabel,
		title,
		subtitle,
		metric = '',
		actionLabel = 'Play',
		actionDisabled = false,
		accent = 'accent',
		loading = false,
		loadingLabel = 'Loading chart mural',
		onSelect = () => {},
		onJump = () => {},
		onPlay = () => {},
		onItemActivate,
		onCardContext,
		onItemContext,
		onPauseChange,
	}: Props = $props();

	let currentItem = $derived(items[currentIndex] ?? items[0] ?? null);

	function muralLayoutClass(count: number): string {
		if (count <= 1) return 'layout-count-1';
		if (count <= 2) return 'layout-count-2';
		if (count <= 3) return 'layout-count-3';
		if (count <= 4) return 'layout-count-4';
		if (count <= 6) return 'layout-count-6';
		if (count <= 8) return 'layout-count-8';
		if (count <= 10) return 'layout-count-10';
		if (count <= 12) return 'layout-count-12';
		if (count <= 15) return 'layout-count-15';
		if (count <= 16) return 'layout-count-16';
		return 'layout-count-20';
	}
</script>

{#if loading}
	<div class="chart-mural-loading">{loadingLabel}</div>
{:else if currentItem}
	<div
		class="chart-mural"
		class:accent-lastfm={accent === 'lastfm'}
		onmouseenter={() => onPauseChange?.(true)}
		onmouseleave={() => onPauseChange?.(false)}
		role="region"
		aria-label={ariaLabel}
		oncontextmenu={(event) => {
			if (onCardContext) void onCardContext(event);
		}}
	>
		<div class={`chart-mural-bg ${muralLayoutClass(items.length)}`} aria-hidden="true">
			{#each items as item, index (item.id)}
				<button
					class="chart-mural-tile"
					class:featured={currentItem.id === item.id}
					type="button"
					onclick={() => onSelect(index)}
					ondblclick={() => {
						if (onItemActivate) void onItemActivate(index);
					}}
					oncontextmenu={(event) => {
						if (onItemContext) void onItemContext(event, index);
					}}
					aria-label={item.tileLabel}
					title={item.tileTitle}
					use:lazyTidalArt={{
						enabled: item.lazy?.enabled ?? false,
						kind: item.lazy?.kind,
						query: item.lazy?.query ?? { artist: null, title: item.title },
						onResolve: item.lazy?.onResolve ?? (() => {}),
					}}
				>
					<ArtworkImage
						src={item.artwork}
						size={320}
						className="chart-mural-art"
						fallbackText={item.fallbackText}
						fadeIn
						decorative
					/>
				</button>
			{/each}
		</div>
		<div class="chart-mural-shade"></div>
		<div class="chart-mural-content">
			<div class="chart-mural-meta">
				<h3 class="chart-mural-title">{title}</h3>
				<p class="chart-mural-sub">{subtitle}</p>
				<!-- A status that only repeats the button is dropped, so the pair
				     never says the same thing twice. It sits on its own quiet line
				     rather than beside the button. -->
				{#if metric && metric !== actionLabel}
					<p class="chart-mural-why">{metric}</p>
				{/if}
				<div class="chart-mural-actions">
					<button
						class="btn btn-primary chart-mural-play"
						type="button"
						disabled={actionDisabled}
						onclick={() => void onPlay()}
					>
						<svg viewBox="0 0 16 16" width="14" height="14" fill="currentColor" aria-hidden="true">
							<path d="M3 2.5l10 5.5-10 5.5V2.5z"/>
						</svg>
						{actionLabel}
					</button>
				</div>
			</div>
		</div>
		{#if items.length > 1}
			<!-- One pager in the top corner: previous, position, next. -->
			<div class="chart-pager">
				<button class="chart-nav" type="button" onclick={() => onJump(-1)} aria-label="Previous chart entry">
					<svg viewBox="0 0 24 24" aria-hidden="true"><path d="m14.5 5-7 7 7 7" /></svg>
				</button>
				<span class="chart-pager-count" aria-live="polite">{currentIndex + 1} / {items.length}</span>
				<button class="chart-nav" type="button" onclick={() => onJump(1)} aria-label="Next chart entry">
					<svg viewBox="0 0 24 24" aria-hidden="true"><path d="m9.5 5 7 7-7 7" /></svg>
				</button>
			</div>
		{/if}
	</div>
{/if}

<style>
	.chart-mural {
		--chart-mural-accent: var(--accent);
		--chart-mural-soft: var(--accent-soft);
		position: relative;
		min-height: clamp(220px, 24vw, 360px);
		border-radius: var(--radius-md);
		overflow: hidden;
		/* clip-path, not just overflow: the tiles are composited layers
		   (fade-in, hover scale), and a rounded overflow clip let their bright
		   edges glow through the anti-aliased corners. */
		clip-path: inset(0 round var(--radius-md));
		isolation: isolate;
		background: var(--bg-base);
	}

	/* The edge is drawn above the art, so no tile can show around it at
	   the corners. */
	.chart-mural::after {
		content: '';
		position: absolute;
		inset: 0;
		z-index: var(--z-raised);
		border-radius: inherit;
		/* A dark inner edge, not a light hairline: a light line over dark
		   art read as a glowing rim, worst at the rounded corners. */
		box-shadow: inset 0 0 0 1px rgba(0, 0, 0, 0.45);
		pointer-events: none;
	}

	.chart-mural.accent-lastfm {
		--chart-mural-accent: var(--service-lastfm);
		--chart-mural-soft: color-mix(in srgb, var(--service-lastfm) 18%, var(--bg-surface));
	}

	.chart-mural-bg {
		position: absolute;
		inset: -7%;
		z-index: 0;
		display: grid;
		grid-template-columns: repeat(10, minmax(0, 1fr));
		grid-template-rows: repeat(2, minmax(0, 1fr));
		background: linear-gradient(120deg, var(--panel-bg), color-mix(in srgb, var(--chart-mural-accent) 16%, transparent));
	}

	.chart-mural-bg.layout-count-1 {
		grid-template-columns: minmax(0, 1fr);
		grid-template-rows: minmax(0, 1fr);
	}

	.chart-mural-bg.layout-count-2 {
		grid-template-columns: repeat(2, minmax(0, 1fr));
		grid-template-rows: minmax(0, 1fr);
	}

	.chart-mural-bg.layout-count-3 {
		grid-template-columns: repeat(3, minmax(0, 1fr));
		grid-template-rows: minmax(0, 1fr);
	}

	.chart-mural-bg.layout-count-4 {
		grid-template-columns: repeat(4, minmax(0, 1fr));
		grid-template-rows: minmax(0, 1fr);
	}

	.chart-mural-bg.layout-count-6 {
		grid-template-columns: repeat(3, minmax(0, 1fr));
		grid-template-rows: repeat(2, minmax(0, 1fr));
	}

	.chart-mural-bg.layout-count-8 {
		grid-template-columns: repeat(4, minmax(0, 1fr));
		grid-template-rows: repeat(2, minmax(0, 1fr));
	}

	.chart-mural-bg.layout-count-10 {
		grid-template-columns: repeat(5, minmax(0, 1fr));
		grid-template-rows: repeat(2, minmax(0, 1fr));
	}

	.chart-mural-bg.layout-count-12 {
		grid-template-columns: repeat(6, minmax(0, 1fr));
		grid-template-rows: repeat(2, minmax(0, 1fr));
	}

	.chart-mural-bg.layout-count-15 {
		grid-template-columns: repeat(5, minmax(0, 1fr));
		grid-template-rows: repeat(3, minmax(0, 1fr));
	}

	.chart-mural-bg.layout-count-16 {
		grid-template-columns: repeat(8, minmax(0, 1fr));
		grid-template-rows: repeat(2, minmax(0, 1fr));
	}

	/* 17-20 items. This matches the base grid, and until now the class had no
	   rule at all and worked only because it fell through to that base. Since
	   the recommendation panels ship exactly 20 items, that fall-through was the
	   common path, not an edge case - stating it means a future change to the
	   base grid cannot silently reshape it. */
	.chart-mural-bg.layout-count-20 {
		grid-template-columns: repeat(10, minmax(0, 1fr));
		grid-template-rows: repeat(2, minmax(0, 1fr));
	}

	.chart-mural-bg::after {
		content: '';
		position: absolute;
		inset: 0;
		background:
			radial-gradient(circle at 78% 42%, rgba(255,255,255,0.2), transparent 30%),
			linear-gradient(90deg, rgba(0,0,0,0.08), transparent 42%, rgba(0,0,0,0.04));
		pointer-events: none;
	}

	.chart-mural-tile {
		appearance: none;
		position: relative;
		min-width: 0;
		min-height: 0;
		padding: 0;
		border: 0;
		background: var(--bg-raised);
		color: var(--text-primary);
		cursor: pointer;
		overflow: hidden;
		opacity: 0.96;
		filter: saturate(1.18) brightness(1.14);
		transform: skewX(-7deg) scaleX(1.08);
		transform-origin: center;
		transition:
			filter var(--motion-fast),
			opacity var(--motion-fast),
			transform var(--motion-base),
			box-shadow var(--motion-base);
	}

	.chart-mural-tile::after {
		content: '';
		position: absolute;
		inset: 0;
		background: linear-gradient(90deg, rgba(0,0,0,0.18), transparent 48%, rgba(0,0,0,0.2));
		opacity: 0.18;
		pointer-events: none;
	}

	.chart-mural-tile:hover,
	.chart-mural-tile:focus-visible,
	.chart-mural-tile.featured {
		z-index: var(--z-raised);
		opacity: 1;
		filter: saturate(1.8) brightness(1.4);
		transform: skewX(-7deg) scaleX(1.08) scale(1.045);
		box-shadow:
			0 0 0 1px rgba(255,255,255,0.3),
			0 14px 30px rgba(0,0,0,0.32),
			0 0 24px color-mix(in srgb, var(--chart-mural-accent) 34%, transparent);
		outline: none;
	}

	:global(.chart-mural-art),
	:global(.chart-mural-art.fallback) {
		display: block;
		width: 100%;
		height: 100%;
	}

	:global(.chart-mural-art) {
		object-fit: cover;
		transform: skewX(7deg) scale(1.24);
		transition: transform var(--motion-base);
	}

	.chart-mural-tile:hover :global(.chart-mural-art),
	.chart-mural-tile:focus-visible :global(.chart-mural-art),
	.chart-mural-tile.featured :global(.chart-mural-art) {
		transform: skewX(7deg) scale(1.34);
	}

	:global(.chart-mural-art.fallback) {
		display: grid;
		place-items: center;
		background: linear-gradient(135deg, var(--bg-raised), var(--chart-mural-soft));
		color: rgba(255,255,255,0.78);
		font-size: var(--font-size-xl);
		font-weight: var(--font-weight-bold);
	}

	/* A deep plate behind the text that falls off smoothly into the collage,
	   plus a soft floor, so the copy reads without heavy text shadows. */
	.chart-mural-shade {
		position: absolute;
		inset: 0;
		z-index: var(--z-base);
		background:
			linear-gradient(90deg, rgba(8, 8, 12, 0.9) 0%, rgba(8, 8, 12, 0.76) 30%, rgba(8, 8, 12, 0.32) 56%, transparent 80%),
			linear-gradient(0deg, rgba(8, 8, 12, 0.45) 0%, transparent 45%);
		pointer-events: none;
	}

	/* Light mode: the app around the mural is bright, so the dark cinematic scrim
	 * reads as muddy. Lighten it (text still reads via its shadow) and push the
	 * collage saturation up so the artwork looks vivid instead of dimmed. */
	:global([data-theme="light"]) .chart-mural-shade {
		background:
			linear-gradient(90deg, rgba(8, 8, 12, 0.78) 0%, rgba(8, 8, 12, 0.6) 30%, rgba(8, 8, 12, 0.2) 56%, transparent 80%),
			linear-gradient(0deg, rgba(8, 8, 12, 0.32) 0%, transparent 45%);
	}

	:global([data-theme="light"]) .chart-mural-tile {
		opacity: 1;
		filter: saturate(1.36) brightness(1.08);
	}

	:global([data-theme="light"]) .chart-mural-tile::after {
		opacity: 0.1;
	}

	.chart-mural-content {
		position: relative;
		z-index: calc(var(--z-base) + 1);
		display: grid;
		align-items: center;
		min-height: inherit;
		padding: var(--space-6) var(--space-6);
		pointer-events: none;
	}

	.chart-mural-meta {
		display: flex;
		flex-direction: column;
		gap: var(--space-1);
		max-width: min(36rem, 52%);
		text-shadow: 0 1px 8px rgba(0, 0, 0, 0.45);
	}

	.chart-mural-title {
		margin: 0;
		color: #fff;
		font-size: var(--font-size-3xl);
		font-weight: var(--font-weight-bold);
		line-height: var(--line-height-tight);
		letter-spacing: -0.01em;
		/* Two lines at most: long titles wrap once, then end in an ellipsis. */
		overflow: hidden;
		display: -webkit-box;
		line-clamp: 2;
		-webkit-line-clamp: 2;
		-webkit-box-orient: vertical;
		overflow-wrap: anywhere;
	}

	.chart-mural-sub {
		margin: 0;
		color: rgba(255, 255, 255, 0.86);
		font-size: var(--font-size-md);
		font-weight: var(--font-weight-medium);
	}

	.chart-mural-why {
		margin: 0;
		color: rgba(255, 255, 255, 0.62);
		font-size: var(--font-size-xs);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.chart-mural-actions {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		margin-top: var(--space-4);
		pointer-events: auto;
	}

	.chart-mural-play {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
	}

	.chart-mural-play:disabled {
		cursor: not-allowed;
		opacity: 0.58;
	}

	/* Pager: solid discs and a position count, together in the top corner. */
	.chart-pager {
		position: absolute;
		top: var(--space-4);
		right: var(--space-4);
		z-index: var(--z-raised);
		display: flex;
		align-items: center;
		gap: var(--space-1);
		padding: 3px;
		border-radius: 999px;
		background: rgba(8, 8, 12, 0.62);
		box-shadow: 0 4px 16px rgba(0, 0, 0, 0.35);
	}

	.chart-pager-count {
		min-width: 3.5em;
		color: rgba(255, 255, 255, 0.86);
		font-size: var(--font-size-xs);
		font-variant-numeric: tabular-nums;
		text-align: center;
	}

	.chart-nav {
		display: grid;
		place-items: center;
		width: 30px;
		height: 30px;
		padding: 0;
		border: 0;
		border-radius: 50%;
		background: transparent;
		color: #fff;
		cursor: pointer;
		transition: background var(--motion-fast);
	}

	.chart-nav:hover {
		background: rgba(255, 255, 255, 0.14);
	}

	.chart-nav:focus-visible {
		outline: 2px solid var(--accent-strong);
		outline-offset: 1px;
	}

	.chart-nav svg {
		width: 16px;
		height: 16px;
		fill: none;
		stroke: currentColor;
		stroke-width: 2.4;
		stroke-linecap: round;
		stroke-linejoin: round;
	}

	.chart-mural-loading {
		display: grid;
		place-items: center;
		min-height: clamp(180px, 20vw, 280px);
		border: 1px solid var(--panel-border);
		border-radius: var(--radius-md);
		background: var(--panel-bg);
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
	}

	@media (max-width: 760px) {
		.chart-mural-bg {
			grid-template-columns: repeat(5, minmax(0, 1fr));
			grid-template-rows: repeat(4, minmax(0, 1fr));
		}

		.chart-mural-bg.layout-count-1 {
			grid-template-columns: minmax(0, 1fr);
			grid-template-rows: minmax(0, 1fr);
		}

		.chart-mural-bg.layout-count-2,
		.chart-mural-bg.layout-count-3 {
			grid-template-columns: repeat(2, minmax(0, 1fr));
			grid-template-rows: repeat(2, minmax(0, 1fr));
		}

		.chart-mural-bg.layout-count-4,
		.chart-mural-bg.layout-count-6 {
			grid-template-columns: repeat(3, minmax(0, 1fr));
			grid-template-rows: repeat(2, minmax(0, 1fr));
		}

		.chart-mural-bg.layout-count-8,
		.chart-mural-bg.layout-count-10 {
			grid-template-columns: repeat(5, minmax(0, 1fr));
			grid-template-rows: repeat(2, minmax(0, 1fr));
		}

		.chart-mural-bg.layout-count-12,
		.chart-mural-bg.layout-count-15 {
			grid-template-columns: repeat(4, minmax(0, 1fr));
			grid-template-rows: repeat(4, minmax(0, 1fr));
		}

		.chart-mural-content {
			padding: var(--space-5) var(--space-4);
		}

		.chart-mural-meta {
			max-width: 100%;
		}
	}
</style>
