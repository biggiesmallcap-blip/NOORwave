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
		kindLabel: string;
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
		kindLabel,
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
				<span class="chart-mural-kind">{kindLabel}</span>
				<h3 class="chart-mural-title">{title}</h3>
				<p class="chart-mural-sub">{subtitle}</p>
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
					{#if metric}
						<span>{metric}</span>
					{/if}
				</div>
			</div>
		</div>
		{#if items.length > 1}
			<button
				class="chart-nav chart-nav--prev"
				type="button"
				onclick={() => onJump(-1)}
				aria-label="Previous chart entry"
			>
				&lsaquo;
			</button>
			<button
				class="chart-nav chart-nav--next"
				type="button"
				onclick={() => onJump(1)}
				aria-label="Next chart entry"
			>
				&rsaquo;
			</button>
		{/if}
	</div>
{/if}

<style>
	.chart-mural {
		--chart-mural-accent: var(--accent);
		--chart-mural-soft: var(--accent-soft);
		position: relative;
		min-height: clamp(220px, 24vw, 360px);
		border: 1px solid var(--panel-border);
		border-radius: var(--radius-md);
		overflow: hidden;
		background: var(--panel-bg);
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

	.chart-mural-shade {
		position: absolute;
		inset: 0;
		z-index: var(--z-base);
		background: linear-gradient(90deg, rgba(0,0,0,0.72) 0%, rgba(0,0,0,0.36) 42%, rgba(0,0,0,0.08) 78%, transparent 100%);
		pointer-events: none;
	}

	/* Light mode: the app around the mural is bright, so the dark cinematic scrim
	 * reads as muddy. Lighten it (text still reads via its shadow) and push the
	 * collage saturation up so the artwork looks vivid instead of dimmed. */
	:global([data-theme="light"]) .chart-mural-shade {
		background: linear-gradient(90deg, rgba(0,0,0,0.52) 0%, rgba(0,0,0,0.24) 44%, rgba(0,0,0,0.05) 78%, transparent 100%);
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
		padding: var(--space-5);
		pointer-events: none;
	}

	.chart-mural-meta {
		display: flex;
		flex-direction: column;
		gap: var(--space-1);
		max-width: min(42rem, 58vw);
		/* Text sits over a dark-scrimmed art collage, so it stays light in both
		 * themes and leans on a strong shadow to read over bright album tiles. */
		text-shadow: 0 2px 16px rgba(0,0,0,0.8), 0 1px 4px rgba(0,0,0,0.6);
	}

	.chart-mural-kind {
		color: var(--chart-mural-accent);
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-semibold);
		letter-spacing: 0;
		text-transform: uppercase;
	}

	.chart-mural-title {
		margin: 0;
		color: #fff;
		font-size: var(--font-size-4xl);
		font-weight: var(--font-weight-bold);
		line-height: var(--line-height-tight);
		letter-spacing: 0;
		/* Two lines, not one. At --font-size-4xl a single nowrap line clipped
		   most real track titles mid-word ("Live and Learn [Extend..."), and the
		   mural has the vertical room for a second. */
		overflow: hidden;
		display: -webkit-box;
		line-clamp: 2;
		-webkit-line-clamp: 2;
		-webkit-box-orient: vertical;
		overflow-wrap: anywhere;
	}

	.chart-mural-sub {
		margin: 0 0 var(--space-2);
		color: rgba(255, 255, 255, 0.82);
		font-size: var(--font-size-sm);
	}

	.chart-mural-actions {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		pointer-events: auto;
	}

	.chart-mural-actions span {
		color: rgba(255, 255, 255, 0.82);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
	}

	.chart-mural-play {
		display: flex;
		align-items: center;
		gap: var(--space-1);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
	}

	.chart-mural-play:disabled {
		cursor: not-allowed;
		opacity: 0.58;
	}

	.chart-nav {
		position: absolute;
		right: var(--space-3);
		bottom: var(--space-4);
		z-index: var(--z-raised);
		display: grid;
		place-items: center;
		width: clamp(32px, 3vw, 40px);
		aspect-ratio: 1 / 1;
		border: 1px solid var(--panel-border);
		border-radius: 50%;
		background: rgba(0,0,0,0.5);
		color: var(--text-primary);
		cursor: pointer;
		font-size: var(--font-size-xl);
		line-height: 1;
		opacity: 0.78;
		transition: opacity var(--motion-fast), background var(--motion-fast);
	}

	.chart-mural:hover .chart-nav,
	.chart-nav:focus-visible {
		opacity: 1;
		outline: none;
	}

	.chart-nav:hover {
		background: rgba(0,0,0,0.75);
	}

	.chart-nav--prev {
		right: calc(var(--space-3) + clamp(32px, 3vw, 40px) + var(--space-2));
	}

	.chart-nav--next {
		right: var(--space-3);
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
			padding: var(--space-4);
		}

		.chart-mural-meta {
			max-width: 100%;
		}

		.chart-mural-title {
			font-size: var(--font-size-3xl);
		}
	}
</style>
