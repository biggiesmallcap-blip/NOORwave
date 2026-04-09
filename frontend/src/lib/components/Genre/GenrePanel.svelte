<script lang="ts">
	import { addTrackToQueue, playTrackNow } from '$lib/stores/player';
	import { formatDuration, getQualityClass } from '$lib/stores/library';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import type { GenreHeat, Track } from '$lib/api/client';
	import type { GalaxyNode } from './galaxy.types';

	let {
		node = null,
		listenHeat = null,
		tracks = [],
		lineage = [],
		nearbyGenres = [],
		isSeed = false,
		loading = false,
		error = null,
		open = false,
		onClose = () => {},
		onMix = () => {},
		onToggleSeed = () => {}
	}: {
		node?: GalaxyNode | null;
		listenHeat?: GenreHeat | null;
		tracks?: Track[];
		lineage?: string[];
		nearbyGenres?: string[];
		isSeed?: boolean;
		loading?: boolean;
		error?: string | null;
		open?: boolean;
		onClose?: () => void;
		onMix?: () => void;
		onToggleSeed?: () => void;
	} = $props();

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

	function formatListenTime(value: number): string {
		if (!value || value <= 0) return '0m';
		const minutes = Math.floor(value / 60000);
		const hours = Math.floor(minutes / 60);
		if (hours > 0) return `${hours}h ${String(minutes % 60).padStart(2, '0')}m`;
		return `${minutes}m`;
	}

	let listenCount = $derived(listenHeat?.listen_count ?? node?.listenCount ?? 0);
	let listenedTime = $derived(listenHeat?.total_listened_ms ?? node?.totalListenedMs ?? 0);
	let visibleTracks = $derived(tracks.slice(0, 20));
	let heatSignal = $derived(Math.round((node?.heatNorm ?? 0) * 100));
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
				<p>{node.depth === 0 ? 'Primary anchor in the live taxonomy atlas.' : `Depth ${node.depth} branch in the live taxonomy atlas.`}</p>
			</div>
			<button class="close-btn" onclick={onClose} aria-label="Close genre panel">×</button>
		</div>

		<div class="panel-actions">
			<div class="panel-action-row">
				<button class="btn btn-primary" onclick={onMix}>▶ Start mix</button>
				<button class={`btn btn-glass ${isSeed ? 'is-seed' : ''}`} onclick={onToggleSeed}>
					{isSeed ? 'Seed locked' : 'Lock as seed'}
				</button>
			</div>
			<p class="panel-action-copy">Queue replaces with this branch, then genre shuffle + automix take over.</p>
		</div>

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

		<div class="signal-grid">
			<section class="signal-block">
				<p class="signal-label">Lineage</p>
				<div class="signal-row">
					<span>Family</span>
					<strong>{node.familyName}</strong>
				</div>
				<div class="signal-row">
					<span>Node level</span>
					<strong>{node.depth === 0 ? 'Root' : `Depth ${node.depth}`}</strong>
				</div>
				<div class="signal-row">
					<span>Branch size</span>
					<strong>{node.trackCount.toLocaleString()} tracks</strong>
				</div>
			</section>

			<section class="signal-block">
				<p class="signal-label">Momentum</p>
				<div class="signal-row">
					<span>90-day listens</span>
					<strong>{listenCount.toLocaleString()}</strong>
				</div>
				<div class="signal-row">
					<span>Listened time</span>
					<strong>{formatListenTime(listenedTime)}</strong>
				</div>
				<div class="signal-row">
					<span>Heat intensity</span>
					<strong>{heatSignal}%</strong>
				</div>
			</section>
		</div>

		{#if nearbyGenres.length > 0}
			<div class="nearby-block">
				<p class="signal-label">Nearby scenes</p>
				<div class="nearby-chips">
					{#each nearbyGenres as genreName}
						<span class="nearby-chip">{genreName}</span>
					{/each}
				</div>
			</div>
		{/if}

		<div class="track-section">
			<div class="track-heading">
				<h3>Listening dossier</h3>
				<span>{tracks.length > 20 ? 'Showing first 20' : `${tracks.length} loaded`}</span>
			</div>

			{#if loading}
				<EmptyState title="Loading tracks" copy={`Pulling ${node.name} tracks for the panel.`} />
			{:else if error}
				<EmptyState title="Tracks could not load" copy={error} />
			{:else if visibleTracks.length === 0}
				<EmptyState title="No tracks in this branch" copy="This node does not currently resolve to any playable tracks." />
			{:else}
				<div class="track-list">
					{#each visibleTracks as track (track.id)}
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
	.panel-copy,
	.track-heading {
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

	.panel-copy p,
	.family-name,
	.track-heading span,
	.panel-action-copy,
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

	.panel-action-row {
		display: flex;
		flex-wrap: wrap;
		gap: 8px;
	}

	.panel-action-row :global(.btn.is-seed) {
		border-color: color-mix(in srgb, var(--accent-line) 90%, transparent);
		box-shadow: 0 0 18px color-mix(in srgb, var(--accent-glow) 45%, transparent);
	}

	.panel-action-copy {
		font-size: 0.74rem;
		line-height: 1.45;
	}

	.breadcrumb-row {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 6px;
	}

	.breadcrumb-chip,
	.nearby-chip {
		padding: 4px 9px;
		border-radius: 999px;
		font-size: 0.7rem;
		letter-spacing: 0.04em;
		background: color-mix(in srgb, var(--instrument-surface-strong) 88%, transparent);
		border: 1px solid color-mix(in srgb, var(--instrument-border) 50%, transparent);
		color: var(--signal-text);
	}

	.breadcrumb-sep {
		color: var(--text-muted);
		font-size: 0.72rem;
	}

	.signal-grid {
		display: grid;
		gap: 10px;
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

	.signal-block {
		display: flex;
		flex-direction: column;
		gap: 4px;
		padding: 10px 12px;
		border-radius: var(--radius);
		background: color-mix(in srgb, var(--instrument-surface) 84%, transparent);
		border: 1px solid color-mix(in srgb, var(--instrument-border) 58%, transparent);
	}

	.signal-label {
		font-size: 0.66rem;
		font-weight: 700;
		letter-spacing: 0.11em;
		text-transform: uppercase;
		color: var(--signal-text);
		padding-bottom: 4px;
	}

	.signal-row {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: 10px;
		font-size: 0.8rem;
		padding: 4px 0;
	}

	.signal-row span {
		color: var(--signal-text);
	}

	.signal-row strong {
		font-size: 0.85rem;
		font-weight: 650;
		color: var(--text-primary);
		text-align: right;
	}

	.track-section {
		display: flex;
		flex-direction: column;
		gap: 12px;
		min-height: 0;
	}

	.track-heading {
		gap: 3px;
	}

	.track-heading h3 {
		font-size: 1rem;
		letter-spacing: 0.01em;
	}

	.track-list {
		display: flex;
		flex-direction: column;
		gap: 8px;
		overflow: auto;
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
