<script lang="ts">
	import { onMount } from 'svelte';
	import { getApiBase, authFetch } from '$lib/api/client';
	import { formatTrackDuration, getQualityClass } from '$lib/utils/format';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import StateBadge from '$lib/components/ui/StateBadge.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import MetricPair from '$lib/components/ui/MetricPair.svelte';
	import { buildTrackMenu } from '$lib/player/track_menu';
	import { openContextMenu } from '$lib/stores/context_menu';

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
	}

	interface GroupDifference {
		kind: string;
		values: string[];
	}

	type Relationship =
		| 'exact_duplicate'
		| 'cross_album_reissue'
		| 'remaster'
		| 'alt_version'
		| 'quality_variant';

	interface DuplicateGroup {
		id: number;
		status: string;
		relationship: Relationship;
		differences: GroupDifference[];
		members: DuplicateMember[];
	}

	const RELATIONSHIPS: Relationship[] = [
		'exact_duplicate',
		'cross_album_reissue',
		'remaster',
		'alt_version',
		'quality_variant'
	];

	const FILTER_STORAGE_KEY = 'noor.duplicates.relationshipFilter';

	let scanState = $state<'idle' | 'scanning' | 'done'>('idle');
	let scanStats = $state<{
		groups_found: number;
		tracks_affected: number;
		isrc_matches: number;
		title_matches: number;
	} | null>(null);
	let groups = $state<DuplicateGroup[]>([]);
	let total = $state(0);
	let loading = $state(false);
	let loadingMore = $state(false);
	let resolving = $state<Set<number>>(new Set());
	let errorMsg = $state('');
	let activeRelationships = $state<Set<Relationship>>(new Set(RELATIONSHIPS));

	onMount(() => {
		try {
			const raw = localStorage.getItem(FILTER_STORAGE_KEY);
			if (raw) {
				const parsed = JSON.parse(raw) as string[];
				const valid = parsed.filter((r): r is Relationship =>
					RELATIONSHIPS.includes(r as Relationship)
				);
				if (valid.length > 0) activeRelationships = new Set(valid);
			}
		} catch {
			// ignore — bad JSON or no storage; keep defaults.
		}
		void loadGroups();
	});

	function persistFilter() {
		try {
			localStorage.setItem(FILTER_STORAGE_KEY, JSON.stringify([...activeRelationships]));
		} catch {
			// no-op
		}
	}

	function toggleRelationship(rel: Relationship) {
		const next = new Set(activeRelationships);
		if (next.has(rel)) {
			if (next.size === 1) return; // never let the user hide everything.
			next.delete(rel);
		} else {
			next.add(rel);
		}
		activeRelationships = next;
		persistFilter();
	}

	async function runScan() {
		scanState = 'scanning';
		errorMsg = '';
		try {
			const resp = await authFetch(`${getApiBase()}/api/library/duplicates/scan`, {
				method: 'POST'
			});
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
			const resp = await authFetch(
				`${getApiBase()}/api/library/duplicates?limit=20&offset=${offset}`
			);
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
			await authFetch(`${getApiBase()}/api/library/duplicates/${groupId}/dismiss`, {
				method: 'POST'
			});
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

	function relationshipLabel(rel: Relationship): string {
		switch (rel) {
			case 'exact_duplicate':
				return 'Exact duplicate';
			case 'cross_album_reissue':
				return 'Cross-album re-release';
			case 'remaster':
				return 'Remaster';
			case 'alt_version':
				return 'Alt version';
			case 'quality_variant':
				return 'Quality variant';
		}
	}

	function relationshipTone(
		rel: Relationship
	): 'success' | 'warning' | 'muted' | 'active' {
		switch (rel) {
			case 'exact_duplicate':
				return 'success';
			case 'remaster':
			case 'alt_version':
				return 'warning';
			case 'quality_variant':
			case 'cross_album_reissue':
				return 'muted';
		}
	}

	function differenceLabel(diff: GroupDifference): string {
		const kindLabel: Record<string, string> = {
			version_marker: 'Marker',
			year: 'Year',
			album: 'Album',
			quality: 'Quality',
			sample_rate: 'Sample rate',
			source: 'Source'
		};
		const head = kindLabel[diff.kind] ?? diff.kind;
		const values = diff.values.length === 0 ? '—' : diff.values.join(' · ');
		return `${head}: ${values}`;
	}

	function openDuplicateTrackContextMenu(event: MouseEvent, track: DuplicateTrack) {
		event.preventDefault();
		event.stopPropagation();
		openContextMenu(
			event,
			buildTrackMenu({
				id: track.id,
				title: track.title,
				artist_name: track.artist_name,
				album_title: track.album_title,
				is_favorite: track.is_favorite
			}),
			track.title
		);
	}

	let visibleGroups = $derived(groups.filter((g) => activeRelationships.has(g.relationship)));

	let removableCount = $derived(
		scanStats
			? Math.max(0, scanStats.tracks_affected - scanStats.groups_found)
			: groups.reduce((count, group) => count + Math.max(0, group.members.length - 1), 0)
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
		<MetricPair
			label="Groups"
			value={(scanStats?.groups_found ?? total).toLocaleString()}
			copy="Open duplicate sets."
		/>
		<MetricPair label="Extra copies" value={removableCount.toLocaleString()} copy="Possible removals." />
		<MetricPair
			label="ISRC matches"
			value={(scanStats?.isrc_matches ?? 0).toLocaleString()}
			copy="Exact recording matches."
		/>
	</section>

	{#if groups.length > 0}
		<div class="filter-row" role="group" aria-label="Filter by relationship">
			{#each RELATIONSHIPS as rel (rel)}
				{@const active = activeRelationships.has(rel)}
				<button
					type="button"
					class="filter-chip"
					class:active
					onclick={() => toggleRelationship(rel)}
					aria-pressed={active}
				>
					{relationshipLabel(rel)}
				</button>
			{/each}
		</div>
	{/if}

	{#if errorMsg}
		<EmptyState title="Duplicate review hit a problem" copy={errorMsg} />
	{:else if loading}
		<EmptyState title="Loading duplicate groups" copy="Fetching the current review set." />
	{:else if groups.length === 0 && scanState === 'done'}
		<EmptyState title="No duplicate groups found" copy="The library looks clean right now." />
	{:else if groups.length === 0}
		<EmptyState title="No scan has been run yet" copy="Start a scan to surface repeated tracks." />
	{:else if visibleGroups.length === 0}
		<EmptyState
			title="All groups filtered out"
			copy="Re-enable a relationship chip above to see groups."
		/>
	{:else}
		<div class="groups-list">
			{#each visibleGroups as group (group.id)}
				{@const busy = resolving.has(group.id)}
				{@const preferred = group.members.find((member) => member.is_preferred)?.track}
				{@const lead = group.members[0]?.track}
				{@const isExact = group.relationship === 'exact_duplicate'}
				<section class="group-card glass-panel" class:busy>
					<div class="group-head">
						<div class="group-title">
							<p class="eyebrow">{lead?.artist_name ?? 'Unknown artist'}</p>
							<h3>{lead?.title ?? 'Unknown track'}</h3>
							{#if group.differences.length > 0}
								<div class="diff-chips">
									{#each group.differences as diff (diff.kind)}
										<span class="diff-chip">{differenceLabel(diff)}</span>
									{/each}
								</div>
							{/if}
						</div>
						<div class="group-head-right">
							<StateBadge
								label={relationshipLabel(group.relationship)}
								tone={relationshipTone(group.relationship)}
							/>
							{#if isExact && preferred}
								<button
									class="btn btn-primary"
									onclick={() => resolveGroup(group.id, preferred.id)}
									disabled={busy}
								>
									{busy ? 'Resolving…' : 'Keep best'}
								</button>
							{/if}
							<button class="btn btn-glass" onclick={() => dismissGroup(group.id)} disabled={busy}>
								Dismiss
							</button>
						</div>
					</div>

					<div class="member-grid">
						{#each group.members as member (member.track.id)}
							<!-- svelte-ignore a11y_no_static_element_interactions -->
							<article
								class="member-card"
								class:preferred={member.is_preferred}
								oncontextmenu={(event) => openDuplicateTrackContextMenu(event, member.track)}
							>
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
									<span class={`quality-badge ${getQualityClass(member.track.best_quality)}`}>
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
										<span>Title</span>
										<strong>{member.track.title}</strong>
									</div>
									<div class="info-row">
										<span>Duration</span>
										<strong>{formatTrackDuration(member.track.duration_ms)}</strong>
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
									<button
										class="btn btn-glass choose-btn"
										onclick={() => resolveGroup(group.id, member.track.id)}
										disabled={busy}
									>
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
	.filter-row {
		display: flex;
		flex-wrap: wrap;
		gap: var(--gap-sm);
		padding: var(--space-1) 0;
	}

	.filter-chip {
		padding: var(--space-2) var(--space-3);
		border-radius: 999px;
		border: 1px solid var(--border-subtle);
		background: var(--bg-surface);
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
		cursor: pointer;
		transition:
			background var(--motion-fast),
			border-color var(--motion-fast),
			color var(--motion-fast);
	}

	.filter-chip:hover {
		background: var(--bg-hover);
		color: var(--text-primary);
	}

	.filter-chip.active {
		background: var(--accent-soft);
		border-color: var(--accent-line);
		color: var(--text-primary);
	}

	.groups-list {
		display: flex;
		flex-direction: column;
		gap: var(--space-4);
	}

	.group-card {
		padding: var(--space-5);
		display: flex;
		flex-direction: column;
		gap: var(--space-5);
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

	.group-title {
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
		min-width: 0;
	}

	.group-head-right {
		display: flex;
		flex-wrap: wrap;
		justify-content: flex-end;
		gap: var(--space-2);
	}

	.diff-chips {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space-2);
	}

	.diff-chip {
		padding: var(--space-1) var(--space-3);
		border-radius: 999px;
		background: var(--bg-surface);
		border: 1px solid var(--border-subtle);
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
		white-space: nowrap;
	}

	.member-grid {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(min(260px, 100%), 1fr));
		gap: var(--space-3);
	}

	.member-card {
		padding: var(--space-4);
		border-radius: var(--radius-md);
		background: var(--bg-surface);
		border: 1px solid var(--border-subtle);
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
	}

	.member-card.preferred {
		border-color: var(--accent-line);
		background: var(--accent-soft);
	}

	.member-card-head {
		display: flex;
		align-items: center;
		gap: var(--space-3);
	}

	.member-art {
		width: clamp(2.5rem, 3vw, 3rem);
		aspect-ratio: 1 / 1;
		border-radius: var(--radius-sm);
		object-fit: cover;
		background: var(--bg-surface);
		border: 1px solid var(--panel-border);
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
		gap: var(--space-2);
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
