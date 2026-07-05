<script lang="ts">
	import { addTrackToQueue, playTrackNow } from '$lib/stores/player';
	import { formatTrackDuration, formatDuration, getQualityClass } from '$lib/utils/format';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { buildTrackMenu } from '$lib/player/track_menu';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import type { GenreHeat, Track } from '$lib/api/client';
	import type { GalaxyNode } from './galaxy.types';

	type NearbyEntry = { id: number; name: string };

	let {
		node = null,
		listenHeat = null,
		tracks = [],
		nearbyGenres = [],
		isSeed = false,
		loading = false,
		error = null,
		open = false,
		onClose = () => {},
		onMix = () => {},
		onToggleSeed = () => {},
		onOpenInterior = () => {},
		onSelectNearby = () => {}
	}: {
		node?: GalaxyNode | null;
		listenHeat?: GenreHeat | null;
		tracks?: Track[];
		nearbyGenres?: NearbyEntry[];
		isSeed?: boolean;
		loading?: boolean;
		error?: string | null;
		open?: boolean;
		onClose?: () => void;
		onMix?: () => void;
		onToggleSeed?: () => void;
		onOpenInterior?: () => void;
		onSelectNearby?: (id: number) => void;
	} = $props();

	function handleTrackContextMenu(event: MouseEvent, track: Track) {
		openContextMenu(event, buildTrackMenu(track));
	}

	function runOnActivation(event: KeyboardEvent, action: () => void) {
		if (event.key !== 'Enter' && event.key !== ' ') return;
		event.preventDefault();
		action();
	}

	async function handleTrackPlay(trackId: number) {
		await playTrackNow(trackId);
	}

	async function handleQueueTrack(trackId: number, event: MouseEvent) {
		event.stopPropagation();
		await addTrackToQueue(trackId);
	}

	let listenedTime = $derived(listenHeat?.total_listened_ms ?? node?.totalListenedMs ?? 0);
	let showTracks = $state(false);

	// The panel is a quick peek, not the browse surface - render only the first
	// slice. Genres resolve to thousands of tracks and dumping every row into the
	// DOM here froze the panel. Full searchable list lives in the interior.
	const PANEL_TRACK_CAP = 50;
	let shownPanelTracks = $derived(tracks.slice(0, PANEL_TRACK_CAP));

	// Top-3 artists derived from the panel's track sample.
	let topArtists = $derived.by(() => {
		const counts = new Map<string, number>();
		for (const track of tracks) {
			const name = track.artist_name?.trim();
			if (!name) continue;
			counts.set(name, (counts.get(name) ?? 0) + 1);
		}
		return [...counts.entries()]
			.sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
			.slice(0, 3)
			.map(([name]) => name);
	});

	// 12-period sparkline data, normalized to the local max for the slice.
	let sparklineData = $derived.by(() => {
		const history = node?.evolutionHistory ?? [];
		if (history.length < 2) return [] as { count: number; norm: number }[];
		const slice = history.slice(-12);
		const max = Math.max(...slice.map((p) => p.listenCount), 1);
		return slice.map((p) => ({ count: p.listenCount, norm: p.listenCount / max }));
	});

	let hasAudioSignature = $derived(
		node !== null && (node.avgBpm != null || node.avgEnergy != null || node.avgDanceability != null)
	);
</script>

<div class:open class="genre-panel glass-panel" style={node ? `--genre-accent: ${node.color}` : ''}>
	{#if node}
		<div class="panel-headline">
			<div class="panel-copy identity">
				<span class="family-row">
					<span class="family-dot"></span>
					<span class="family-name">{node.familyName}</span>
				</span>
				<h2>{node.name}</h2>
				<p class="panel-subtitle">
					<span>{node.trackCount.toLocaleString()} tracks</span>
					{#if listenedTime > 0}<span class="sep">·</span><span>{formatDuration(listenedTime)} played</span>{/if}
				</p>
			</div>
			<button class="close-btn" onclick={onClose} aria-label="Close genre panel">×</button>
		</div>

		<div class="panel-actions">
			<button class="mix-hero" onclick={onMix}>
				<span class="mix-hero-icon">▶</span>
				<span class="mix-hero-copy">
					<strong>Start mix</strong>
					<small>Radio seeded from {node.name}</small>
				</span>
			</button>
			<div class="secondary-actions">
				<button class={`ghost-btn ${isSeed ? 'is-seed' : ''}`} onclick={onToggleSeed}>
					{isSeed ? 'Seed locked' : 'Lock as seed'}
				</button>
				<button class="ghost-btn" onclick={onOpenInterior}>Open interior</button>
			</div>
		</div>

		{#if topArtists.length > 0}
			<div class="meta-row">
				<span class="meta-label">Top artists</span>
				<div class="meta-values">
					{#each topArtists as artist}
						<span class="meta-pill">{artist}</span>
					{/each}
				</div>
			</div>
		{/if}

		{#if hasAudioSignature && node}
			<div class="meta-row">
				<span class="meta-label">Vibe</span>
				<div class="meta-values">
					{#if node.avgBpm != null}
						<span class="meta-pill"><strong>{Math.round(node.avgBpm)}</strong> BPM</span>
					{/if}
					{#if node.avgEnergy != null}
						<span class="meta-pill"><strong>{node.avgEnergy.toFixed(2)}</strong> energy</span>
					{/if}
					{#if node.avgDanceability != null}
						<span class="meta-pill"><strong>{node.avgDanceability.toFixed(2)}</strong> dance</span>
					{/if}
				</div>
			</div>
		{/if}

		{#if sparklineData.length > 0}
			<div class="meta-row sparkline-row">
				<span class="meta-label">Trend</span>
				<div class="sparkline" aria-label="12-period listen trend">
					{#each sparklineData as bar}
						<span
							class="sparkline-bar"
							style={`height: ${Math.max(3, bar.norm * 100)}%; opacity: ${0.35 + bar.norm * 0.65}`}
						></span>
					{/each}
				</div>
			</div>
		{/if}

		{#if nearbyGenres.length > 0}
			<div class="nearby-block">
				<div class="nearby-chips">
					{#each nearbyGenres as nearby}
						<button
							type="button"
							class="nearby-chip"
							onclick={() => onSelectNearby(nearby.id)}
						>
							{nearby.name}
						</button>
					{/each}
				</div>
			</div>
		{/if}

		<div class="track-section">
			<button class="tracks-toggle" onclick={() => (showTracks = !showTracks)}>
				{showTracks ? '▲ Hide tracks' : `Preview tracks (${node.trackCount.toLocaleString()}) ▼`}
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
						{#each shownPanelTracks as track (track.id)}
							<div
								class="track-row"
								role="button"
								tabindex="0"
								onclick={() => void handleTrackPlay(track.id)}
								onkeydown={(event) => runOnActivation(event, () => void handleTrackPlay(track.id))}
								oncontextmenu={(event) => handleTrackContextMenu(event, track)}
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
									<span>{formatTrackDuration(track.duration_ms)}</span>
									<button class="queue-btn" onclick={(event) => void handleQueueTrack(track.id, event)}>+</button>
								</div>
							</div>
						{/each}
					</div>
					{#if tracks.length > PANEL_TRACK_CAP}
						<button class="browse-all" onclick={onOpenInterior}>
							Open interior to browse &amp; search all {tracks.length.toLocaleString()} tracks
						</button>
					{/if}
				{/if}
			{/if}
		</div>
	{/if}
</div>

<style>
	.genre-panel {
		position: absolute;
		top: 20px;
		right: 20px;
		width: min(360px, calc(100% - 40px));
		max-height: calc(100% - 40px);
		padding: 20px;
		display: flex;
		flex-direction: column;
		gap: 14px;
		transform: translateX(120%);
		opacity: 0;
		pointer-events: none;
		transition:
			transform 280ms cubic-bezier(0.22, 1, 0.36, 1),
			opacity var(--motion-base);
		z-index: 6;
		/* Panel themes to the selected genre via --genre-accent: a soft top-right
		   bloom of the genre's own color over the neutral instrument surface. */
		background:
			radial-gradient(135% 80% at 100% -10%, color-mix(in srgb, var(--genre-accent, transparent) 22%, transparent), transparent 55%),
			linear-gradient(180deg, color-mix(in srgb, var(--instrument-surface-strong) 92%, transparent), color-mix(in srgb, var(--instrument-surface) 86%, transparent)),
			var(--panel-bg);
		border-color: color-mix(in srgb, var(--genre-accent, var(--instrument-border)) 40%, var(--instrument-border));
		box-shadow:
			0 22px 54px rgba(0, 0, 0, 0.52),
			inset 0 1px 0 color-mix(in srgb, var(--instrument-edge) 62%, transparent);
	}

	.genre-panel.open {
		transform: translateX(0);
		opacity: 1;
		pointer-events: auto;
	}

	.panel-headline,
	.panel-copy {
		display: flex;
		flex-direction: column;
	}

	.panel-headline {
		flex-direction: row;
		align-items: flex-start;
		justify-content: space-between;
		gap: 16px;
	}

	.panel-copy {
		gap: 6px;
		min-width: 0;
	}

	.panel-copy h2 {
		font-size: var(--font-size-2xl);
		line-height: var(--line-height-tight);
		letter-spacing: -0.02em;
	}

	.panel-subtitle,
	.family-name,
	.track-main p,
	.track-side span {
		color: var(--signal-text);
	}

	.panel-subtitle {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		font-variant-numeric: tabular-nums;
	}

	.panel-subtitle .sep {
		opacity: 0.5;
	}

	.family-row {
		display: inline-flex;
		align-items: center;
		gap: 7px;
		font-size: var(--font-size-2xs);
		text-transform: uppercase;
		letter-spacing: 0.12em;
		font-weight: var(--font-weight-semibold);
		color: color-mix(in srgb, var(--genre-accent, var(--signal-text)) 72%, var(--text-primary));
	}

	.family-dot {
		width: 9px;
		height: 9px;
		border-radius: 50%;
		background: var(--genre-accent, var(--signal-text));
		box-shadow: 0 0 12px color-mix(in srgb, var(--genre-accent, transparent) 80%, transparent);
	}

	.close-btn {
		flex-shrink: 0;
		width: 36px;
		height: 36px;
		border-radius: 999px;
		background: color-mix(in srgb, var(--instrument-surface) 88%, transparent);
		border: 1px solid color-mix(in srgb, var(--instrument-border) 64%, transparent);
		font-size: var(--font-size-xl);
		line-height: 1;
		transition: border-color var(--motion-fast), background var(--motion-fast);
	}

	.close-btn:hover {
		background: color-mix(in srgb, var(--instrument-surface-strong) 90%, transparent);
		border-color: color-mix(in srgb, var(--instrument-border) 86%, transparent);
	}

	.panel-actions {
		display: flex;
		flex-direction: column;
		gap: 8px;
	}

	/* Start mix is the panel's hero: full-width, themed to the genre, with the
	   secondary actions demoted to a quiet ghost row below it. */
	.mix-hero {
		display: flex;
		align-items: center;
		gap: 12px;
		width: 100%;
		padding: 11px 14px;
		border-radius: var(--radius);
		text-align: left;
		cursor: pointer;
		color: var(--text-primary);
		border: 1px solid color-mix(in srgb, var(--genre-accent, var(--accent-line)) 62%, transparent);
		background:
			linear-gradient(135deg, color-mix(in srgb, var(--genre-accent, var(--accent)) 34%, transparent), color-mix(in srgb, var(--genre-accent, var(--accent)) 12%, transparent)),
			color-mix(in srgb, var(--instrument-surface-strong) 74%, transparent);
		box-shadow: 0 10px 26px color-mix(in srgb, var(--genre-accent, transparent) 24%, transparent);
		transition:
			transform var(--motion-fast),
			box-shadow var(--motion-fast),
			border-color var(--motion-fast);
	}

	.mix-hero:hover {
		transform: translateY(-1px);
		border-color: color-mix(in srgb, var(--genre-accent, var(--accent-line)) 92%, transparent);
		box-shadow: 0 14px 34px color-mix(in srgb, var(--genre-accent, transparent) 40%, transparent);
	}

	.mix-hero-icon {
		flex-shrink: 0;
		width: 40px;
		height: 40px;
		border-radius: 999px;
		display: grid;
		place-items: center;
		font-size: var(--font-size-sm);
		color: #06070d;
		background: color-mix(in srgb, var(--genre-accent, var(--accent)) 82%, #ffffff 12%);
		box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.45);
	}

	.mix-hero-copy {
		display: flex;
		flex-direction: column;
		gap: 1px;
		min-width: 0;
	}

	.mix-hero-copy strong {
		font-size: var(--font-size-md);
		font-weight: var(--font-weight-bold);
	}

	.mix-hero-copy small {
		font-size: var(--font-size-2xs);
		color: var(--signal-text);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.secondary-actions {
		display: flex;
		gap: 8px;
	}

	.ghost-btn {
		flex: 1;
		padding: 8px 12px;
		border-radius: var(--radius);
		border: 1px solid color-mix(in srgb, var(--instrument-border) 55%, transparent);
		background: color-mix(in srgb, var(--instrument-surface) 70%, transparent);
		color: var(--signal-text);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-medium);
		cursor: pointer;
		transition:
			background var(--motion-fast),
			border-color var(--motion-fast),
			color var(--motion-fast);
	}

	.ghost-btn:hover {
		color: var(--text-primary);
		background: color-mix(in srgb, var(--instrument-surface-strong) 84%, transparent);
		border-color: color-mix(in srgb, var(--instrument-border) 84%, transparent);
	}

	.ghost-btn.is-seed {
		color: var(--text-primary);
		border-color: color-mix(in srgb, var(--genre-accent, var(--accent-line)) 82%, transparent);
		background: color-mix(in srgb, var(--genre-accent, var(--accent-soft)) 24%, var(--instrument-surface));
		box-shadow: 0 0 16px color-mix(in srgb, var(--genre-accent, transparent) 34%, transparent);
	}

	.tracks-toggle {
		width: 100%;
		background: color-mix(in srgb, var(--instrument-surface) 84%, transparent);
		border: 1px solid color-mix(in srgb, var(--instrument-border) 58%, transparent);
		border-radius: var(--radius);
		padding: 8px 12px;
		font-size: var(--font-size-xs);
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

	.browse-all {
		width: 100%;
		padding: 9px 12px;
		border-radius: var(--radius);
		border: 1px dashed color-mix(in srgb, var(--instrument-border) 60%, transparent);
		background: transparent;
		color: var(--signal-text);
		font-size: var(--font-size-xs);
		cursor: pointer;
		transition: color var(--motion-fast), border-color var(--motion-fast);
	}

	.browse-all:hover {
		color: var(--text-primary);
		border-color: color-mix(in srgb, var(--genre-accent, var(--accent-line)) 70%, transparent);
	}

	.nearby-chip {
		padding: 4px 9px;
		border-radius: 999px;
		font-size: var(--font-size-2xs);
		letter-spacing: 0.04em;
		background: color-mix(in srgb, var(--instrument-surface-strong) 88%, transparent);
		border: 1px solid color-mix(in srgb, var(--instrument-border) 50%, transparent);
		color: var(--signal-text);
		cursor: pointer;
		transition:
			background var(--motion-fast),
			border-color var(--motion-fast),
			color var(--motion-fast),
			transform var(--motion-fast);
	}

	.nearby-chip:hover {
		background: color-mix(in srgb, var(--accent-soft) 60%, var(--instrument-surface-strong));
		border-color: color-mix(in srgb, var(--accent-line) 80%, transparent);
		color: var(--text-primary);
		transform: translateY(-1px);
	}

	.nearby-block {
		display: flex;
		flex-direction: column;
		gap: 8px;
	}

	.nearby-chips {
		display: flex;
		flex-wrap: wrap;
		gap: 8px;
	}

	.meta-row {
		display: flex;
		flex-direction: column;
		gap: 6px;
	}

	.meta-label {
		font-size: var(--font-size-2xs);
		text-transform: uppercase;
		letter-spacing: 0.1em;
		color: var(--signal-text);
		font-weight: var(--font-weight-semibold);
	}

	.meta-values {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
	}

	.meta-pill {
		padding: 3px 9px;
		border-radius: 999px;
		font-size: var(--font-size-xs);
		background: color-mix(in srgb, var(--instrument-surface) 76%, transparent);
		border: 1px solid color-mix(in srgb, var(--instrument-border) 42%, transparent);
		color: var(--text-primary);
		font-variant-numeric: tabular-nums;
	}

	.meta-pill strong {
		font-weight: var(--font-weight-bold);
		margin-right: 2px;
	}

	.sparkline-row {
		flex-direction: row;
		align-items: center;
		gap: 10px;
	}

	.sparkline {
		display: flex;
		align-items: flex-end;
		gap: 2px;
		flex: 1;
		height: 28px;
	}

	.sparkline-bar {
		flex: 1;
		min-width: 2px;
		background: color-mix(in srgb, var(--accent-line) 80%, transparent);
		border-radius: 2px 2px 0 0;
	}

	.track-section {
		display: flex;
		flex-direction: column;
		gap: 12px;
		min-height: 0;
	}

	.track-list {
		display: flex;
		flex-direction: column;
		gap: 8px;
		overflow-y: auto;
		max-height: 50vh;
		padding-right: 4px;
	}

	.track-row {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 12px;
		padding: 11px 12px;
		border-radius: var(--radius);
		background: color-mix(in srgb, var(--instrument-surface) 82%, transparent);
		border: 1px solid color-mix(in srgb, var(--instrument-border) 56%, transparent);
		transition:
			background var(--motion-fast),
			border-color var(--motion-fast),
			transform var(--motion-fast);
	}

	.track-row:hover {
		border-color: color-mix(in srgb, var(--instrument-border) 86%, transparent);
		background: color-mix(in srgb, var(--instrument-surface-strong) 88%, transparent);
		transform: translateY(-1px);
	}

	.track-main,
	.track-side {
		display: flex;
		align-items: center;
		gap: 10px;
	}

	.track-main {
		min-width: 0;
		flex: 1;
		flex-direction: column;
		align-items: flex-start;
		gap: 4px;
	}

	.track-main strong,
	.track-main p {
		max-width: 100%;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.track-side {
		flex-shrink: 0;
		flex-direction: column;
		align-items: flex-end;
		gap: 6px;
	}

	.queue-btn {
		width: 28px;
		height: 28px;
		border-radius: 999px;
		border: 1px solid color-mix(in srgb, var(--instrument-border) 64%, transparent);
		background: color-mix(in srgb, var(--accent-soft) 88%, transparent);
		color: var(--accent-strong);
	}

	@media (max-width: 1180px) {
		.genre-panel {
			top: auto;
			bottom: 16px;
			right: 16px;
			left: 16px;
			width: auto;
			max-height: min(62vh, 560px);
		}
	}

	@media (max-width: 760px) {
		.genre-panel {
			position: relative;
			inset: auto;
			width: 100%;
			max-height: none;
			margin-top: 0;
			transform: translateY(18px);
		}

		.genre-panel.open {
			transform: translateY(0);
		}

		.track-list {
			padding-right: 0;
		}

		.track-row {
			align-items: flex-start;
		}

		.track-side {
			align-items: flex-end;
		}
	}
</style>
