<script lang="ts">
	import { onMount } from 'svelte';
	import { getApiBase, authFetch } from '$lib/api/client';
	import { formatDuration } from '$lib/stores/library';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import StateBadge from '$lib/components/ui/StateBadge.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import MetricPair from '$lib/components/ui/MetricPair.svelte';

	interface DuplicateTrack {
		id: number;
		title: string;
		artist_name: string | null;
		album_title: string | null;
		duration_ms: number | null;
		best_quality: string | null;
		fidelity_score: number;
		is_favorite: boolean;
		play_count: number;
		source: string;
		tidal_id: number | null;
		artwork_url: string | null;
	}

	interface DuplicateMember {
		track: DuplicateTrack;
		is_preferred: boolean;
		match_reason: string;
	}

	interface DuplicateGroup {
		id: number;
		status: string;
		members: DuplicateMember[];
	}

	let scanState = $state<'idle' | 'scanning' | 'done'>('idle');
	let scanStats = $state<{ groups_found: number; tracks_affected: number; isrc_matches: number; title_matches: number } | null>(null);
	let groups = $state<DuplicateGroup[]>([]);
	let total = $state(0);
	let loading = $state(false);
	let loadingMore = $state(false);
	let resolving = $state<Set<number>>(new Set());
	let errorMsg = $state('');

	onMount(() => {
		void loadGroups();
	});

	async function runScan() {
		scanState = 'scanning';
		errorMsg = '';
		try {
			const resp = await authFetch(`${getApiBase()}/api/library/duplicates/scan`, { method: 'POST' });
			if (!resp.ok) throw new Error(`Scan failed: ${resp.status}`);
			scanStats = await resp.json();
			scanState = 'done';
			groups = [];
			await loadGroups();
		} catch (e) {
			errorMsg = String(e);
			scanState = 'idle';
		}
	}

	async function loadGroups(append = false) {
		if (!append) loading = true;
		else loadingMore = true;
		try {
			const offset = append ? groups.length : 0;
			const resp = await authFetch(`${getApiBase()}/api/library/duplicates?limit=20&offset=${offset}`);
			const data = await resp.json();
			total = data.total;
			groups = append ? [...groups, ...data.groups] : data.groups;
			if (groups.length > 0 && scanState === 'idle') scanState = 'done';
		} catch (e) {
			errorMsg = String(e);
		} finally {
			loading = false;
			loadingMore = false;
		}
	}

	async function resolveGroup(groupId: number, preferredTrackId: number) {
		resolving = new Set([...resolving, groupId]);
		try {
			const resp = await authFetch(`${getApiBase()}/api/library/duplicates/${groupId}/resolve`, {
				method: 'POST',
				headers: { 'Content-Type': 'application/json' },
				body: JSON.stringify({ preferred_track_id: preferredTrackId })
			});
			if (!resp.ok) throw new Error(`Resolve failed: ${resp.status}`);
			groups = groups.filter((group) => group.id !== groupId);
			total = Math.max(0, total - 1);
		} catch (e) {
			errorMsg = String(e);
		} finally {
			resolving = new Set([...resolving].filter((id) => id !== groupId));
		}
	}

	async function dismissGroup(groupId: number) {
		resolving = new Set([...resolving, groupId]);
		try {
			await authFetch(`${getApiBase()}/api/library/duplicates/${groupId}/dismiss`, { method: 'POST' });
			groups = groups.filter((group) => group.id !== groupId);
			total = Math.max(0, total - 1);
		} finally {
			resolving = new Set([...resolving].filter((id) => id !== groupId));
		}
	}

	function qualityLabel(q: string | null): string {
		if (!q) return 'Unknown';
		if (q === 'HI_RES_LOSSLESS' || q === 'HI_RES') return 'Hi-Res';
		if (q === 'LOSSLESS') return 'Lossless';
		if (q === 'HIGH') return 'High';
		return q;
	}

	function qualityClass(q: string | null): string {
		if (!q) return 'lossy';
		if (q.startsWith('HI_RES')) return 'hires';
		if (q === 'LOSSLESS') return 'lossless';
		return 'lossy';
	}

	let removableCount = $derived(
		scanStats ? Math.max(0, scanStats.tracks_affected - scanStats.groups_found) : groups.reduce((count, group) => count + Math.max(0, group.members.length - 1), 0)
	);
</script>

<svelte:head>
	<title>Duplicates | NOOR</title>
</svelte:head>

<div class="page-shell duplicates-page animate-in">
	<PageHeader
		eyebrow="Duplicates"
		title="Duplicate review"
		subtitle="Compare versions, keep the best copy, or dismiss the match."
	>
		{#snippet actions()}
			<button class="btn btn-primary" onclick={runScan} disabled={scanState === 'scanning'}>
				{scanState === 'scanning' ? 'Scanning…' : scanState === 'done' ? 'Rescan' : 'Scan library'}
			</button>
		{/snippet}
	</PageHeader>

	<section class="stat-grid">
		<MetricPair label="Groups" value={(scanStats?.groups_found ?? total).toLocaleString()} copy="Open duplicate sets." />
		<MetricPair label="Extra copies" value={removableCount.toLocaleString()} copy="Possible removals." />
		<MetricPair label="ISRC matches" value={(scanStats?.isrc_matches ?? 0).toLocaleString()} copy="Exact recording matches." />
	</section>

	{#if errorMsg}
		<EmptyState title="Duplicate review hit a problem" copy={errorMsg} />
	{:else if loading}
		<EmptyState title="Loading duplicate groups" copy="Fetching the current review set." />
	{:else if groups.length === 0 && scanState === 'done'}
		<EmptyState title="No duplicate groups found" copy="The library looks clean right now." />
	{:else if groups.length === 0}
		<EmptyState title="No scan has been run yet" copy="Start a scan to surface repeated tracks." />
	{:else}
		<div class="groups-list">
			{#each groups as group (group.id)}
				{@const busy = resolving.has(group.id)}
				{@const preferred = group.members.find((member) => member.is_preferred)?.track}
				{@const lead = group.members[0]?.track}
				<section class="group-card glass-panel" class:busy>
					<div class="group-head">
						<div>
							<p class="eyebrow">{lead?.artist_name ?? 'Unknown artist'}</p>
							<h3>{lead?.title ?? 'Unknown track'}</h3>
						</div>
						<div class="group-head-right">
							<StateBadge label={group.members[0]?.match_reason === 'isrc' ? 'ISRC match' : 'Title + duration match'} tone={group.members[0]?.match_reason === 'isrc' ? 'success' : 'muted'} />
							{#if preferred}
								<button class="btn btn-primary" onclick={() => resolveGroup(group.id, preferred.id)} disabled={busy}>
									{busy ? 'Resolving…' : 'Keep best'}
								</button>
							{/if}
							<button class="btn btn-glass" onclick={() => dismissGroup(group.id)} disabled={busy}>Dismiss</button>
						</div>
					</div>

					<div class="member-grid">
						{#each group.members as member (member.track.id)}
							<article class="member-card" class:preferred={member.is_preferred}>
								<div class="member-card-head">
									{#if member.track.artwork_url}
										<img class="member-art" src={member.track.artwork_url} alt="" />
									{:else}
										<div class="member-art placeholder">♫</div>
									{/if}
									<div class="member-title">
										<h4>{member.track.album_title ?? 'Standalone / unknown album'}</h4>
										<p>{member.track.artist_name ?? 'Unknown artist'}</p>
									</div>
								</div>

								<div class="member-badges">
									<span class={`quality-badge ${qualityClass(member.track.best_quality)}`}>
										{qualityLabel(member.track.best_quality)}
									</span>
									{#if member.track.is_favorite}
										<StateBadge label="Favorite" tone="warning" compact={true} />
									{/if}
									{#if member.is_preferred}
										<StateBadge label="Recommended keep" tone="active" compact={true} />
									{/if}
								</div>

								<div class="info-list">
									<div class="info-row">
										<span>Duration</span>
										<strong>{formatDuration(member.track.duration_ms)}</strong>
									</div>
									<div class="info-row">
										<span>Plays</span>
										<strong>{member.track.play_count.toLocaleString()}</strong>
									</div>
									<div class="info-row">
										<span>Source</span>
										<strong>{member.track.source}</strong>
									</div>
									<div class="info-row">
										<span>Fidelity score</span>
										<strong>{member.track.fidelity_score}</strong>
									</div>
								</div>

								{#if !member.is_preferred}
									<button class="btn btn-glass choose-btn" onclick={() => resolveGroup(group.id, member.track.id)} disabled={busy}>
										Keep this version
									</button>
								{/if}
							</article>
						{/each}
					</div>
				</section>
			{/each}
		</div>

		{#if groups.length < total}
			<div class="load-row">
				<p>{groups.length} of {total} groups loaded</p>
				<button class="btn btn-glass" onclick={() => loadGroups(true)} disabled={loadingMore}>
					{loadingMore ? 'Loading…' : 'Load more'}
				</button>
			</div>
		{/if}
	{/if}
</div>

<style>
	.groups-list {
		display: flex;
		flex-direction: column;
		gap: var(--space-4);
	}

	.group-card {
		padding: 22px;
		display: flex;
		flex-direction: column;
		gap: 18px;
	}

	.group-card.busy {
		opacity: 0.6;
		pointer-events: none;
	}

	.group-head {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: var(--space-4);
	}

	.group-head-right {
		display: flex;
		flex-wrap: wrap;
		justify-content: flex-end;
		gap: var(--space-2);
	}

	.member-grid {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(260px, 1fr));
		gap: var(--space-3);
	}

	.member-card {
		padding: 16px;
		border-radius: var(--radius);
		background: rgba(255, 255, 255, 0.03);
		border: 1px solid rgba(255, 255, 255, 0.06);
		display: flex;
		flex-direction: column;
		gap: 14px;
	}

	.member-card.preferred {
		border-color: rgba(124, 128, 255, 0.22);
		background: rgba(124, 128, 255, 0.08);
	}

	.member-card-head {
		display: flex;
		align-items: center;
		gap: 12px;
	}

	.member-art {
		width: 46px;
		height: 46px;
		border-radius: 12px;
		object-fit: cover;
		background: rgba(255, 255, 255, 0.04);
		border: 1px solid rgba(255, 255, 255, 0.08);
	}

	.placeholder {
		display: grid;
		place-items: center;
		color: var(--text-tertiary);
	}

	.member-title p,
	.load-row p {
		color: var(--text-secondary);
	}

	.member-badges {
		display: flex;
		flex-wrap: wrap;
		gap: 8px;
	}

	.choose-btn {
		margin-top: auto;
	}

	.load-row {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-3);
	}

	@media (max-width: 860px) {
		.group-head,
		.load-row {
			flex-direction: column;
			align-items: flex-start;
		}

		.group-head-right {
			justify-content: flex-start;
		}
	}
</style>
