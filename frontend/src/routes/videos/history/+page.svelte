<script lang="ts">
	import { onMount } from 'svelte';
	import { api, type TidalSearchVideo, type VideoHistoryEntry } from '$lib/api/client';
	import VideoCard from '$lib/components/video/VideoCard.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import Skeleton from '$lib/components/ui/Skeleton.svelte';
	import { playVideo } from '$lib/stores/video_session';
	import { showToast } from '$lib/stores/toast';
	import { historyDays, playsLine } from '$lib/video/history_days';

	// The History tab: every video you have watched, newest first, in days
	// (Today, Yesterday, the weekday, then the date), one card per video with
	// how far you got. Watches are recorded by the player; the
	// same history keeps recently watched picks off the shelves and feeds the
	// discovery crawler, so removing one here also lets it come back.
	const HISTORY_LIMIT = 200;
	let entries = $state<VideoHistoryEntry[]>([]);
	let loading = $state(true);
	let failed = $state(false);
	let clearing = $state(false);

	let queue = $derived(entries.map((entry) => entry.video));
	let days = $derived(historyDays(entries));
	let countLine = $derived(
		entries.length >= HISTORY_LIMIT ? `Your latest ${HISTORY_LIMIT} videos` : `${entries.length} ${entries.length === 1 ? 'video' : 'videos'}`
	);

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
		<header class="opener">
			<div class="opener-text">
				<h2>History</h2>
				<p>{countLine}, newest first, with how far you got.</p>
			</div>
			<button type="button" class="clear" disabled={clearing} onclick={() => void clearAll()}>Clear history</button>
		</header>
		{#each days as day (day.key)}
			<section class="day" aria-label={day.label}>
				<h3 class="day-label">{day.label}<span class="day-count">{day.entries.length}</span></h3>
				<div class="video-grid">
					{#each day.entries as entry, index (entry.video.tidal_id)}
						{@const done = progress(entry)}
						<div class="history-card rise-in-card" style={`--rise-index: ${index % 24}`}>
							<VideoCard video={entry.video} onSelect={(item) => !('id' in item) && play(item)} />
							{#if done !== null}
								<div class="progress" aria-label={entry.completed ? 'Watched to the end' : `Watched ${Math.round(done * 100)}%`}>
									<span style={`width: ${Math.max(4, done * 100)}%`}></span>
								</div>
							{/if}
							{#if entry.plays > 1}<p class="watched">{playsLine(entry.plays)}</p>{/if}
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
					{/each}
				</div>
			</section>
		{/each}
	{/if}
</div>

<style>
	.history-page {
		display: grid;
		grid-template-columns: minmax(0, 1fr);
		gap: var(--space-6);
	}

	/* The opener, as the Logbook's: the word in the titles' italic serif,
	   one quiet line, and clearing kept quiet beside it. */
	.opener {
		display: flex;
		align-items: flex-end;
		justify-content: space-between;
		gap: var(--space-4);
	}

	.opener-text {
		display: grid;
		gap: 6px;
	}

	.opener h2 {
		margin: 0;
		color: var(--text-primary);
		font-family: var(--font-display);
		font-size: var(--font-size-2xl);
		font-style: italic;
		font-weight: var(--font-weight-medium);
		line-height: 1.1;
		letter-spacing: -0.01em;
	}

	.opener p {
		margin: 0;
		color: var(--text-tertiary);
		font-size: var(--font-size-sm);
	}

	.clear {
		height: 30px;
		padding: 0 12px;
		border: 0;
		border-radius: 999px;
		background: none;
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		cursor: pointer;
		transition: background var(--motion-fast), color var(--motion-fast);
	}

	.clear:hover:not(:disabled) {
		background: var(--bg-hover);
		color: var(--text-primary);
	}

	.clear:focus-visible {
		outline: 2px solid var(--accent-strong);
		outline-offset: 2px;
	}

	/* A day: its name over a hairline, then its cards. */
	.day {
		display: grid;
		gap: var(--space-4);
	}

	.day-label {
		display: flex;
		align-items: baseline;
		gap: 8px;
		margin: 0;
		padding-bottom: var(--space-2);
		border-bottom: 1px solid var(--border-subtle);
		color: var(--text-secondary);
		font-size: var(--font-size-label);
		font-weight: var(--font-weight-semibold);
		letter-spacing: 0.12em;
		text-transform: uppercase;
	}

	.day-count {
		color: var(--text-tertiary);
		font-family: var(--font-mono);
		font-weight: var(--font-weight-medium);
		letter-spacing: 0;
	}

	.video-grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(210px, 1fr));
		gap: 18px 14px;
	}

	.history-card {
		position: relative;
		min-width: 0;
	}

	/* How far you got, as a thin bar under the card. */
	.progress {
		height: 2px;
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
		margin: var(--space-1) 0 0;
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
	}

	/* Removing sits on the picture's corner, only for the card you are on. */
	.remove {
		position: absolute;
		top: 8px;
		right: 8px;
		z-index: 1;
		display: grid;
		place-items: center;
		width: 26px;
		height: 26px;
		padding: 0;
		border: 0;
		border-radius: 50%;
		background: rgba(0, 0, 0, 0.6);
		color: rgba(255, 255, 255, 0.86);
		cursor: pointer;
		opacity: 0;
		transition: opacity var(--motion-fast), background var(--motion-fast), color var(--motion-fast);
	}

	.history-card:hover .remove,
	.remove:focus-visible {
		opacity: 1;
	}

	.remove:hover {
		background: rgba(0, 0, 0, 0.82);
		color: #fff;
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
