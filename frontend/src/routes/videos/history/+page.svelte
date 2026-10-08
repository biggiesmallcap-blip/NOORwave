<script lang="ts">
	import { onMount } from 'svelte';
	import { api, type TidalSearchVideo, type VideoHistoryEntry } from '$lib/api/client';
	import VideoCard from '$lib/components/video/VideoCard.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import Skeleton from '$lib/components/ui/Skeleton.svelte';
	import { playVideo } from '$lib/stores/video_session';
	import { showToast } from '$lib/stores/toast';
	import { formatDateShort } from '$lib/utils/format';

	// The History tab: every video you have watched, newest first, one card
	// per video with how far you got. Watches are recorded by the player; the
	// same history keeps recently watched picks off the shelves and feeds the
	// discovery crawler, so removing one here also lets it come back.
	const HISTORY_LIMIT = 200;
	let entries = $state<VideoHistoryEntry[]>([]);
	let loading = $state(true);
	let failed = $state(false);
	let clearing = $state(false);

	let queue = $derived(entries.map((entry) => entry.video));

	async function load() {
		try {
			entries = (await api.getVideoHistory(HISTORY_LIMIT)).items;
			failed = false;
		} catch {
			failed = true;
		} finally {
			loading = false;
		}
	}

	function progress(entry: VideoHistoryEntry): number | null {
		if (entry.completed) return 1;
		const duration = entry.duration_ms ?? entry.video.duration_ms;
		if (!duration || !entry.watched_ms) return null;
		return Math.min(1, entry.watched_ms / duration);
	}

	function watchedLine(entry: VideoHistoryEntry): string {
		const when = formatDateShort(entry.watched_at);
		const times = entry.plays > 1 ? ` - watched ${entry.plays} times` : '';
		return `${when}${times}`;
	}

	function play(video: TidalSearchVideo) {
		void playVideo(video, { queue, source: 'search', sourceLabel: 'Watch history' });
	}

	async function remove(entry: VideoHistoryEntry) {
		const before = entries;
		entries = entries.filter((item) => item.video.tidal_id !== entry.video.tidal_id);
		try {
			const result = await api.removeVideoFromHistory(entry.video.tidal_id);
			if (!result.ok) throw new Error();
		} catch {
			entries = before;
			showToast('Could not remove that video from your history.', 'error');
		}
	}

	async function clearAll() {
		if (clearing || entries.length === 0) return;
		if (!confirm('Clear your whole watch history? Recently watched picks can then come back on the shelves.')) return;
		clearing = true;
		try {
			const result = await api.clearVideoHistory();
			if (!result.ok) throw new Error();
			entries = [];
			showToast('Watch history cleared.');
		} catch {
			showToast('Could not clear your watch history.', 'error');
		} finally {
			clearing = false;
		}
	}

	onMount(() => {
		void load();
	});
</script>

<svelte:head><title>Watch history - NOOR</title></svelte:head>

<div class="history-page">
	{#if loading}
		<Skeleton rows={6} label="Loading your watch history" />
	{:else if failed}
		<EmptyState title="Could not load your watch history" copy="Check that NOORwave's server is running, then reopen this tab." />
	{:else if entries.length === 0}
		<EmptyState title="Nothing watched yet" copy="Videos you play show up here, newest first, with how far you got." />
	{:else}
		<div class="history-head">
			<h2>Watch history</h2>
			<span class="count">{entries.length >= HISTORY_LIMIT ? `Latest ${HISTORY_LIMIT}` : `${entries.length} ${entries.length === 1 ? 'video' : 'videos'}`}</span>
			<button type="button" class="btn btn-ghost clear" disabled={clearing} onclick={() => void clearAll()}>Clear history</button>
		</div>
		<div class="video-grid">
			{#each entries as entry, index (entry.video.tidal_id)}
				{@const done = progress(entry)}
				<div class="history-card rise-in-card" style={`--rise-index: ${index % 24}`}>
					<VideoCard video={entry.video} onSelect={(item) => !('id' in item) && play(item)} />
					{#if done !== null}
						<div class="progress" aria-label={entry.completed ? 'Watched to the end' : `Watched ${Math.round(done * 100)}%`}>
							<span style={`width: ${Math.max(4, done * 100)}%`}></span>
						</div>
					{/if}
					<div class="watched">
						<span>{watchedLine(entry)}</span>
						<button
							type="button"
							class="remove"
							aria-label={`Remove ${entry.video.title} from watch history`}
							title="Remove from history"
							onclick={() => void remove(entry)}
						>
							<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M6 6l12 12M18 6 6 18" /></svg>
						</button>
					</div>
				</div>
			{/each}
		</div>
	{/if}
</div>

<style>
	.history-page {
		display: grid;
		grid-template-columns: minmax(0, 1fr);
		gap: var(--space-4);
	}

	.history-head {
		display: flex;
		align-items: baseline;
		gap: var(--space-3);
	}

	.history-head h2 {
		margin: 0;
		font-size: var(--font-size-xl);
		font-weight: var(--font-weight-bold);
	}

	.count {
		color: var(--text-tertiary);
		font-size: var(--font-size-sm);
	}

	.clear {
		margin-left: auto;
	}

	.video-grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(210px, 1fr));
		gap: 14px;
	}

	.history-card {
		position: relative;
		min-width: 0;
	}

	/* How far you got, as a thin bar under the card. */
	.progress {
		height: 3px;
		margin-top: var(--space-2);
		border-radius: 999px;
		background: var(--border-subtle);
		overflow: hidden;
	}

	.progress span {
		display: block;
		height: 100%;
		background: var(--accent);
	}

	.watched {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		margin-top: var(--space-1);
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
	}

	.remove {
		display: grid;
		place-items: center;
		width: 22px;
		height: 22px;
		margin-left: auto;
		padding: 0;
		border: 0;
		border-radius: 50%;
		background: transparent;
		color: var(--text-tertiary);
		cursor: pointer;
		opacity: 0;
		transition: opacity var(--motion-fast), background var(--motion-fast), color var(--motion-fast);
	}

	.history-card:hover .remove,
	.remove:focus-visible {
		opacity: 1;
	}

	.remove:hover {
		background: var(--bg-hover);
		color: var(--text-primary);
	}

	.remove:focus-visible {
		outline: 2px solid var(--accent-strong);
		outline-offset: 1px;
	}

	.remove svg {
		width: 12px;
		height: 12px;
		fill: none;
		stroke: currentColor;
		stroke-width: 2.4;
		stroke-linecap: round;
	}
</style>
