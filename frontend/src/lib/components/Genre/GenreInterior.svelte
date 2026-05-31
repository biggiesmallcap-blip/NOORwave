<script lang="ts">
	import { onMount } from 'svelte';
	import { api, type Track, type GenreHeat } from '$lib/api/client';
	import { cachedApi } from '$lib/cache/api_queries';
	import { playTrackNow, setPlayerAutomixEnabled, setPlayerShuffleMode } from '$lib/stores/player';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { buildTrackMenu } from '$lib/player/track_menu';
	import { buildArtistMenu } from '$lib/player/artist_menu';
	import type { GalaxyNode } from './galaxy.types';

	function handleTrackContextMenu(event: MouseEvent, track: Track) {
		openContextMenu(event, buildTrackMenu(track));
	}

	function handleArtistContextMenu(event: MouseEvent, artist: ArtistCluster) {
		if (artist.artistId == null) return;
		event.preventDefault();
		event.stopPropagation();
		openContextMenu(
			event,
			buildArtistMenu({ id: artist.artistId, name: artist.name, in_library: true }, { isLocal: true }),
			artist.name
		);
	}

	type ArtistCluster = {
		name: string;
		artistId: number | null;
		count: number;
		x: number;
		y: number;
		radius: number;
	};

	let {
		node = null,
		heat = null,
		cohortLabel = null,
		onClose = () => {},
		onPlayMix = () => {}
	}: {
		node?: GalaxyNode | null;
		heat?: GenreHeat | null;
		cohortLabel?: string | null;
		onClose?: () => void;
		onPlayMix?: () => void;
	} = $props();

	let tracks = $state<Track[]>([]);
	let loading = $state(true);
	let error = $state<string | null>(null);
	let actionError = $state<string | null>(null);

	// Artist micro-galaxy data
	let artistClusters = $state<ArtistCluster[]>([]);
	let canvasEl = $state<HTMLCanvasElement | null>(null);
	let width = $state(0);
	let height = $state(0);
	let dpr = $state(1);

	let totalListens = $derived(heat?.listen_count ?? 0);
	let totalTime = $derived(heat?.total_listened_ms ?? 0);
	let evolutionData = $derived(node?.evolutionHistory ?? []);

	function formatMs(ms: number): string {
		const totalSeconds = Math.floor(ms / 1000);
		const hours = Math.floor(totalSeconds / 3600);
		const minutes = Math.floor((totalSeconds % 3600) / 60);
		if (hours > 0) return `${hours}h ${minutes}m`;
		return `${minutes}m`;
	}

	function buildArtistGalaxy(trackList: Track[]) {
		const artistAcc = new Map<string, { count: number; artistId: number | null }>();
		for (const track of trackList) {
			const artist = track.artist_name?.trim();
			if (!artist) continue;
			const entry = artistAcc.get(artist) ?? { count: 0, artistId: track.artist_id ?? null };
			entry.count += 1;
			if (entry.artistId == null && track.artist_id != null) entry.artistId = track.artist_id;
			artistAcc.set(artist, entry);
		}

		const sorted = [...artistAcc.entries()].sort((a, b) => b[1].count - a[1].count);
		const topArtists = sorted.slice(0, 20);
		const maxCount = topArtists[0]?.[1].count ?? 1;

		// Place artists in a mini force-directed layout
		const nodes = topArtists.map(([name, info], i) => ({
			name,
			artistId: info.artistId,
			count: info.count,
			x: Math.cos((Math.PI * 2 * i) / topArtists.length) * 80,
			y: Math.sin((Math.PI * 2 * i) / topArtists.length) * 80,
			vx: 0,
			vy: 0,
			radius: 4 + Math.sqrt(info.count / maxCount) * 12
		}));

		// Mini simulation
		for (let tick = 0; tick < 80; tick++) {
			for (let i = 0; i < nodes.length; i++) {
				for (let j = i + 1; j < nodes.length; j++) {
					const a = nodes[i];
					const b = nodes[j];
					const dx = b.x - a.x;
					const dy = b.y - a.y;
					const distSq = Math.max(dx * dx + dy * dy, 16);
					const dist = Math.sqrt(distSq);
					const force = 60 / distSq;
					a.vx -= (dx / dist) * force;
					a.vy -= (dy / dist) * force;
					b.vx += (dx / dist) * force;
					b.vy += (dy / dist) * force;
				}
			}
			for (const n of nodes) {
				n.vx *= 0.82;
				n.vy *= 0.82;
				n.x += n.vx;
				n.y += n.vy;
			}
		}

		artistClusters = nodes.map((n) => ({
			name: n.name,
			artistId: n.artistId,
			count: n.count,
			x: n.x,
			y: n.y,
			radius: n.radius
		}));
	}

	function drawArtistGalaxy() {
		if (!canvasEl || width === 0 || height === 0) return;
		const ctx = canvasEl.getContext('2d');
		if (!ctx) return;

		ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
		ctx.clearRect(0, 0, width, height);

		const cx = width / 2;
		const cy = height / 2;
		const scale = Math.min(width / 260, height / 180, 1.2);

		// Background stars
		ctx.save();
		for (let i = 0; i < 60; i++) {
			const x = ((i * 7919 + 13) % width);
			const y = ((i * 6271 + 37) % height);
			const r = ((i * 3) % 2) + 0.5;
			ctx.globalAlpha = 0.15 + ((i * 7) % 30) / 100;
			ctx.fillStyle = '#fff';
			ctx.beginPath();
			ctx.arc(x, y, r, 0, Math.PI * 2);
			ctx.fill();
		}
		ctx.restore();

		// Center glow
		const gradient = ctx.createRadialGradient(cx, cy, 0, cx, cy, 80 * scale);
		gradient.addColorStop(0, hexToRgba(node?.color ?? '#7c80ff', 0.15));
		gradient.addColorStop(1, 'rgba(0, 0, 0, 0)');
		ctx.fillStyle = gradient;
		ctx.beginPath();
		ctx.arc(cx, cy, 80 * scale, 0, Math.PI * 2);
		ctx.fill();

		// Artist nodes
		for (const artist of artistClusters) {
			const x = cx + artist.x * scale;
			const y = cy + artist.y * scale;
			const r = artist.radius * scale;

			ctx.save();
			const glow = ctx.createRadialGradient(x, y, 0, x, y, r * 2);
			glow.addColorStop(0, hexToRgba(node?.color ?? '#7c80ff', 0.3));
			glow.addColorStop(1, 'rgba(0, 0, 0, 0)');
			ctx.fillStyle = glow;
			ctx.beginPath();
			ctx.arc(x, y, r * 2, 0, Math.PI * 2);
			ctx.fill();

			ctx.beginPath();
			ctx.arc(x, y, r, 0, Math.PI * 2);
			ctx.fillStyle = hexToRgba(node?.color ?? '#7c80ff', 0.6 + (artist.count / (artistClusters[0]?.count ?? 1)) * 0.3);
			ctx.shadowBlur = 10;
			ctx.shadowColor = node?.glowColor ?? 'rgba(124, 128, 255, 0.34)';
			ctx.fill();

			// Label
			ctx.globalAlpha = 0.8;
			ctx.fillStyle = '#e0e3ff';
			ctx.font = `${Math.max(9, 11 * scale)}px "Avenir Next", sans-serif`;
			ctx.textAlign = 'center';
			ctx.textBaseline = 'top';
			ctx.fillText(artist.name, x, y + r + 4);
			ctx.restore();
		}
	}

	function hexToRgba(hex: string, alpha: number): string {
		const normalized = hex.replace('#', '');
		if (normalized.length !== 6) return `rgba(255, 255, 255, ${alpha})`;
		const red = Number.parseInt(normalized.slice(0, 2), 16);
		const green = Number.parseInt(normalized.slice(2, 4), 16);
		const blue = Number.parseInt(normalized.slice(4, 6), 16);
		return `rgba(${red}, ${green}, ${blue}, ${alpha})`;
	}

	async function loadTracks() {
		if (!node) return;
		loading = true;
		error = null;
		try {
			const response = await cachedApi.getGenreTracks(node.id, true);
			tracks = response.tracks;
			buildArtistGalaxy(tracks);
		} catch (reason) {
			error = reason instanceof Error ? reason.message : String(reason);
		} finally {
			loading = false;
		}
	}

	async function handlePlayTrack(track: Track) {
		if (!node) return;
		try {
			await api.replacePlaybackQueue(tracks.map(t => t.id));
			await setPlayerShuffleMode('genre');
			await setPlayerAutomixEnabled(true);
			await playTrackNow(track.id);
		} catch (reason) {
			actionError = reason instanceof Error ? reason.message : String(reason);
		}
	}

	async function handleMix() {
		if (!node || tracks.length === 0) return;
		try {
			await api.replacePlaybackQueue(tracks.map(t => t.id));
			const shuffled = await setPlayerShuffleMode('genre');
			await setPlayerAutomixEnabled(true);
			await playTrackNow(shuffled?.queue[0]?.track.id ?? tracks[0].id);
		} catch (reason) {
			actionError = reason instanceof Error ? reason.message : String(reason);
		}
	}

	$effect(() => {
		if (node) {
			void loadTracks();
		}
	});

	$effect(() => {
		if (artistClusters.length > 0 && canvasEl && width > 0) {
			drawArtistGalaxy();
		}
	});

	function handleResize(entries: ResizeObserverEntry[]) {
		const entry = entries[0];
		if (!entry) return;
		const rect = entry.contentRect;
		width = rect.width;
		height = rect.height;
		dpr = window.devicePixelRatio ?? 1;
		if (canvasEl) {
			canvasEl.width = rect.width * dpr;
			canvasEl.height = rect.height * dpr;
			if (artistClusters.length > 0) drawArtistGalaxy();
		}
	}

	let resizeObserver: ResizeObserver | null = null;
	onMount(() => {
		if (canvasEl?.parentElement) {
			resizeObserver = new ResizeObserver(handleResize);
			resizeObserver.observe(canvasEl.parentElement);
		}
		return () => resizeObserver?.disconnect();
	});
</script>

{#if !node}
	<div class="interior-empty">
		<p>Select a genre node to explore its interior.</p>
	</div>
{:else}
	<div class="genre-interior">
		<div class="interior-header glass-panel">
			<div class="header-top">
				<button class="back-btn" onclick={onClose} aria-label="Back to galaxy">← Galaxy</button>
				<div class="header-title">
					<span class="family-dot" style={`background: ${node.color}`}></span>
					<h2>{node.name}</h2>
				</div>
				{#if cohortLabel}
					<span class="cohort-chip">{cohortLabel}</span>
				{/if}
			</div>

			<div class="header-stats">
				<div><strong>{node.trackCount}</strong><span>tracks</span></div>
				{#if totalListens > 0}
					<div><strong>{totalListens}</strong><span>listens</span></div>
					<div><strong>{formatMs(totalTime)}</strong><span>listened</span></div>
				{/if}
			</div>

			{#if node.avgBpm != null || node.avgEnergy != null || node.avgDanceability != null}
				<div class="audio-dials" aria-label="Audio character">
					{#if node.avgBpm != null}
						<div class="dial">
							<span class="dial-value">{Math.round(node.avgBpm)}</span>
							<span class="dial-label">BPM</span>
						</div>
					{/if}
					{#if node.avgEnergy != null}
						<div class="dial">
							<span class="dial-value">{node.avgEnergy.toFixed(2)}</span>
							<span class="dial-label">Energy</span>
						</div>
					{/if}
					{#if node.avgDanceability != null}
						<div class="dial">
							<span class="dial-value">{node.avgDanceability.toFixed(2)}</span>
							<span class="dial-label">Danceability</span>
						</div>
					{/if}
				</div>
			{/if}

			{#if evolutionData.length > 1}
				<div class="evolution-strip">
					<span class="evo-label">Listening trail</span>
					<div class="evo-bars">
						{#each evolutionData.slice(-12) as point}
							{@const maxVal = Math.max(...evolutionData.slice(-12).map(p => p.listenCount), 1)}
							<div class="evo-bar-wrap">
								<div
									class="evo-bar"
									style={`height: ${Math.max(4, (point.listenCount / maxVal) * 36)}px; opacity: ${0.3 + (point.listenCount / maxVal) * 0.7}`}
								></div>
								<span class="evo-date">{point.periodStart.slice(5)}</span>
							</div>
						{/each}
					</div>
				</div>
			{/if}
		</div>

		<div class="artist-galaxy-wrap glass-panel">
			<div class="galaxy-canvas-container">
				<canvas bind:this={canvasEl}></canvas>
				{#if loading}
					<div class="galaxy-loading">
						<span>Mapping artists...</span>
					</div>
				{/if}
			</div>
			<p class="galaxy-copy">
				{artistClusters.length > 0
					? `${artistClusters.length} artists orbiting by play count`
					: 'No artist data available yet'}
			</p>

			{#if artistClusters.length > 0}
				<div class="artist-chip-row">
					{#each artistClusters.slice(0, 8) as artist}
						{#if artist.artistId != null}
							<a
								class="artist-chip"
								href={`/artists/${artist.artistId}`}
								oncontextmenu={(event) => handleArtistContextMenu(event, artist)}
							>{artist.name}</a>
						{:else}
							<span class="artist-chip artist-chip-static">{artist.name}</span>
						{/if}
					{/each}
				</div>
			{/if}
		</div>

		{#if actionError}
			<div class="error-chip">{actionError}</div>
		{/if}

		<div class="track-list glass-panel">
			<div class="track-list-header">
				<h3>Tracks</h3>
				{#if tracks.length > 0}
					<button class="mix-btn" onclick={() => void handleMix()}>▶ Mix this genre</button>
				{/if}
			</div>

			{#if loading}
				<div class="track-loading">Loading tracks…</div>
			{:else if error}
				<div class="track-error">{error}</div>
			{:else}
				<div class="tracks">
					{#each tracks.slice(0, 40) as track (track.id)}
						<div
							class="track-row"
							role="button"
							tabindex="0"
							oncontextmenu={(event) => handleTrackContextMenu(event, track)}
						>
							{#if track.artwork_url}
								<img class="track-art" src={track.artwork_url} alt="" />
							{:else}
								<div class="track-art placeholder">♫</div>
							{/if}
							<div class="track-meta">
								<strong>{track.title}</strong>
								<span>{track.artist_name ?? 'Unknown'}</span>
							</div>
							<button class="play-btn" onclick={() => void handlePlayTrack(track)}>▶</button>
						</div>
					{/each}
					{#if tracks.length > 40}
						<p class="more-hint">+ {tracks.length - 40} more tracks</p>
					{/if}
				</div>
			{/if}
		</div>
	</div>
{/if}

<style>
	.genre-interior {
		position: absolute;
		inset: 0;
		z-index: 20;
		padding: 16px;
		display: flex;
		flex-direction: column;
		gap: 12px;
		overflow-y: auto;
		background:
			radial-gradient(circle at 30% 20%, rgba(124, 128, 255, 0.08), transparent 40%),
			radial-gradient(circle at 70% 80%, rgba(6, 214, 160, 0.06), transparent 35%),
			linear-gradient(180deg, rgba(8, 10, 18, 0.94), rgba(6, 7, 14, 0.97));
	}

	.interior-header {
		padding: 12px 14px;
		display: flex;
		flex-direction: column;
		gap: 8px;
	}

	.header-top {
		display: flex;
		align-items: center;
		gap: 10px;
	}

	.back-btn {
		padding: 6px 12px;
		border-radius: 999px;
		border: 1px solid color-mix(in srgb, var(--instrument-border) 50%, transparent);
		background: color-mix(in srgb, var(--instrument-surface) 60%, transparent);
		color: var(--signal-text);
		font-size: var(--font-size-xs);
		cursor: pointer;
	}

	.back-btn:hover {
		background: color-mix(in srgb, var(--accent-soft) 50%, transparent);
	}

	.header-title {
		display: flex;
		align-items: center;
		gap: 8px;
		flex: 1;
	}

	.family-dot {
		width: 10px;
		height: 10px;
		border-radius: 50%;
	}

	.header-title h2 {
		font-size: var(--font-size-lg);
		font-family: var(--font-display);
	}

	.cohort-chip {
		padding: 4px 10px;
		border-radius: 999px;
		border: 1px solid rgba(255, 220, 160, 0.2);
		background: rgba(255, 220, 160, 0.08);
		color: rgba(255, 220, 160, 0.9);
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-semibold);
	}

	.header-stats {
		display: flex;
		gap: 8px;
	}

	.header-stats div {
		display: flex;
		align-items: baseline;
		gap: 5px;
		padding: 4px 10px;
		border-radius: 999px;
		background: color-mix(in srgb, var(--instrument-surface) 50%, transparent);
		border: 1px solid color-mix(in srgb, var(--instrument-border) 25%, transparent);
	}

	.header-stats strong {
		font-size: var(--font-size-sm);
		font-family: var(--font-display);
	}

	.header-stats span {
		color: var(--signal-text);
		font-size: var(--font-size-2xs);
		text-transform: uppercase;
		letter-spacing: 0.08em;
	}

	.evolution-strip {
		display: flex;
		align-items: center;
		gap: 10px;
		padding-top: 4px;
	}

	.evo-label {
		color: var(--signal-text);
		font-size: var(--font-size-2xs);
		text-transform: uppercase;
		letter-spacing: 0.08em;
		white-space: nowrap;
	}

	.audio-dials {
		display: flex;
		gap: 10px;
		padding-top: 4px;
	}

	.dial {
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 2px;
		min-width: 64px;
		padding: 6px 10px;
		border-radius: 12px;
		background: color-mix(in srgb, var(--instrument-surface-strong) 60%, transparent);
		border: 1px solid color-mix(in srgb, var(--instrument-border) 35%, transparent);
	}

	.dial-value {
		font-size: var(--font-size-md);
		font-weight: var(--font-weight-bold);
		font-variant-numeric: tabular-nums;
		color: var(--text-primary);
	}

	.dial-label {
		color: var(--signal-text);
		font-size: var(--font-size-2xs);
		text-transform: uppercase;
		letter-spacing: 0.08em;
	}

	.artist-chip-row {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
		padding: 8px 0 0;
	}

	.artist-chip {
		padding: 4px 10px;
		border-radius: 999px;
		font-size: var(--font-size-xs);
		background: color-mix(in srgb, var(--instrument-surface) 70%, transparent);
		border: 1px solid color-mix(in srgb, var(--instrument-border) 36%, transparent);
		color: var(--text-primary);
		text-decoration: none;
		transition:
			background var(--motion-fast),
			border-color var(--motion-fast),
			transform var(--motion-fast);
	}

	a.artist-chip:hover {
		background: color-mix(in srgb, var(--accent-soft) 55%, var(--instrument-surface));
		border-color: color-mix(in srgb, var(--accent-line) 75%, transparent);
		transform: translateY(-1px);
	}

	.artist-chip-static {
		opacity: 0.65;
	}

	.evo-bars {
		display: flex;
		align-items: flex-end;
		gap: 3px;
		flex: 1;
		height: 42px;
	}

	.evo-bar-wrap {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 2px;
		flex: 1;
	}

	.evo-bar {
		width: 100%;
		min-height: 3px;
		border-radius: 2px;
		background: var(--accent-soft);
	}

	.evo-date {
		color: var(--signal-text);
		font-size: var(--font-size-2xs);
		opacity: 0.6;
	}

	.artist-galaxy-wrap {
		padding: 10px;
		display: flex;
		flex-direction: column;
		gap: 6px;
	}

	.galaxy-canvas-container {
		position: relative;
		width: 100%;
		height: 180px;
		border-radius: 8px;
		overflow: hidden;
	}

	.galaxy-canvas-container canvas {
		width: 100%;
		height: 100%;
	}

	.galaxy-loading {
		position: absolute;
		inset: 0;
		display: grid;
		place-items: center;
		background: rgba(8, 10, 18, 0.7);
	}

	.galaxy-loading span {
		color: var(--signal-text);
		font-size: var(--font-size-sm);
	}

	.galaxy-copy {
		color: var(--signal-text);
		font-size: var(--font-size-xs);
		text-align: center;
	}

	.error-chip {
		padding: 8px 12px;
		border-radius: 8px;
		background: rgba(28, 10, 16, 0.8);
		border: 1px solid rgba(255, 80, 80, 0.2);
		color: #ff6b6b;
		font-size: var(--font-size-xs);
	}

	.track-list {
		padding: 10px 14px;
		display: flex;
		flex-direction: column;
		gap: 8px;
	}

	.track-list-header {
		display: flex;
		justify-content: space-between;
		align-items: center;
	}

	.track-list-header h3 {
		font-size: var(--font-size-sm);
		font-family: var(--font-display);
	}

	.mix-btn {
		padding: 6px 14px;
		border-radius: 999px;
		border: none;
		background: var(--accent-soft);
		color: var(--text-primary);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		cursor: pointer;
	}

	.mix-btn:hover {
		background: color-mix(in srgb, var(--accent) 70%, var(--accent-soft));
	}

	.track-loading {
		color: var(--signal-text);
		font-size: var(--font-size-sm);
		padding: 20px 0;
		text-align: center;
	}

	.track-error {
		color: #ff6b6b;
		font-size: var(--font-size-xs);
		padding: 10px 0;
	}

	.tracks {
		display: flex;
		flex-direction: column;
		gap: 6px;
	}

	.track-row {
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 6px 8px;
		border-radius: 8px;
		transition: background 0.15s;
	}

	.track-row:hover {
		background: color-mix(in srgb, var(--instrument-surface) 40%, transparent);
	}

	.track-art {
		width: 40px;
		height: 40px;
		border-radius: 6px;
		object-fit: cover;
		flex-shrink: 0;
	}

	.track-art.placeholder {
		width: 40px;
		height: 40px;
		border-radius: 6px;
		background: color-mix(in srgb, var(--instrument-surface) 50%, transparent);
		display: grid;
		place-items: center;
		color: var(--signal-text);
		flex-shrink: 0;
	}

	.track-meta {
		flex: 1;
		min-width: 0;
		display: flex;
		flex-direction: column;
		gap: 2px;
	}

	.track-meta strong {
		font-size: var(--font-size-sm);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.track-meta span {
		color: var(--signal-text);
		font-size: var(--font-size-xs);
	}

	.play-btn {
		width: 32px;
		height: 32px;
		border-radius: 50%;
		border: 1px solid color-mix(in srgb, var(--instrument-border) 40%, transparent);
		background: color-mix(in srgb, var(--instrument-surface) 50%, transparent);
		color: var(--signal-text);
		font-size: var(--font-size-sm);
		cursor: pointer;
		flex-shrink: 0;
	}

	.play-btn:hover {
		background: var(--accent-soft);
		color: var(--text-primary);
	}

	.more-hint {
		color: var(--signal-text);
		font-size: var(--font-size-xs);
		text-align: center;
		padding: 8px 0;
		opacity: 0.6;
	}

	.interior-empty {
		display: grid;
		place-items: center;
		min-height: 200px;
	}

	.interior-empty p {
		color: var(--signal-text);
		font-size: var(--font-size-sm);
	}

	@media (max-width: 760px) {
		.genre-interior {
			padding: 10px;
		}

		.galaxy-canvas-container {
			height: 140px;
		}
	}
</style>
