<script lang="ts">
	import { onMount, tick } from 'svelte';
	import type { Unsubscriber } from 'svelte/store';
	import { api, type Genre, type GenreHeat, type GenreCoOccurrence, type GenreCohort, type GenreEvolutionPoint, type Track } from '$lib/api/client';
	import { wsMessages } from '$lib/api/ws';
	import { playTrackNow, setPlayerAutomixEnabled, setPlayerShuffleMode } from '$lib/stores/player';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import GenreGalaxy from '$lib/components/Genre/GenreGalaxy.svelte';
	import GenrePanel from '$lib/components/Genre/GenrePanel.svelte';
	import GenreInterior from '$lib/components/Genre/GenreInterior.svelte';
	import { buildGalaxyData } from '$lib/components/Genre/galaxyBuilder';
	import type { GalaxyViewMode } from '$lib/components/Genre/galaxy.types';

	let taxonomy = $state<Genre[]>([]);
	let heat = $state<GenreHeat[]>([]);
	let coOccurrences = $state<GenreCoOccurrence[]>([]);
	let cohorts = $state<GenreCohort[]>([]);
	let evolution = $state<GenreEvolutionPoint[]>([]);
	let selectedId = $state<number | null>(null);
	let loading = $state(true);
	let refreshingTopology = $state(false);
	let refreshingHeat = $state(false);
	let error = $state<string | null>(null);
	let actionError = $state<string | null>(null);
	let panelTracksById = $state<Record<number, Track[]>>({});
	let panelLoadingById = $state<Record<number, boolean>>({});
	let panelErrorById = $state<Record<number, string | null>>({});
	let artistChipMap = $state<Map<number, string[]>>(new Map());
	let loadingArtistNodes = $state<Set<number>>(new Set());
	let wsUnsubscribe: Unsubscriber | null = null;
	let galaxyRefreshTimer: ReturnType<typeof setTimeout> | null = null;
	let galaxyDailyRefreshTimer: ReturnType<typeof setInterval> | null = null;
	let pendingRefreshKind: 'heat' | 'full' | null = null;
	let viewMode = $state<GalaxyViewMode>('map');
	let labelsEnabled = $state(true);
	let heatEnabled = $state(true);
	let autoDrift = $state(true);
	let listeningDriven = $state(false);
	let showCohorts = $state(true);
	let showCoListening = $state(true);
	let searchQuery = $state('');
	let focusNodeId = $state<number | null>(null);
	let resetViewToken = $state(0);
	let selectedSeedIds = $state<number[]>([]);
	let interiorOpen = $state(false);
	const viewModes: GalaxyViewMode[] = ['map', 'constellations', 'mood', 'heat', 'paths'];

	let galaxyData = $derived(
		taxonomy.length > 0
			? buildGalaxyData(taxonomy, heat, {
					coOccurrences: showCoListening ? coOccurrences : [],
					cohorts: showCohorts ? cohorts : [],
					evolution,
					listeningDriven
				})
			: { nodes: [], edges: [] }
	);
	let heatById = $derived(new Map(heat.map((entry) => [entry.genre_id, entry])));
	let selectedNode = $derived(
		selectedId === null ? null : galaxyData.nodes.find((node) => node.id === selectedId) ?? null
	);
	let selectedHeat = $derived(selectedId === null ? null : heatById.get(selectedId) ?? null);
	let selectedTracks = $derived(selectedId === null ? [] : panelTracksById[selectedId] ?? []);
	let selectedTrackLoading = $derived(selectedId === null ? false : panelLoadingById[selectedId] ?? false);
	let selectedTrackError = $derived(selectedId === null ? null : panelErrorById[selectedId] ?? null);
	let hotGenreCount = $derived(heat.filter((entry) => entry.listen_count > 0).length);
	let noGenresLoaded = $derived(!loading && !error && galaxyData.nodes.length === 0);
	let flatTaxonomy = $derived(flattenGenres(taxonomy));
	let genreById = $derived(new Map(flatTaxonomy.map((genre) => [genre.id, genre])));
	let rootStats = $derived(
		taxonomy.map((root) => ({
			id: root.id,
			name: root.name,
			listens: heatById.get(root.id)?.listen_count ?? 0
		}))
	);
	let activeThisMonthCount = $derived(heat.filter((entry) => entry.listen_count > 0).length);
	let rediscoveryCount = $derived(
		flatTaxonomy.filter(
			(genre) => (genre.track_count ?? 0) > 0 && (heatById.get(genre.id)?.listen_count ?? 0) === 0
		).length
	);
	let liveBridgeCount = $derived(
		rootStats.filter((root) => root.listens > 0).length > 1
			? rootStats.filter((root) => root.listens > 0).length - 1
			: 0
	);
	let coListeningBridgeCount = $derived(
		coOccurrences.filter((p) => p.jaccard > 0.1).length
	);
	let activeCohortCount = $derived(
		cohorts.filter((c) => c.genre_ids.length > 0).length
	);
	let activeModeCopy = $derived.by(() => {
		if (listeningDriven) {
			switch (viewMode) {
				case 'constellations':
					return 'Your personal listening clusters, shaped by when and how you listen.';
				case 'mood':
					return 'Emotional field mapped to your taste gravity.';
				case 'heat':
					return 'Momentum halos — your current taste orbit.';
				case 'paths':
					return 'Co-listening bridges between genres you bridge in real life.';
				default:
					return 'Your personal genre cosmos — shaped by what you actually listen to.';
			}
		}
		switch (viewMode) {
			case 'constellations':
				return 'Editorial scene clusters and orbit guides.';
			case 'mood':
				return 'Emotional field overlay across the taxonomy.';
			case 'heat':
				return 'Momentum halos reflecting recent listening.';
			case 'paths':
				return 'Lineage routes and bridge emphasis.';
			default:
				return 'Canonical galaxy map of your library.';
		}
	});
	let selectedLineage = $derived.by(() => {
		if (!selectedNode) return [] as string[];
		const lineage: string[] = [];
		let current = genreById.get(selectedNode.id) ?? null;
		while (current) {
			lineage.unshift(current.name);
			current = current.parent_id === null ? null : genreById.get(current.parent_id) ?? null;
		}
		return lineage;
	});
	let selectedNodeCohort = $derived(
		selectedNode?.cohortId
			? cohorts.find((c) => c.id === selectedNode.cohortId)
			: null
	);
	let nearbyGenres = $derived.by(() => {
		if (!selectedNode) return [] as string[];
		const genre = genreById.get(selectedNode.id);
		if (!genre) return [] as string[];

		const siblingNames = genre.parent_id === null
			? taxonomy.filter((item) => item.id !== genre.id).map((item) => item.name)
			: (genreById.get(genre.parent_id)?.children ?? [])
				.filter((item) => item.id !== genre.id)
				.map((item) => item.name);
		const childNames = (genre.children ?? []).map((item) => item.name);
		return [...new Set([...childNames, ...siblingNames])].slice(0, 6);
	});

	function flattenGenres(nodes: Genre[]): Genre[] {
		const result: Genre[] = [];
		for (const node of nodes) {
			result.push(node, ...flattenGenres(node.children ?? []));
		}
		return result;
	}

	function buildZeroHeat(nodes: Genre[]): GenreHeat[] {
		return flattenGenres(nodes).map((genre) => ({
			genre_id: genre.id,
			genre_name: genre.name,
			listen_count: 0,
			total_listened_ms: 0
		}));
	}

	function isNotFoundError(reason: unknown): boolean {
		return reason instanceof Error && /\b404\b/.test(reason.message);
	}

	type GalaxySnapshot = {
		genres: Genre[];
		heat: GenreHeat[];
		coOccurrences: GenreCoOccurrence[];
		cohorts: GenreCohort[];
		evolution: GenreEvolutionPoint[];
	};

	async function fetchGalaxySnapshot(): Promise<GalaxySnapshot> {
		const genreResponse = await api.getGenres();
		const heatResponse = await api.getGenreHeat(90).catch((reason) => {
			if (isNotFoundError(reason)) {
				console.warn('Genre heat endpoint unavailable; rendering galaxy with zero heat.');
				return { heat: buildZeroHeat(genreResponse.genres) };
			}
			throw reason;
		});

		// Fetch new data in parallel — these are lightweight queries
		const [coOccurrencesResp, cohortsResp, evolutionResp] = await Promise.allSettled([
			api.getGenreCoOccurrence(90, 30, 3),
			api.getGenreCohorts(90),
			api.getGenreEvolution(90)
		]);

		return {
			genres: genreResponse.genres,
			heat: heatResponse.heat,
			coOccurrences: coOccurrencesResp.status === 'fulfilled' ? coOccurrencesResp.value.pairs : [],
			cohorts: cohortsResp.status === 'fulfilled' ? cohortsResp.value.cohorts : [],
			evolution: evolutionResp.status === 'fulfilled' ? evolutionResp.value.evolution : []
		};
	}

	function clearGalaxyCaches() {
		panelTracksById = {};
		panelLoadingById = {};
		panelErrorById = {};
		artistChipMap = new Map();
		loadingArtistNodes = new Set();
	}

	function applyGalaxySnapshot(
		snapshot: GalaxySnapshot,
		options: { invalidateCaches?: boolean } = {}
	) {
		const { invalidateCaches = false } = options;
		const nextIds = new Set(flattenGenres(snapshot.genres).map((genre) => genre.id));
		taxonomy = snapshot.genres;
		heat = snapshot.heat;
		coOccurrences = snapshot.coOccurrences;
		cohorts = snapshot.cohorts;
		evolution = snapshot.evolution;

		if (invalidateCaches) {
			clearGalaxyCaches();
		}

		if (selectedId !== null && !nextIds.has(selectedId)) {
			selectedId = null;
			focusNodeId = null;
		}

		if (selectedSeedIds.length > 0) {
			selectedSeedIds = selectedSeedIds.filter((id) => nextIds.has(id));
		}

		if (invalidateCaches && selectedId !== null) {
			void getOrLoadPanelTracks(selectedId);
		}
	}

	function collectTopArtists(tracks: Track[], seed: string[] = []): string[] {
		const counts = new Map<string, number>();
		for (const track of tracks) {
			const artist = track.artist_name?.trim();
			if (!artist) continue;
			counts.set(artist, (counts.get(artist) ?? 0) + 1);
		}

		const ordered = [...counts.entries()]
			.sort((left, right) => right[1] - left[1] || left[0].localeCompare(right[0]))
			.map(([artist]) => artist);

		const merged = [...seed];
		for (const artist of ordered) {
			if (merged.includes(artist)) continue;
			merged.push(artist);
			if (merged.length >= 3) break;
		}
		return merged;
	}

	function interleaveTrackLists(lists: Track[][]): Track[] {
		const queue: Track[] = [];
		const seen = new Set<number>();
		let index = 0;
		let madeProgress = true;

		while (madeProgress) {
			madeProgress = false;
			for (const list of lists) {
				const track = list[index];
				if (!track || seen.has(track.id)) continue;
				seen.add(track.id);
				queue.push(track);
				madeProgress = true;
			}
			index += 1;
		}

		return queue;
	}

	async function focusNode(nodeId: number | null) {
		focusNodeId = null;
		await tick();
		focusNodeId = nodeId;
	}

	async function loadGalaxy() {
		loading = true;
		error = null;
		actionError = null;
		try {
			const snapshot = await fetchGalaxySnapshot();
			applyGalaxySnapshot(snapshot, { invalidateCaches: true });
		} catch (reason) {
			error = reason instanceof Error ? reason.message : String(reason);
			taxonomy = [];
			heat = [];
			clearGalaxyCaches();
		} finally {
			loading = false;
		}
	}

	async function refreshGalaxyTopology() {
		refreshingTopology = true;
		try {
			const snapshot = await fetchGalaxySnapshot();
			applyGalaxySnapshot(snapshot, { invalidateCaches: true });
			error = null;
		} catch (reason) {
			console.error('Failed to refresh genre topology', reason);
		} finally {
			refreshingTopology = false;
		}
	}

	async function refreshHeat() {
		refreshingHeat = true;
		try {
			const [heatResp, coResp, cohortResp, evolResp] = await Promise.allSettled([
				api.getGenreHeat(90),
				api.getGenreCoOccurrence(90, 30, 3),
				api.getGenreCohorts(90),
				api.getGenreEvolution(90)
			]);
			if (heatResp.status === 'fulfilled') heat = heatResp.value.heat;
			else if (isNotFoundError(heatResp.reason)) heat = buildZeroHeat(taxonomy);
			if (coResp.status === 'fulfilled') coOccurrences = coResp.value.pairs;
			if (cohortResp.status === 'fulfilled') cohorts = cohortResp.value.cohorts;
			if (evolResp.status === 'fulfilled') evolution = evolResp.value.evolution;
		} catch (reason) {
			console.error('Failed to refresh galaxy data', reason);
		} finally {
			refreshingHeat = false;
		}
	}

	function scheduleGalaxyRefresh(kind: 'heat' | 'full') {
		pendingRefreshKind =
			kind === 'full' || pendingRefreshKind === 'full' ? 'full' : 'heat';
		if (galaxyRefreshTimer) clearTimeout(galaxyRefreshTimer);
		galaxyRefreshTimer = setTimeout(() => {
			const nextRefresh = pendingRefreshKind;
			pendingRefreshKind = null;
			if (nextRefresh === 'full') {
				void refreshGalaxyTopology();
				return;
			}
			void refreshHeat();
		}, kind === 'full' ? 360 : 280);
	}

	async function getOrLoadPanelTracks(id: number): Promise<Track[]> {
		if (panelTracksById[id]) return panelTracksById[id];
		if (panelLoadingById[id]) return [];

		panelLoadingById = { ...panelLoadingById, [id]: true };
		panelErrorById = { ...panelErrorById, [id]: null };

		try {
			const response = await api.getGenreTracks(id, true);
			panelTracksById = { ...panelTracksById, [id]: response.tracks };
			return response.tracks;
		} catch (reason) {
			const message = reason instanceof Error ? reason.message : String(reason);
			panelTracksById = { ...panelTracksById, [id]: [] };
			panelErrorById = { ...panelErrorById, [id]: message };
			return [];
		} finally {
			panelLoadingById = { ...panelLoadingById, [id]: false };
		}
	}

	function handleSelect(id: number | null) {
		selectedId = id;
		interiorOpen = false;
		actionError = null;
		if (id === null) {
			focusNodeId = null;
		}
		if (id !== null) {
			void getOrLoadPanelTracks(id);
		}
	}

	function toggleSeed(id: number) {
		if (selectedSeedIds.includes(id)) {
			selectedSeedIds = selectedSeedIds.filter((seedId) => seedId !== id);
			return;
		}
		selectedSeedIds =
			selectedSeedIds.length >= 3 ? [...selectedSeedIds.slice(1), id] : [...selectedSeedIds, id];
	}

	async function handleSearchSubmit(event?: Event) {
		event?.preventDefault();
		actionError = null;
		const query = searchQuery.trim().toLowerCase();
		if (!query) return;

		const match = galaxyData.nodes
			.slice()
			.sort((left, right) => left.depth - right.depth || left.name.localeCompare(right.name))
			.find(
				(node) =>
					node.name.toLowerCase() === query ||
					node.name.toLowerCase().startsWith(query) ||
					node.name.toLowerCase().includes(query)
			);

		if (!match) {
			actionError = `No mapped genre matched "${searchQuery.trim()}".`;
			return;
		}

		handleSelect(match.id);
		await focusNode(match.id);
	}

	async function handleJumpToFamily(familyId: number) {
		if (!familyId) return;
		handleSelect(familyId);
		await focusNode(familyId);
	}

	function handleBackOut() {
		if (selectedId !== null) {
			handleSelect(null);
			return;
		}
		resetViewToken += 1;
	}

	async function loadArtistChipsForFamily(familyId: number) {
		const depthOneNodes = galaxyData.nodes.filter(
			(node) => node.familyId === familyId && node.depth === 1
		);
		const pendingNodes = depthOneNodes.filter(
			(node) => !artistChipMap.has(node.id) && !loadingArtistNodes.has(node.id)
		);
		if (pendingNodes.length === 0) return;

		loadingArtistNodes = new Set([...loadingArtistNodes, ...pendingNodes.map((node) => node.id)]);

		await Promise.all(
			pendingNodes.map(async (node) => {
				try {
					const direct = await api.getGenreTracks(node.id, false);
					let artists = collectTopArtists(direct.tracks);
					if (artists.length < 3) {
						const descendants = await api.getGenreTracks(node.id, true);
						artists = collectTopArtists(descendants.tracks, artists);
					}
					artistChipMap = new Map(artistChipMap).set(node.id, artists.slice(0, 3));
				} catch (reason) {
					console.error(`Failed to load artist chips for genre ${node.id}`, reason);
					artistChipMap = new Map(artistChipMap).set(node.id, []);
				} finally {
					const nextLoading = new Set(loadingArtistNodes);
					nextLoading.delete(node.id);
					loadingArtistNodes = nextLoading;
				}
			})
		);
	}

	async function handleMix(id: number) {
		actionError = null;
		try {
			const tracks = await getOrLoadPanelTracks(id);
			if (tracks.length === 0) {
				actionError = 'This genre does not currently resolve to any tracks.';
				return;
			}

			await api.replacePlaybackQueue(tracks.map((track) => track.id));
			await setPlayerShuffleMode('genre');
			await setPlayerAutomixEnabled(true);
			const startIndex = Math.floor(Math.random() * tracks.length);
			await playTrackNow(tracks[startIndex].id);
		} catch (reason) {
			actionError = reason instanceof Error ? reason.message : String(reason);
		}
	}

	async function handleSeedMix() {
		actionError = null;
		if (selectedSeedIds.length === 0) return;
		try {
			const lists = await Promise.all(
				selectedSeedIds.map(async (id) => (await getOrLoadPanelTracks(id)).slice(0, 60))
			);
			const mergedTracks = interleaveTrackLists(lists);
			if (mergedTracks.length === 0) {
				actionError = 'The current seed blend does not resolve to any tracks.';
				return;
			}

			await api.replacePlaybackQueue(mergedTracks.map((track) => track.id));
			await setPlayerShuffleMode('genre');
			await setPlayerAutomixEnabled(true);
			const startIndex = Math.floor(Math.random() * mergedTracks.length);
			await playTrackNow(mergedTracks[startIndex].id);
		} catch (reason) {
			actionError = reason instanceof Error ? reason.message : String(reason);
		}
	}

	onMount(() => {
		wsUnsubscribe = wsMessages.subscribe((messages) => {
			const latest = messages.at(-1);
			if (!latest) return;
			if (latest.type === 'listen_history_updated') {
				scheduleGalaxyRefresh('heat');
				return;
			}
			if (latest.type === 'library_synced' || latest.type === 'musicbrainz_enriched') {
				scheduleGalaxyRefresh('full');
			}
		});

		// Daily auto-refresh to pick up new enrichment tags.
		galaxyDailyRefreshTimer = setInterval(() => {
			void refreshGalaxyTopology();
		}, 24 * 60 * 60 * 1000);

		void loadGalaxy();

		return () => {
			wsUnsubscribe?.();
			if (galaxyRefreshTimer) clearTimeout(galaxyRefreshTimer);
			if (galaxyDailyRefreshTimer) clearInterval(galaxyDailyRefreshTimer);
		};
	});
</script>

<svelte:head>
	<title>Genres | NOOR</title>
</svelte:head>

<div class="genres-route animate-in">
	<div class="galaxy-stage">
		{#if loading}
			<div class="state-overlay">
				<EmptyState title="Calibrating the genre observatory" copy="Mapping taxonomy topology, sampling 90-day momentum, and stabilizing constellation layout." />
			</div>
		{:else if error}
			<div class="state-overlay">
				<EmptyState title="Genre observatory unavailable" copy={error}>
					{#snippet actions()}
						<button class="btn btn-glass" onclick={() => void loadGalaxy()}>Retry</button>
					{/snippet}
				</EmptyState>
			</div>
		{:else if noGenresLoaded}
			<div class="state-overlay">
				<EmptyState title="No genres loaded yet" copy="The taxonomy seed needs to exist before the galaxy can render." />
			</div>
		{:else}
			<div class="galaxy-map-frame">
				<GenreGalaxy
					nodes={galaxyData.nodes}
					edges={galaxyData.edges}
					{selectedId}
					selectedSeedIds={selectedSeedIds}
					focusNodeId={focusNodeId}
					resetViewToken={resetViewToken}
					viewMode={viewMode}
					labelsEnabled={labelsEnabled}
					heatEnabled={heatEnabled}
					autoDrift={autoDrift}
					{artistChipMap}
					onSelect={handleSelect}
					onToggleSeed={toggleSeed}
					onMix={(id) => void handleMix(id)}
					onZoomFamily={(familyId) => void loadArtistChipsForFamily(familyId)}
					onEnterInterior={(id) => { handleSelect(id); interiorOpen = true; }}
				/>
			</div>

			<div class="hud glass-panel">
				<div class="hud-topline">
					<div>
						<h1>Genre Galaxy</h1>
						<p class="hud-copy">Navigate your library as a living genre cosmos.</p>
					</div>
					<span class="mode-chip">{viewMode}</span>
				</div>

				<div class="hud-status">
					<span>
						{refreshingTopology
							? 'Refreshing mapped genres and observatory signals...'
							: refreshingHeat
								? 'Refreshing observatory signals...'
								: activeModeCopy}
					</span>
				</div>

				<div class="hud-stats compact">
					<div><strong>{taxonomy.length}</strong><span>families</span></div>
					<div><strong>{galaxyData.nodes.length}</strong><span>mapped</span></div>
					<div><strong>{activeThisMonthCount}</strong><span>active</span></div>
					{#if listeningDriven}
						<div><strong>{activeCohortCount}</strong><span>clusters</span></div>
						<div><strong>{coListeningBridgeCount}</strong><span>bridges</span></div>
					{:else}
						<div><strong>{rediscoveryCount}</strong><span>rediscovery</span></div>
						<div><strong>{liveBridgeCount}</strong><span>bridges</span></div>
					{/if}
				</div>
			</div>

			<div class="mode-switcher glass-panel" role="tablist" aria-label="Galaxy views">
				{#each viewModes as mode}
					<button
						class:active={viewMode === mode}
						class="mode-btn"
						role="tab"
						aria-selected={viewMode === mode}
						onclick={() => (viewMode = mode)}
					>
						{mode}
					</button>
				{/each}
			</div>

			{#if actionError}
				<div class="error-toast glass-panel">{actionError}</div>
			{/if}

			<div class="control-dock glass-panel">
				<form class="search-shell" onsubmit={(event) => void handleSearchSubmit(event)}>
					<input
						type="search"
						bind:value={searchQuery}
						placeholder="Search genres"
						aria-label="Search mapped genres"
					/>
				</form>

				<select
					class="family-jump"
					onchange={(event) => void handleJumpToFamily(Number((event.currentTarget as HTMLSelectElement).value))}
					aria-label="Jump to family"
				>
					<option value="">Jump to family</option>
					{#each taxonomy as family}
						<option value={family.id}>{family.name}</option>
					{/each}
				</select>

				<button class="dock-btn" onclick={handleBackOut}>Back</button>
				<button class="dock-btn" onclick={() => (resetViewToken += 1)}>Center map</button>
				<button class:active={listeningDriven} class="dock-btn" onclick={() => (listeningDriven = !listeningDriven)}>
					Gravity mode
				</button>
				<button class:active={showCohorts} class="dock-btn" onclick={() => (showCohorts = !showCohorts)}>
					Clusters
				</button>
				<button class:active={showCoListening} class="dock-btn" onclick={() => (showCoListening = !showCoListening)}>
					Bridges
				</button>
				<button class:active={labelsEnabled} class="dock-btn" onclick={() => (labelsEnabled = !labelsEnabled)}>
					Labels
				</button>
				<button class:active={heatEnabled} class="dock-btn" onclick={() => (heatEnabled = !heatEnabled)}>
					Heat
				</button>
				<button class:active={autoDrift} class="dock-btn" onclick={() => (autoDrift = !autoDrift)}>
					Auto drift
				</button>
			</div>

			{#if selectedSeedIds.length > 0}
				<div class="seed-builder glass-panel">
					<div class="seed-copy">
						<p class="eyebrow">Seed Mix Builder</p>
						<span>{selectedSeedIds.length} seed{selectedSeedIds.length === 1 ? '' : 's'} locked</span>
					</div>
					<div class="seed-chips">
						{#each selectedSeedIds as seedId}
							{@const seedNode = galaxyData.nodes.find((node) => node.id === seedId)}
							{#if seedNode}
								<button class="seed-chip" onclick={() => toggleSeed(seedId)}>{seedNode.name}</button>
							{/if}
						{/each}
					</div>
					<button class="btn btn-primary seed-mix-btn" onclick={() => void handleSeedMix()}>
						▶ Build seed mix
					</button>
				</div>
			{/if}

			<GenrePanel
				node={selectedNode}
				listenHeat={selectedHeat}
				tracks={selectedTracks}
				lineage={selectedLineage}
				nearbyGenres={nearbyGenres}
				isSeed={selectedNode !== null && selectedSeedIds.includes(selectedNode.id)}
				loading={selectedTrackLoading}
				error={selectedTrackError}
				open={selectedNode !== null && !interiorOpen}
				onClose={() => handleSelect(null)}
				onMix={() => selectedNode && void handleMix(selectedNode.id)}
				onToggleSeed={() => selectedNode && toggleSeed(selectedNode.id)}
			/>

			{#if interiorOpen && selectedNode}
				<GenreInterior
					node={selectedNode}
					heat={selectedHeat}
					cohortLabel={selectedNodeCohort?.label ?? null}
					onClose={() => (interiorOpen = false)}
					onPlayMix={() => selectedNode && void handleMix(selectedNode.id)}
				/>
			{/if}
		{/if}
	</div>
</div>

<style>
	.genres-route {
		position: relative;
		margin: -28px -30px -48px;
		min-height: 100vh;
		overflow: hidden;
		background:
			radial-gradient(circle at 16% 12%, var(--atlas-haze-a), transparent 34%),
			radial-gradient(circle at 82% 18%, var(--atlas-haze-b), transparent 30%),
			radial-gradient(circle at 74% 84%, var(--atlas-haze-c), transparent 34%),
			var(--atlas-bg);
	}

	.galaxy-stage {
		position: relative;
		min-height: 100vh;
		overflow: hidden;
	}

	.galaxy-map-frame {
		position: absolute;
		inset: 0;
	}

	.state-overlay {
		position: absolute;
		inset: 0;
		display: grid;
		place-items: center;
		padding: 32px;
		z-index: 9;
		background:
			radial-gradient(circle at 20% 20%, var(--atlas-haze-a), transparent 32%),
			linear-gradient(180deg, rgba(8, 10, 18, 0.92), rgba(6, 7, 14, 0.96));
	}

	.state-overlay :global(.empty-state) {
		width: min(100%, 620px);
	}

	.hud,
	.mode-switcher,
	.control-dock,
	.seed-builder,
	.error-toast {
		position: absolute;
		z-index: 6;
	}

	.hud {
		top: 20px;
		left: 20px;
		width: min(480px, calc(100% - 40px));
		padding: 12px 14px;
		display: flex;
		flex-direction: column;
		gap: 10px;
		border-color: color-mix(in srgb, var(--instrument-border) 72%, transparent);
		background:
			linear-gradient(180deg, color-mix(in srgb, var(--instrument-surface-strong) 42%, transparent), color-mix(in srgb, var(--instrument-surface) 34%, transparent)),
			var(--panel-bg);
		box-shadow:
			0 16px 44px rgba(0, 0, 0, 0.28),
			inset 0 1px 0 color-mix(in srgb, var(--instrument-edge) 32%, transparent);
	}

	.hud h1 {
		font-size: clamp(1.35rem, 2vw, 1.8rem);
		line-height: 0.98;
	}

	.hud-topline {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: 12px;
	}

	.mode-chip {
		padding: 5px 10px;
		border-radius: 999px;
		border: 1px solid color-mix(in srgb, var(--instrument-border) 58%, transparent);
		background: color-mix(in srgb, var(--instrument-surface-strong) 70%, transparent);
		color: var(--text-primary);
		font-size: 0.7rem;
		font-weight: 700;
		letter-spacing: 0.1em;
		text-transform: uppercase;
		white-space: nowrap;
	}

	.hud-copy,
	.hud-status span {
		color: var(--signal-text);
	}

	.hud-copy {
		font-size: 0.83rem;
	}

	.hud-stats {
		display: grid;
		grid-template-columns: repeat(5, minmax(0, 1fr));
		gap: 8px;
	}

	.hud-stats:has(> :nth-child(6)) {
		grid-template-columns: repeat(6, minmax(0, 1fr));
	}

	.hud-stats div {
		display: flex;
		align-items: baseline;
		gap: 6px;
		padding: 6px 8px;
		border-radius: 999px;
		background: color-mix(in srgb, var(--instrument-surface) 62%, transparent);
		border: 1px solid color-mix(in srgb, var(--instrument-border) 36%, transparent);
		justify-content: center;
	}

	.hud-stats strong {
		font-size: 0.96rem;
		font-family: var(--font-display);
	}

	.hud-stats span {
		color: var(--signal-text);
		font-size: 0.62rem;
		text-transform: uppercase;
		letter-spacing: 0.1em;
	}

	.hud-status {
		padding-top: 2px;
	}

	.mode-switcher {
		top: 20px;
		right: 20px;
		padding: 8px;
		display: inline-flex;
		align-items: center;
		gap: 6px;
		border-color: color-mix(in srgb, var(--instrument-border) 66%, transparent);
		background:
			linear-gradient(180deg, color-mix(in srgb, var(--instrument-surface) 78%, transparent), color-mix(in srgb, var(--instrument-surface-strong) 80%, transparent)),
			var(--panel-bg);
	}

	.mode-btn,
	.dock-btn,
	.seed-chip {
		padding: 8px 12px;
		border-radius: 999px;
		border: 1px solid color-mix(in srgb, var(--instrument-border) 46%, transparent);
		background: color-mix(in srgb, var(--instrument-surface) 72%, transparent);
		color: var(--signal-text);
		transition:
			background var(--motion-fast),
			border-color var(--motion-fast),
			color var(--motion-fast),
			transform var(--motion-fast);
	}

	.mode-btn {
		font-size: 0.7rem;
		text-transform: uppercase;
		letter-spacing: 0.08em;
	}

	.mode-btn.active,
	.dock-btn.active,
	.seed-chip {
		background: color-mix(in srgb, var(--accent-soft) 78%, var(--instrument-surface));
		border-color: color-mix(in srgb, var(--accent-line) 92%, transparent);
		color: var(--text-primary);
		box-shadow: 0 0 18px color-mix(in srgb, var(--accent-glow) 34%, transparent);
	}

	.mode-btn:hover,
	.dock-btn:hover,
	.seed-chip:hover {
		transform: translateY(-1px);
	}

	.control-dock {
		left: 50%;
		bottom: 20px;
		transform: translateX(-50%);
		padding: 10px;
		display: flex;
		align-items: center;
		gap: 8px;
		flex-wrap: wrap;
		width: min(960px, calc(100% - 40px));
		justify-content: center;
	}

	.search-shell {
		flex: 1 1 220px;
		min-width: 180px;
	}

	.search-shell input,
	.family-jump {
		height: 40px;
	}

	.family-jump {
		width: 180px;
	}

	.seed-builder {
		left: 50%;
		bottom: 86px;
		transform: translateX(-50%);
		width: min(720px, calc(100% - 40px));
		padding: 12px 14px;
		display: flex;
		align-items: center;
		gap: 12px;
		flex-wrap: wrap;
		justify-content: space-between;
	}

	.seed-copy {
		display: flex;
		flex-direction: column;
		gap: 2px;
	}

	.seed-copy span {
		color: var(--signal-text);
		font-size: 0.8rem;
	}

	.seed-chips {
		display: flex;
		align-items: center;
		gap: 8px;
		flex-wrap: wrap;
		flex: 1;
	}

	.seed-chip {
		font-size: 0.72rem;
	}

	.seed-mix-btn {
		flex-shrink: 0;
	}

	.error-toast {
		top: 74px;
		left: 20px;
		padding: 10px 12px;
		color: var(--state-error);
		background: rgba(28, 10, 16, 0.88);
		border-color: color-mix(in srgb, var(--state-error) 28%, transparent);
	}

	@media (max-width: 1180px) {
		.hud {
			width: min(420px, calc(100% - 40px));
		}
	}

	@media (max-width: 1180px) {
		.genres-route {
			margin: -24px -24px -40px;
		}

		.galaxy-stage,
		.genres-route {
			min-height: calc(100dvh - 40px);
		}

		.hud {
			top: 16px;
			left: 16px;
			width: min(100%, calc(100% - 32px));
		}

		.mode-switcher {
			top: auto;
			right: 16px;
			bottom: 120px;
		}

		.control-dock,
		.seed-builder {
			left: 16px;
			right: 16px;
			transform: none;
			width: auto;
			bottom: 16px;
		}

		.seed-builder {
			bottom: 92px;
		}
	}

	@media (max-width: 760px) {
		.genres-route {
			margin: -22px -18px -30px;
			overflow: visible;
			background: linear-gradient(180deg, #0d0e15 0%, #090a11 52%, #07070b 100%);
		}

		.galaxy-stage {
			min-height: auto;
			display: flex;
			flex-direction: column;
			gap: 14px;
			overflow: visible;
			padding-bottom: 10px;
		}

		.galaxy-map-frame {
			position: relative;
			inset: auto;
			order: 2;
			height: min(56dvh, 460px);
			min-height: 340px;
		}

		.hud,
		.mode-switcher,
		.control-dock,
		.seed-builder,
		.error-toast {
			position: relative;
			inset: auto;
			left: auto;
			right: auto;
			top: auto;
			bottom: auto;
			transform: none;
			width: 100%;
		}

		.hud {
			order: 1;
			gap: 12px;
			padding: 14px;
		}

		.hud-stats {
			grid-template-columns: repeat(2, minmax(0, 1fr));
		}

		.mode-switcher {
			order: 3;
			overflow-x: auto;
			justify-content: flex-start;
		}

		.control-dock {
			order: 4;
			justify-content: flex-start;
			padding: 12px;
		}

		.search-shell {
			flex-basis: 100%;
			min-width: 100%;
		}

		.family-jump {
			width: 100%;
		}

		.seed-builder {
			order: 5;
			align-items: flex-start;
		}

		.seed-chips {
			width: 100%;
		}
	}
</style>
