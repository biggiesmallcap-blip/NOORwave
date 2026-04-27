<script lang="ts">
	import { api } from '$lib/api/client';
	import type { DiscoverTrackNode } from './discover.types';
	import type { TidalSearchTrack } from '$lib/api/client';
	import { playTidalTrackNow, addTidalTrackToQueue } from '$lib/stores/player';

	let { node = null }: { node?: DiscoverTrackNode | null } = $props();

	type ResolutionState = 'idle' | 'resolving' | 'resolved' | 'unavailable';

	// Persists across node selections within this page session.
	const resolutionCache = new Map<number, TidalSearchTrack | null>();

	let resolutionState = $state<ResolutionState>('idle');
	let resolvedTrack = $state<TidalSearchTrack | null>(null);

	$effect(() => {
		const n = node;
		if (!n || n.source !== 'external') {
			resolutionState = 'idle';
			resolvedTrack = null;
			return;
		}

		if (resolutionCache.has(n.track_id)) {
			const cached = resolutionCache.get(n.track_id) as TidalSearchTrack | null;
			resolvedTrack = cached;
			resolutionState = cached !== null ? 'resolved' : 'unavailable';
			return;
		}

		resolutionState = 'resolving';
		resolvedTrack = null;
		const capturedId = n.track_id;

		api.searchTidal(`${n.title} ${n.artist_name}`, 1)
			.then(results => {
				if (node?.track_id !== capturedId) return;
				const first = results.tracks[0] ?? null;
				resolutionCache.set(capturedId, first);
				resolvedTrack = first;
				resolutionState = first !== null ? 'resolved' : 'unavailable';
			})
			.catch(() => {
				if (node?.track_id !== capturedId) return;
				resolutionCache.set(capturedId, null);
				resolutionState = 'unavailable';
			});
	});

	async function play() {
		if (!node) return;
		if (node.source === 'external') {
			if (resolvedTrack) {
				await playTidalTrackNow({
					tidal_id: resolvedTrack.tidal_id,
					title: resolvedTrack.title,
					artist_name: resolvedTrack.artist_name,
					album_title: resolvedTrack.album_title,
					artwork_url: resolvedTrack.artwork_url,
					duration_ms: resolvedTrack.duration_ms,
					artist_tidal_id: resolvedTrack.artist_id ?? null,
				});
			}
			return;
		}
		await api.playTrack(node.track_id);
	}

	async function queue() {
		if (!node) return;
		if (node.source === 'external') {
			if (resolvedTrack) {
				await addTidalTrackToQueue({
					tidal_id: resolvedTrack.tidal_id,
					title: resolvedTrack.title,
					artist_name: resolvedTrack.artist_name,
					album_title: resolvedTrack.album_title,
					artwork_url: resolvedTrack.artwork_url,
					duration_ms: resolvedTrack.duration_ms,
					artist_tidal_id: resolvedTrack.artist_id ?? null,
				});
			}
			return;
		}
		await api.addQueueTrack(node.track_id);
	}
</script>

{#if node}
	<div class="discover-panel glass-panel">
		{#if node.artwork_url}
			<img src={node.artwork_url} alt="" class="panel-artwork" />
		{/if}
		<h3>{node.title}</h3>
		<p>{node.artist_name}</p>
		{#if node.album_title}<p class="album">{node.album_title}</p>{/if}

		<div class="panel-actions">
			<button class="action-btn primary" onclick={play}>▶ Play</button>
			<button class="action-btn" onclick={queue}>+ Queue</button>
		</div>

		<div class="metrics">
			{#if node.bpm}<div class="metric"><span>BPM</span><strong>{Math.round(node.bpm)}</strong></div>{/if}
			{#if node.camelot_key}<div class="metric"><span>Key</span><span class="key-badge">{node.camelot_key}</span></div>{/if}
			{#if node.energy != null}
				<div class="metric"><span>Energy</span>
					<div class="bar"><div class="bar-fill" style="width:{node.energy * 100}%"></div></div>
				</div>
			{/if}
			{#if node.danceability != null}
				<div class="metric"><span>Dance</span>
					<div class="bar"><div class="bar-fill" style="width:{node.danceability * 100}%"></div></div>
				</div>
			{/if}
		</div>
	</div>
{/if}

<style>
	.discover-panel { padding: 16px; }
	.panel-artwork { width: 100%; aspect-ratio: 1; border-radius: var(--radius); object-fit: cover; margin-bottom: 10px; }
	h3 { font-size: 0.9rem; font-weight: 600; margin: 0 0 2px; color: rgba(255,255,255,0.9); }
	p { font-size: 0.8rem; color: rgba(255,255,255,0.5); margin: 0 0 2px; }
	.album { font-style: italic; }
	.panel-actions { display: flex; gap: 6px; margin: 10px 0; }
	.action-btn {
		flex: 1;
		padding: 7px 0;
		border-radius: 8px;
		border: none;
		background: rgba(255,255,255,0.07);
		color: rgba(255,255,255,0.75);
		font-size: 0.8rem;
		cursor: pointer;
		transition: background 0.15s;
	}
	.action-btn:hover { background: rgba(255,255,255,0.12); }
	.action-btn.primary { background: rgba(124,128,255,0.25); color: rgba(255,255,255,0.95); }
	.action-btn.primary:hover { background: rgba(124,128,255,0.4); }
	.metrics { display: grid; grid-template-columns: 1fr 1fr; gap: 8px; margin-top: 12px; }
	.metric { display: flex; flex-direction: column; gap: 4px; }
	.metric span { color: var(--text-secondary); font-size: 0.75rem; }
	.metric strong { color: rgba(255,255,255,0.85); font-size: 0.85rem; }
	.key-badge { display: inline-block; padding: 2px 8px; border-radius: 999px; background: rgba(255,255,255,0.08); font-size: 0.8rem; color: var(--text-primary); }
	.bar { height: 4px; background: rgba(255,255,255,0.1); border-radius: 2px; overflow: hidden; }
	.bar-fill { height: 100%; background: var(--accent, #7c80ff); border-radius: 2px; }
</style>
