<script lang="ts">
	import type { DiscoverTrackNode } from './discover.types';

	let { node = null }: { node?: DiscoverTrackNode | null } = $props();
</script>

{#if node}
	<div class="discover-panel glass-panel">
		{#if node.artwork_url}
			<img src={node.artwork_url} alt="" class="panel-artwork" />
		{/if}
		<h3>{node.title}</h3>
		<p>{node.artist_name}</p>
		{#if node.album_title}<p class="album">{node.album_title}</p>{/if}

		<div class="metrics">
			{#if node.bpm}<div class="metric"><span>BPM</span><strong>{node.bpm}</strong></div>{/if}
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
	.panel-artwork { width: 100%; aspect-ratio: 1; border-radius: var(--radius); object-fit: cover; }
	.metrics { display: grid; grid-template-columns: 1fr 1fr; gap: 8px; margin-top: 12px; }
	.metric { display: flex; flex-direction: column; gap: 4px; }
	.metric span { color: var(--text-secondary); font-size: 0.75rem; }
	.key-badge { display: inline-block; padding: 2px 8px; border-radius: 999px; background: rgba(255,255,255,0.08); font-size: 0.8rem; color: var(--text-primary); }
	.bar { height: 4px; background: rgba(255,255,255,0.1); border-radius: 2px; overflow: hidden; }
	.bar-fill { height: 100%; background: var(--accent, #7c80ff); border-radius: 2px; }
</style>
