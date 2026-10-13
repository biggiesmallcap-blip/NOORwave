<script lang="ts">
	// Blind radio comparison (Settings > Library > Radio blind test). Each seed
	// shows the station designs' first tracks as Station A, B, C in a shuffled
	// order. Mark tracks that do not fit, pick the best station; Reveal names
	// the designs and scores them. The session is saved on the server.
	import { onDestroy, onMount } from 'svelte';
	import { api } from '$lib/api/client';
	import type { RadioCompareArm, RadioCompareTrack } from '$lib/api/client';
	import { currentTrack, isPlaying, playTidalTracksNow } from '$lib/stores/player';
	import { showToast } from '$lib/stores/toast';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import ErrorState from '$lib/components/ui/ErrorState.svelte';
	import Skeleton from '$lib/components/ui/Skeleton.svelte';
	import FilterChip from '$lib/components/ui/FilterChip.svelte';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import TidalTrackRow from '$lib/components/TidalTrackRow.svelte';
	import {
		addSeed,
		armName,
		emptySession,
		parseSession,
		removeSeed,
		setBest,
		stationLabel,
		summarize,
		toggleMisfit,
		trackKey,
		withArms,
		type CompareSession,
	} from '$lib/radio_compare/session';

	const SAVE_DELAY_MS = 600;

	let session = $state<CompareSession>(emptySession());
	let ready = $state(false);
	let activeId = $state<number | null>(null);
	let loadingId = $state<number | null>(null);
	let loadError = $state<unknown>(null);
	let revealed = $state(false);
	let suggesting = $state(false);
	let saveTimer: ReturnType<typeof setTimeout> | null = null;

	const active = $derived(session.entries.find((entry) => entry.seed.local_id === activeId) ?? null);
	const summary = $derived(summarize(session));
	const judged = $derived(session.entries.filter((entry) => entry.best).length);

	function save() {
		saveTimer = null;
		api.saveRadioCompareSession(session).catch(() =>
			showToast("Couldn't save the blind test. Your last marks may be lost.", 'error')
		);
	}

	function commit(next: CompareSession) {
		session = next;
		if (saveTimer) clearTimeout(saveTimer);
		saveTimer = setTimeout(save, SAVE_DELAY_MS);
	}

	onMount(async () => {
		try {
			const { session: saved } = await api.getRadioCompareSession();
			session = parseSession(saved);
		} catch {
			// A missing or unreadable session starts a fresh one.
		} finally {
			ready = true;
		}
		const first = session.entries[0]?.seed.local_id;
		if (first != null) select(first);
	});

	onDestroy(() => {
		if (saveTimer) {
			clearTimeout(saveTimer);
			save();
		}
	});

	async function load(localId: number) {
		loadingId = localId;
		loadError = null;
		try {
			const result = await api.radioCompare(localId);
			commit(withArms(session, localId, result.arms));
		} catch (error) {
			if (activeId === localId) loadError = error;
		} finally {
			if (loadingId === localId) loadingId = null;
		}
	}

	function select(localId: number) {
		activeId = localId;
		loadError = null;
		const entry = session.entries.find((e) => e.seed.local_id === localId);
		if (entry && !entry.arms && loadingId !== localId) void load(localId);
	}

	async function addSuggested() {
		suggesting = true;
		try {
			const { seeds } = await api.radioCompareSeeds();
			const before = session.entries.length;
			commit(seeds.reduce(addSeed, session));
			const added = session.entries.length - before;
			showToast(
				added > 0 ? `Added ${added} seeds` : 'Every suggested seed is already here',
				added > 0 ? 'success' : 'info'
			);
			if (activeId == null && session.entries[0]?.seed.local_id != null) {
				select(session.entries[0].seed.local_id);
			}
		} catch {
			showToast("Couldn't load suggested seeds", 'error');
		} finally {
			suggesting = false;
		}
	}

	function addPlaying() {
		const track = $currentTrack;
		if (!track || track.id <= 0) {
			showToast('Play a track from your library first', 'info');
			return;
		}
		const seed: RadioCompareTrack = {
			tidal_id: track.tidal_id ?? 0,
			title: track.title,
			artist_name: track.artist_name ?? null,
			album_title: track.album_title ?? null,
			artwork_url: track.artwork_url ?? null,
			duration_ms: track.duration_ms ?? null,
			artist_tidal_id: track.artist_tidal_id ?? null,
			album_tidal_id: track.album_tidal_id ?? null,
			local_id: track.id,
		};
		commit(addSeed(session, seed));
		select(track.id);
	}

	function remove(localId: number) {
		const index = session.entries.findIndex((e) => e.seed.local_id === localId);
		commit(removeSeed(session, localId));
		const next = session.entries[Math.min(index, session.entries.length - 1)]?.seed.local_id;
		activeId = null;
		if (next != null) select(next);
	}

	function listFor(arms: RadioCompareArm[], arm: string) {
		return arms.find((a) => a.arm === arm) ?? { arm, tracks: [], note: null };
	}

	function playFrom(list: RadioCompareArm, label: string, startIndex = 0) {
		void playTidalTracksNow(list.tracks, `Station ${label}`, { startIndex });
	}

	function percent(value: number | null) {
		return value == null ? '-' : `${Math.round(value * 100)}%`;
	}
</script>

<div class="compare-page">
	<PageHeader
		title="Radio blind test"
		subtitle="Each seed gets its stations in a random order. Play them, mark tracks that don't fit the seed, and pick the best station. Reveal shows which station is which."
	>
		{#snippet actions()}
			<button type="button" class="btn btn-secondary" onclick={addSuggested} disabled={suggesting}>
				{suggesting ? 'Adding...' : 'Add suggested seeds'}
			</button>
			<button type="button" class="btn btn-secondary" onclick={addPlaying}>Add playing track</button>
			<button
				type="button"
				class="btn btn-primary"
				aria-pressed={revealed}
				onclick={() => (revealed = !revealed)}
				disabled={session.entries.length === 0}
			>
				{revealed ? 'Hide names' : 'Reveal'}
			</button>
		{/snippet}
	</PageHeader>

	{#if !ready}
		<Skeleton rows={6} label="Loading the blind test" />
	{:else if session.entries.length === 0}
		<EmptyState
			title="No seeds yet"
			copy="Add suggested seeds from your most played tracks across genres, or play a track and add it."
		>
			{#snippet actions()}
				<button type="button" class="btn btn-primary" onclick={addSuggested} disabled={suggesting}>
					Add suggested seeds
				</button>
			{/snippet}
		</EmptyState>
	{:else}
		<div class="seed-chips" role="group" aria-label="Seeds">
			{#each session.entries as entry (entry.seed.local_id)}
				<FilterChip
					pressed={entry.seed.local_id === activeId}
					onclick={() => entry.seed.local_id != null && select(entry.seed.local_id)}
					title={entry.best ? 'Judged' : 'Not judged yet'}
				>
					{#snippet children()}
						<span class="chip-mark" aria-hidden="true">{entry.best ? '✓' : ''}</span>
						{entry.seed.title}
					{/snippet}
				</FilterChip>
			{/each}
		</div>
		<p class="progress t-meta">{judged} of {session.entries.length} seeds judged</p>

		{#if active}
			<section class="seed" aria-label="Seed">
				<div class="seed-head">
					<ArtworkImage
						src={active.seed.artwork_url}
						size={320}
						className="seed-art"
						fallbackText={active.seed.title}
						decorative
					/>
					<div class="seed-copy">
						<p class="t-label">Seed</p>
						<h2 class="t-section">{active.seed.title}</h2>
						<p class="t-meta">{active.seed.artist_name ?? ''}</p>
					</div>
					<div class="seed-actions">
						<button
							type="button"
							class="btn btn-ghost"
							onclick={() => active.seed.local_id != null && load(active.seed.local_id)}
							disabled={loadingId === active.seed.local_id}
						>
							Reload
						</button>
						<button
							type="button"
							class="btn btn-ghost"
							onclick={() => active.seed.local_id != null && remove(active.seed.local_id)}
						>
							Remove seed
						</button>
					</div>
				</div>

				{#if loadingId === active.seed.local_id && !active.arms}
					<Skeleton rows={8} label="Building the stations" />
				{:else if loadError && !active.arms}
					<ErrorState
						title="Couldn't build the stations"
						error={loadError}
						onretry={() => active.seed.local_id != null && load(active.seed.local_id)}
					/>
				{:else if active.arms}
					<div class="stations">
						{#each active.order as armId, i (armId)}
							{@const list = listFor(active.arms, armId)}
							{@const label = stationLabel(i)}
							{@const marked = active.misfits[armId] ?? []}
							{@const isBest = active.best === armId}
							<div class="station" class:best={isBest}>
								<div class="station-head">
									<div class="station-name">
										<span class="t-label">Station {label}</span>
										{#if revealed}
											<span class="t-meta design">{armName(armId)}</span>
										{/if}
									</div>
									<span class="t-micro">
										{list.tracks.length} tracks{marked.length ? `, ${marked.length} marked` : ''}
									</span>
									<button
										type="button"
										class="btn btn-secondary"
										onclick={() => playFrom(list, label)}
										disabled={list.tracks.length === 0}
									>
										Play
									</button>
									<button
										type="button"
										class="btn {isBest ? 'btn-primary' : 'btn-ghost'}"
										aria-pressed={isBest}
										onclick={() => active.seed.local_id != null && commit(setBest(session, active.seed.local_id, armId))}
									>
										{isBest ? 'Best' : 'Pick best'}
									</button>
								</div>
								{#if list.note}
									<p class="note t-body">{list.note}</p>
								{/if}
								<ul class="rows">
									{#each list.tracks as track, index (trackKey(track))}
										{@const key = trackKey(track)}
										{@const misfit = marked.includes(key)}
										<li class="row" class:misfit>
											<div class="row-track">
												<TidalTrackRow
													{track}
													variant="art"
													showAlbum={false}
													isCurrent={track.tidal_id > 0 && $currentTrack?.tidal_id === track.tidal_id}
													isPlaying={track.tidal_id > 0 && $currentTrack?.tidal_id === track.tidal_id && $isPlaying}
													onRowClick={() => playFrom(list, label, index)}
												/>
											</div>
											<button
												type="button"
												class="row-btn misfit-btn"
												aria-pressed={misfit}
												aria-label={misfit ? `Unmark ${track.title}` : `${track.title} doesn't fit`}
												title={misfit ? 'Marked as not fitting' : "Doesn't fit the seed"}
												onclick={() => active.seed.local_id != null && commit(toggleMisfit(session, active.seed.local_id, armId, key))}
											>
												✕
											</button>
										</li>
									{/each}
								</ul>
							</div>
						{/each}
					</div>
				{/if}
			</section>
		{/if}

		{#if revealed}
			<section class="results" aria-label="Results">
				<h2 class="t-section">Results <span class="count">{judged} judged</span></h2>
				{#if summary.length === 0}
					<p class="t-body muted">Pick a best station for a seed to score it.</p>
				{:else}
					<table>
						<thead>
							<tr>
								<th scope="col">Design</th>
								<th scope="col">Fit</th>
								<th scope="col">Best picks</th>
								<th scope="col">Tracks</th>
								<th scope="col">Empty lists</th>
							</tr>
						</thead>
						<tbody>
							{#each summary as row (row.arm)}
								<tr>
									<th scope="row">{armName(row.arm)}</th>
									<td>{percent(row.fit)}</td>
									<td>{row.wins} of {row.seeds}</td>
									<td>{row.tracks}</td>
									<td>{row.empty}</td>
								</tr>
							{/each}
						</tbody>
					</table>
					<p class="t-meta muted">Fit is the share of shown tracks you did not mark, over judged seeds.</p>
				{/if}
			</section>
		{/if}
	{/if}
</div>

<style>
	.compare-page {
		width: min(100%, var(--content-width));
		margin: 0 auto;
		padding-bottom: var(--space-7);
		display: flex;
		flex-direction: column;
		gap: var(--group-gap);
	}

	.compare-page > :global(.page-header) {
		margin-bottom: calc(var(--header-gap) - var(--group-gap));
	}

	.seed-chips {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space-2);
	}

	.chip-mark {
		display: inline-block;
		width: 1ch;
		color: var(--accent-strong);
	}

	.progress {
		font-variant-numeric: tabular-nums;
	}

	.seed {
		display: flex;
		flex-direction: column;
		gap: var(--space-5);
		margin-top: var(--space-5);
	}

	.seed-head {
		display: grid;
		grid-template-columns: 64px minmax(0, 1fr) auto;
		align-items: center;
		gap: var(--space-4);
	}

	.seed-head :global(.seed-art) {
		width: 64px;
		height: 64px;
		border-radius: var(--radius-sm);
	}

	.seed-copy {
		min-width: 0;
		display: flex;
		flex-direction: column;
		gap: 2px;
	}

	.seed-actions {
		display: flex;
		gap: var(--space-2);
	}

	.stations {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(min(340px, 100%), 1fr));
		gap: var(--gap-lg);
		align-items: start;
	}

	.station {
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
		padding-top: var(--space-3);
		border-top: 2px solid transparent;
	}

	.station.best {
		border-top-color: var(--accent);
	}

	.station-head {
		display: grid;
		grid-template-columns: minmax(0, 1fr) auto auto auto;
		align-items: center;
		gap: var(--space-2);
	}

	.station-name {
		display: flex;
		flex-direction: column;
		gap: 2px;
		min-width: 0;
	}

	.design {
		color: var(--text-primary);
	}

	.note {
		color: var(--text-secondary);
	}

	.rows {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: var(--row-gap);
	}

	.row {
		display: grid;
		grid-template-columns: minmax(0, 1fr) 28px;
		align-items: center;
		gap: var(--space-1);
	}

	.row-track {
		min-width: 0;
		transition: opacity var(--motion-fast);
	}

	.row.misfit .row-track {
		opacity: 0.45;
	}

	.row:hover .misfit-btn,
	.row:focus-within .misfit-btn,
	.misfit-btn[aria-pressed='true'] {
		opacity: 1;
	}

	.misfit-btn[aria-pressed='true'] {
		color: var(--state-error);
	}

	.results {
		margin-top: var(--section-gap);
		display: flex;
		flex-direction: column;
		gap: var(--group-gap);
	}

	.count {
		color: var(--text-tertiary);
		font-weight: var(--font-weight-medium);
	}

	table {
		border-collapse: collapse;
		width: min(100%, var(--measure-form));
		font-variant-numeric: tabular-nums;
	}

	th,
	td {
		text-align: left;
		padding: var(--space-2) var(--space-3);
		border-bottom: 1px solid var(--border-subtle);
	}

	thead th {
		font-size: var(--font-size-label);
		font-weight: var(--font-weight-semibold);
		letter-spacing: 0.12em;
		text-transform: uppercase;
		color: var(--text-tertiary);
	}

	tbody th {
		font-weight: var(--font-weight-semibold);
	}

	.muted {
		color: var(--text-secondary);
	}
</style>
