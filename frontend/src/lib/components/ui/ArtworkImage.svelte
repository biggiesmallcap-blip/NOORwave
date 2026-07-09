<script lang="ts">
	import {
		tidalArtworkFallbackSizes,
		upscaleTidalArtwork,
		type TidalArtworkSize
	} from '$lib/utils/artwork';

	type Props = {
		src?: string | string[] | null;
		alt?: string;
		size?: TidalArtworkSize;
		className?: string;
		fallbackText?: string;
		decorative?: boolean;
		loading?: 'eager' | 'lazy';
		decoding?: 'async' | 'sync' | 'auto';
		fetchPriority?: 'high' | 'low' | 'auto';
		// When true, a missing-artwork fallback gets a stable per-name colour
		// instead of a flat grey box, so empty tiles read as intentional.
		tint?: boolean;
	};

	let {
		src = null,
		alt = '',
		size = 320,
		className = '',
		fallbackText = 'NOOR',
		decorative = false,
		loading = 'lazy',
		decoding = 'async',
		fetchPriority = 'auto',
		tint = false,
	}: Props = $props();

	// Deterministic hue from the label so the same album/artist always gets the
	// same colour across sessions and surfaces.
	function stableTone(name: string): string {
		let hash = 0;
		for (let i = 0; i < name.length; i++) hash = (hash * 31 + name.charCodeAt(i)) | 0;
		const hue = Math.abs(hash) % 360;
		return `hsl(${hue}, 42%, 30%)`;
	}
	const tintColor = $derived(tint ? stableTone(alt || fallbackText) : null);

	let failedAttempts = $state(0);
	let lastSrcKey = $state('');
	let lastSize = $state<TidalArtworkSize>();
	const sources = $derived(normalizeSources(src));
	const srcKey = $derived(sources.join('\n'));
	const attempts = $derived(
		sources.flatMap((source) =>
			tidalArtworkFallbackSizes(source, size).map((fallbackSize) => ({ source, size: fallbackSize }))
		)
	);
	const exhausted = $derived(failedAttempts >= attempts.length);
	const resolvedSrc = $derived(
		!exhausted
			? upscaleTidalArtwork(
					attempts[failedAttempts]?.source,
					attempts[failedAttempts]?.size ?? size,
				)
			: null
	);

	$effect(() => {
		if (srcKey === lastSrcKey && size === lastSize) return;
		lastSrcKey = srcKey;
		lastSize = size;
		failedAttempts = 0;
	});

	function normalizeSources(value: string | string[] | null | undefined): string[] {
		const values = Array.isArray(value) ? value : [value];
		return values
			.filter((candidate): candidate is string => typeof candidate === 'string' && candidate.trim().length > 0)
			.filter((candidate, index, list) => list.indexOf(candidate) === index);
	}
</script>

{#if resolvedSrc}
	<img
		class={className}
		src={resolvedSrc}
		alt={decorative ? '' : alt}
		loading={loading}
		decoding={decoding}
		fetchpriority={fetchPriority}
		onerror={() => {
			failedAttempts += 1;
		}}
	/>
{:else}
	<div
		class={`${className} fallback`}
		class:tinted={tintColor != null}
		role={decorative ? undefined : 'img'}
		aria-label={decorative ? undefined : alt}
		aria-hidden={decorative ? 'true' : undefined}
		style={tintColor ? `background:${tintColor}` : undefined}
	>
		<span style={tintColor ? 'color: rgba(255,255,255,0.92)' : undefined}>{fallbackText}</span>
	</div>
{/if}

<style>
	.top-art {
		width: 168px;
		height: 168px;
		border-radius: 8px;
		background: var(--bg-raised);
		box-shadow: 0 12px 28px -12px rgba(0, 0, 0, 0.6);
		object-fit: cover;
	}

	.top-art.fallback {
		display: flex;
		align-items: center;
		justify-content: center;
	}

	.top-art--circle { border-radius: 50%; }

	.top-art.fallback span {
		font-size: var(--font-size-4xl);
		color: rgba(255, 255, 255, 0.55);
		font-weight: var(--font-weight-semibold);
	}

	.top-art--circle.fallback span { font-size: var(--font-size-3xl); }

	:global(.top-result-card.has-hero-bg) .top-art {
		position: relative;
		z-index: 2;
	}

	.top-hero-bg {
		position: absolute;
		inset: -16px;
		z-index: 0;
		width: calc(100% + 32px);
		height: calc(100% + 32px);
		object-fit: cover;
		object-position: center;
		opacity: 0.72;
		filter: blur(12px) saturate(1.08) contrast(0.96);
		transform: scale(1.02);
	}

	.top-hero-bg.fallback {
		display: none;
	}

	:global(.top-result-card.artist-hero) .top-art--circle {
		position: relative;
		z-index: 2;
		width: 100px;
		height: 100px;
		filter: none;
		opacity: 1;
		border: 1px solid rgba(255, 255, 255, 0.2);
		box-shadow: 0 16px 34px -14px rgba(0, 0, 0, 0.75), 0 0 0 5px rgba(255, 255, 255, 0.05);
	}

	.artist-avatar {
		width: 72px;
		height: 72px;
		border-radius: 50%;
		background: var(--bg-raised);
		object-fit: cover;
		display: block;
		transition: opacity 0.15s;
	}

	:global(.artist-card:hover) .artist-avatar {
		opacity: 0.85;
	}

	.artist-avatar.fallback {
		display: flex;
		align-items: center;
		justify-content: center;
	}

	.artist-avatar.fallback span {
		font-family: var(--font-body, inherit);
		font-size: var(--font-size-xl);
		font-weight: var(--font-weight-semibold);
		color: rgba(255, 255, 255, 0.78);
		letter-spacing: 0.02em;
	}

	.album-art {
		width: 128px;
		height: 128px;
		border-radius: 6px;
		background: var(--bg-raised);
		margin-bottom: 7px;
		transition: opacity 0.15s;
		object-fit: cover;
		display: block;
	}

	.album-art.fallback {
		display: flex;
		align-items: center;
		justify-content: center;
	}

	.album-art.fallback span {
		font-size: var(--font-size-xl);
		color: rgba(255, 255, 255, 0.62);
		font-weight: var(--font-weight-semibold);
	}

	:global(.album-card:hover) .album-art { opacity: 0.85; }

	:global(.section-grid-albums) .album-art {
		width: 100%;
		height: 100%;
		aspect-ratio: 1 / 1;
	}

	.track-art {
		width: 36px;
		height: 36px;
		border-radius: 4px;
		background: var(--bg-raised);
		flex-shrink: 0;
		object-fit: cover;
		display: block;
	}

	.track-art.fallback {
		display: flex;
		align-items: center;
		justify-content: center;
	}

	.track-art.fallback span {
		font-size: var(--font-size-sm);
		color: rgba(255, 255, 255, 0.62);
		font-weight: var(--font-weight-semibold);
	}

	.row-art {
		width: 32px;
		height: 32px;
		border-radius: 3px;
		background: var(--bg-raised);
		flex-shrink: 0;
		object-fit: cover;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		font-size: var(--font-size-sm);
		color: rgba(255, 255, 255, 0.62);
	}

	.row-art.fallback span {
		font-weight: var(--font-weight-semibold);
	}

	.row-art--circle { border-radius: 50%; }

	.cell-art-thumb {
		width: 36px;
		height: 36px;
		border-radius: 4px;
		background: var(--bg-raised);
		flex-shrink: 0;
		object-fit: cover;
		display: flex;
		align-items: center;
		justify-content: center;
	}

	.cell-art-thumb.fallback span {
		font-size: var(--font-size-sm);
		color: rgba(255, 255, 255, 0.62);
		font-weight: var(--font-weight-semibold);
	}

	.art {
		width: 100%;
		height: 100%;
		object-fit: cover;
		display: block;
	}

	.art.fallback {
		display: grid;
		place-items: center;
		color: var(--text-tertiary);
		background: var(--bg-surface);
	}

	.art.fallback span {
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
	}
</style>
