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

<div class:open class="genre-panel glass-panel">
	{#if node}
		<div class="panel-headline">
			<div class="panel-copy identity">
				<div class="family-row">
					<span class="family-dot" style={`--dot-color: ${node.color}`}></span>
					<span class="family-name">{node.familyName} system</span>
				</div>
				<h2>{node.name}</h2>
				<p class="panel-subtitle">{node.trackCount.toLocaleString()} tracks{listenedTime > 0 ? ` · ${formatDuration(listenedTime)}` : ''}</p>
			</div>
			<button class="close-btn" onclick={onClose} aria-label="Close genre panel">×</button>
		</div>

		<div class="panel-actions">
			<div class="panel-action-row">
				<button class="btn btn-primary" onclick={onMix}>▶ Start mix</button>
				<button class={`btn btn-glass ${isSeed ? 'is-seed' : ''}`} onclick={onToggleSeed}>
					{isSeed ? 'Seed locked' : 'Lock as seed'}
				</button>
				<button class="btn btn-glass" onclick={onOpenInterior}>Open interior</button>
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
		background:
			linear-gradient(180deg, color-mix(in srgb, var(--instrument-surface-strong) 90%, transparent), color-mix(in srgb, var(--instrument-surface) 84%, transparent)),
			var(--panel-bg);
		border-color: color-mix(in srgb, var(--instrument-border) 74%, transparent);
		box-shadow:
			0 18px 46px rgba(0, 0, 0, 0.5),
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
		font-size: 1.8rem;
		line-height: 1.05;
		letter-spacing: -0.02em;
	}

	.panel-subtitle,
	.family-name,
	.track-main p,
	.track-side span {
		color: var(--signal-text);
	}

	.family-row {
		display: inline-flex;
		align-items: center;
		gap: 8px;
		font-size: 0.8rem;
		text-transform: uppercase;
		letter-spacing: 0.08em;
	}

	.family-dot {
		width: 10px;
		height: 10px;
		border-radius: 50%;
		background: var(--dot-color);
		box-shadow: 0 0 0 6px color-mix(in srgb, var(--dot-color) 18%, transparent);
	}

	.close-btn {
		flex-shrink: 0;
		width: 36px;
		height: 36px;
		border-radius: 999px;
		background: color-mix(in srgb, var(--instrument-surface) 88%, transparent);
		border: 1px solid color-mix(in srgb, var(--instrument-border) 64%, transparent);
		font-size: 1.35rem;
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

	.panel-action-row {
		display: flex;
		flex-wrap: wrap;
		gap: 8px;
	}

	.panel-action-row :global(.btn.is-seed) {
		border-color: color-mix(in srgb, var(--accent-line) 90%, transparent);
		box-shadow: 0 0 18px color-mix(in srgb, var(--accent-glow) 45%, transparent);
	}

	.nearby-chip {
		padding: 4px 9px;
		border-radius: 999px;
		font-size: 0.7rem;
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
		font-size: 0.66rem;
		text-transform: uppercase;
		letter-spacing: 0.1em;
		color: var(--signal-text);
		font-weight: 600;
	}

	.meta-values {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
	}

	.meta-pill {
		padding: 3px 9px;
		border-radius: 999px;
		font-size: 0.74rem;
		background: color-mix(in srgb, var(--instrument-surface) 76%, transparent);
		border: 1px solid color-mix(in srgb, var(--instrument-border) 42%, transparent);
		color: var(--text-primary);
		font-variant-numeric: tabular-nums;
	}

	.meta-pill strong {
		font-weight: 700;
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
