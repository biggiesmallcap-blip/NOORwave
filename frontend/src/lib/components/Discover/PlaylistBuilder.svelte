<script lang="ts">
	import type { DiscoverTrackNode } from './discover.types';
	import { addTrackToQueue, playTrackNow } from '$lib/stores/player';

	let { selectedNodes = [] }: { selectedNodes?: DiscoverTrackNode[] } = $props();

	async function playSelected() {
		if (selectedNodes.length === 0) return;
		for (const node of selectedNodes) {
			await addTrackToQueue(node.track_id);
		}
		if (selectedNodes.length > 0) {
			await playTrackNow(selectedNodes[0]!.track_id);
		}
	}

	async function queueSelected() {
		if (selectedNodes.length === 0) return;
		for (const node of selectedNodes) {
			await addTrackToQueue(node.track_id);
		}
	}

	function sortByEnergy() {
		selectedNodes.sort((a, b) => (a.energy ?? 0) - (b.energy ?? 0));
	}
</script>

{#if selectedNodes.length > 0}
	<div class="playlist-builder glass-panel">
		<h4>{selectedNodes.length} tracks selected</h4>
		<div class="builder-actions">
			<button class="btn btn-primary btn-sm" onclick={playSelected}>▶ Play</button>
			<button class="btn btn-glass btn-sm" onclick={queueSelected}>+ Queue</button>
			<button class="btn btn-glass btn-sm" onclick={sortByEnergy}>⟳ Energy Sort</button>
		</div>
	</div>
{/if}

<style>
	.playlist-builder { padding: 12px; }
	.builder-actions { display: flex; gap: 8px; margin-top: 8px; flex-wrap: wrap; }
	.btn-sm { padding: 6px 10px; font-size: 0.78rem; }
</style>
