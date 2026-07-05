<script lang="ts">
	import { onMount, tick } from 'svelte';
	import type { Unsubscriber } from 'svelte/store';
	import { api, type Genre, type GenreHeat, type GenreCohort, type GenreEvolutionPoint, type GenreAudioMetrics, type Track } from '$lib/api/client';
	import { cachedApi } from '$lib/cache/api_queries';
	import { wsMessages } from '$lib/api/ws';
	import { playTrackNow, setPlayerAutomixEnabled, setPlayerShuffleMode, startGenreRadio } from '$lib/stores/player';
	import type { RadioBlend } from '$lib/api/client';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import GenreGalaxy from '$lib/components/Genre/GenreGalaxy.svelte';
	import GenrePanel from '$lib/components/Genre/GenrePanel.svelte';
	import GenreInterior from '$lib/components/Genre/GenreInterior.svelte';
	import { buildGalaxyData } from '$lib/components/Genre/galaxyBuilder';
	import type { GalaxyViewMode, GalaxyNode } from '$lib/components/Genre/galaxy.types';

	let taxonomy = $state<Genre[]>([]);
	let heat = $state<GenreHeat[]>([]);
	let cohorts = $state<GenreCohort[]>([]);
	let evolution = $state<GenreEvolutionPoint[]>([]);
	let metrics = $state<GenreAudioMetrics[]>([]);
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
	let autoDrift = $state(true);
	let libraryOnly = $state(true);
	let searchQuery = $state('');
	let focusNodeId = $state<number | null>(null);
	let resetViewToken = $state(0);
	let selectedSeedIds = $state<number[]>([]);
	let interiorOpen = $state(false);
	const viewModes: GalaxyViewMode[] = ['map', 'heat', 'vibe', 'rediscover'];

	// Prune to subtrees that actually contain tracks. The default is on because
	// pure-taxonomy nodes (no tracks in your library, even transitively) are
	// decorative noise — you can't play them and they crowd the canvas.
	function pruneToLibrary(nodes: Genre[]): Genre[] {
		const result: Genre[] = [];
		for (const node of nodes) {
			const prunedChildren = pruneToLibrary(node.children ?? []);
			const subtreeHasTracks = (node.track_count ?? 0) > 0 || prunedChildren.length > 0;
			if (subtreeHasTracks) {
				result.push({ ...node, children: prunedChildren });
			}
		}
		return result;
	}

	let displayedTaxonomy = $derived(libraryOnly ? pruneToLibrary(taxonomy) : taxonomy);

	let galaxyData = $derived(
		displayedTaxonomy.length > 0
			? buildGalaxyData(displayedTaxonomy, heat, { cohorts, evolution, metrics })
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
	let noGenresLoaded = $derived(!loading && !error && galaxyData.nodes.length === 0);
	let flatTaxonomy = $derived(flattenGenres(taxonomy));
	let genreById = $derived(new Map(flatTaxonomy.map((genre) => [genre.id, genre])));
	let activeThisMonthCount = $derived(heat.filter((entry) => entry.listen_count > 0).length);
	let rediscoveryCount = $derived(
		flatTaxonomy.filter(
			(genre) => (genre.track_count ?? 0) > 0 && (heatById.get(genre.id)?.listen_count ?? 0) === 0
		).length
	);
	let activeModeCopy = $derived.by(() => {
		switch (viewMode) {
			case 'heat': return 'Recent listening heat.';
			case 'vibe': return 'Energy, BPM, and dance.';
			case 'rediscover': return 'Stocked but unplayed.';
			default: return 'Canonical library map.';
		}
	});
	let selectedNodeCohort = $derived(
		selectedNode?.cohortId
			? cohorts.find((c) => c.id === selectedNode.cohortId)
			: null
	);
	type NearbyEntry = { id: number; name: string };
	let searchHighlightIds = $derived.by<Set<number>>(() => {
		const query = searchQuery.trim().toLowerCase();
		if (!query) return new Set();
		const ids = new Set<number>();
		const nodeMap = new Map(galaxyData.nodes.map((node) => [node.id, node]));
		for (const node of galaxyData.nodes) {
			if (!node.name.toLowerCase().includes(query)) continue;
			let cursor: typeof node | undefined = node;
			while (cursor) {
				if (ids.has(cursor.id)) break;
				ids.add(cursor.id);
				cursor = cursor.parentId === null ? undefined : nodeMap.get(cursor.parentId);
			}
		}
		return ids;
	});

	let nearbyGenres = $derived.by<NearbyEntry[]>(() => {
		if (!selectedNode) return [];
		const genre = genreById.get(selectedNode.id);
		if (!genre) return [];

		const siblings: NearbyEntry[] = genre.parent_id === null
			? taxonomy.filter((item) => item.id !== genre.id).map((item) => ({ id: item.id, name: item.name }))
			: (genreById.get(genre.parent_id)?.children ?? [])
				.filter((item) => item.id !== genre.id)
				.map((item) => ({ id: item.id, name: item.name }));
		const children: NearbyEntry[] = (genre.children ?? []).map((item) => ({ id: item.id, name: item.name }));
		const seen = new Set<number>();
		const merged: NearbyEntry[] = [];
		for (const entry of [...children, ...siblings]) {
			if (seen.has(entry.id)) continue;
			seen.add(entry.id);
			merged.push(entry);
			if (merged.length >= 6) break;
		}
		return merged;
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
		cohorts: GenreCohort[];
		evolution: GenreEvolutionPoint[];
		metrics: GenreAudioMetrics[];
	};

	async function fetchGalaxySnapshot(): Promise<GalaxySnapshot> {
		return cachedApi.getGenreGalaxySnapshot(90);
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
		cohorts = snapshot.cohorts;
		evolution = snapshot.evolution;
		metrics = snapshot.metrics;

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
		// Only show the skeleton when nothing is painted yet. When we've seeded from
		// the persisted snapshot, revalidate quietly instead of flashing a spinner.
		if (taxonomy.length === 0) loading = true;
		error = null;
		actionError = null;
		try {
			const snapshot = await fetchGalaxySnapshot();
			applyGalaxySnapshot(snapshot, { invalidateCaches: true });
		} catch (reason) {
			error = reason instanceof Error ? reason.message : String(reason);
			taxonomy = [];
			heat = [];
			cohorts = [];
			evolution = [];
			metrics = [];
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
			const [heatResp, cohortResp, evolResp, metricsResp] = await Promise.allSettled([
				cachedApi.getGenreHeat(90),
				cachedApi.getGenreCohorts(90),
				cachedApi.getGenreEvolution(90),
				cachedApi.getGenreAudioMetrics()
			]);
			if (heatResp.status === 'fulfilled') heat = heatResp.value.heat;
			else if (isNotFoundError(heatResp.reason)) heat = buildZeroHeat(taxonomy);
			if (cohortResp.status === 'fulfilled') cohorts = cohortResp.value.cohorts;
			if (evolResp.status === 'fulfilled') evolution = evolResp.value.evolution;
			if (metricsResp.status === 'fulfilled') metrics = metricsResp.value.metrics;
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
			const response = await cachedApi.getGenreTracks(id, true);
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
		actionNotice = null;
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
					const direct = await cachedApi.getGenreTracks(node.id, false);
					let artists = collectTopArtists(direct.tracks);
					if (artists.length < 3) {
						const descendants = await cachedApi.getGenreTracks(node.id, true);
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

	// "Mix"/"Start mix" for one genre: seed a mixed radio from a representative
	// track so it plays as a continuous station, not a static (often unplayable)
	// queue that stalls on the seed.
	async function handleMix(id: number) {
		actionError = null;
		try {
			const tracks = await getOrLoadPanelTracks(id);
			const seed = pickSeedTrackId(tracks);
			if (seed == null) {
				actionError = 'This genre does not currently resolve to any playable tracks.';
				return;
			}
			const label = galaxyData.nodes.find((node) => node.id === id)?.name ?? 'Genre';
			await startGenreRadio(seed, 'mixed', label);
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
			const shuffled = await setPlayerShuffleMode('genre');
			await setPlayerAutomixEnabled(true);
			await playTrackNow(shuffled?.queue[0]?.track.id ?? mergedTracks[0].id);
		} catch (reason) {
			actionError = reason instanceof Error ? reason.message : String(reason);
		}
	}

	// --- Mode actions: heat / vibe / rediscover become playable -------------
	let modeActionBusy = $state(false);
	let actionNotice = $state<string | null>(null);
	let noticeTimer: ReturnType<typeof setTimeout> | null = null;

	function showNotice(message: string) {
		actionNotice = message;
		if (noticeTimer) clearTimeout(noticeTimer);
		noticeTimer = setTimeout(() => (actionNotice = null), 6000);
	}

	let rediscoverCandidates = $derived(
		galaxyData.nodes.filter((node) => node.trackCount > 0 && node.listenCount === 0)
	);
	let hottestNodes = $derived(
		galaxyData.nodes
			.filter((node) => node.listenCount > 0 && node.trackCount > 0)
			.sort((left, right) => right.listenCount - left.listenCount)
	);

	function subtreeCandidateIds(rootId: number): number[] {
		const byId = new Map(galaxyData.nodes.map((node) => [node.id, node]));
		const ids: number[] = [];
		for (const candidate of rediscoverCandidates) {
			let cursor = byId.get(candidate.id);
			while (cursor) {
				if (cursor.id === rootId) {
					ids.push(candidate.id);
					break;
				}
				cursor = cursor.parentId === null ? undefined : byId.get(cursor.parentId);
			}
		}
		return ids;
	}

	function randomItem<T>(items: T[]): T | undefined {
		if (items.length === 0) return undefined;
		return items[Math.floor(Math.random() * items.length)];
	}

	function shuffled<T>(items: T[]): T[] {
		const copy = items.slice();
		for (let i = copy.length - 1; i > 0; i -= 1) {
			const j = Math.floor(Math.random() * (i + 1));
			[copy[i], copy[j]] = [copy[j], copy[i]];
		}
		return copy;
	}

	// Pick a RANDOM library track to seed radio from, not always the same
	// most-played row - a fixed seed made the orchestrator grow the same station
	// every launch. Random seed -> a fresh station each time.
	function pickSeedTrackId(tracks: Track[]): number | null {
		const playable = tracks.filter((track) => track.id > 0);
		return randomItem(playable)?.id ?? null;
	}

	async function seedTrackForGenre(genreId: number, includeDescendants: boolean): Promise<number | null> {
		const { tracks } = await cachedApi.getGenreTracks(genreId, includeDescendants);
		return pickSeedTrackId(tracks);
	}

	// Walk a list of genres, seeding from the first that resolves to a real
	// track. Callers shuffle the list first so the seed genre also varies.
	async function firstSeed(nodes: GalaxyNode[], includeDescendants: boolean): Promise<[number, string] | null> {
		for (const node of nodes) {
			const seed = await seedTrackForGenre(node.id, includeDescendants);
			if (seed != null) return [seed, node.name];
		}
		return null;
	}

	// Rediscover: seed an ADVENTUROUS radio from an unplayed genre (trackCount > 0,
	// zero listens - the same rule the canvas highlights). Surfaces the forgotten
	// track plus related finds, instead of looping one static row.
	async function playRediscover() {
		actionError = null;
		const scopeIds =
			selectedId !== null ? subtreeCandidateIds(selectedId) : rediscoverCandidates.map((node) => node.id);
		if (scopeIds.length === 0) {
			actionError =
				selectedId !== null
					? 'No unplayed genres inside this selection.'
					: 'No rediscover candidates right now.';
			return;
		}
		modeActionBusy = true;
		try {
			const byId = new Map(galaxyData.nodes.map((node) => [node.id, node]));
			// Bias to the bigger unplayed genres, then shuffle so the seed varies.
			const ranked = scopeIds
				.map((id) => byId.get(id))
				.filter((node): node is GalaxyNode => node !== undefined)
				.sort((left, right) => right.trackCount - left.trackCount)
				.slice(0, 12);
			const seed = await firstSeed(shuffled(ranked), false);
			if (!seed) {
				actionError = 'Rediscover candidates resolved to no playable tracks.';
				return;
			}
			await startGenreRadio(seed[0], 'adventurous', 'Rediscover');
		} catch (reason) {
			actionError = reason instanceof Error ? reason.message : String(reason);
		} finally {
			modeActionBusy = false;
		}
	}

	// Heat: seed a FAMILIAR radio from your hottest genre - a comfortable
	// continuous rotation, not a fixed list.
	async function playHottest() {
		actionError = null;
		if (hottestNodes.length === 0) {
			actionError = 'No listening heat yet - play something first.';
			return;
		}
		modeActionBusy = true;
		try {
			// Shuffle the top hot genres so it doesn't always seed from the #1.
			const seed = await firstSeed(shuffled(hottestNodes.slice(0, 8)), false);
			if (!seed) {
				actionError = 'Your hottest genres resolved to no playable tracks.';
				return;
			}
			await startGenreRadio(seed[0], 'familiar', 'Hottest');
		} catch (reason) {
			actionError = reason instanceof Error ? reason.message : String(reason);
		} finally {
			modeActionBusy = false;
		}
	}

	// Heat -> playlist: snapshot a static rotation of your hottest genres into a
	// real playlist via the queue (createPlaylistFromQueue). A playlist IS a
	// fixed artifact, so this stays a concrete list (unlike the radio actions).
	// Live smart-rule persistence is a follow-up.
	async function saveHeatPlaylist() {
		actionError = null;
		modeActionBusy = true;
		try {
			const top = hottestNodes.slice(0, 8);
			const lists = await Promise.all(
				top.map(async (node) => (await cachedApi.getGenreTracks(node.id, false)).tracks.slice(0, 60))
			);
			const merged = interleaveTrackLists(lists);
			if (merged.length === 0) {
				actionError = 'No listening heat yet - nothing to save.';
				return;
			}
			await api.replacePlaybackQueue(merged.map((track) => track.id));
			const name = `Hot rotation ${new Date().toISOString().slice(0, 10)}`;
			await api.createPlaylistFromQueue(name, true);
			showNotice(`Saved "${name}" (${merged.length} tracks). Queue now holds the same mix.`);
		} catch (reason) {
			actionError = reason instanceof Error ? reason.message : String(reason);
		} finally {
			modeActionBusy = false;
		}
	}

	// Instant-paint: seed the galaxy from the persisted snapshot so the page renders
	// last-known topology immediately instead of a spinner. loadGalaxy() in onMount
	// still revalidates in the background.
	{
		const seeded = cachedApi.genreGalaxySnapshotQuery(90).getSnapshot().data;
		if (seeded) {
			applyGalaxySnapshot(seeded);
			loading = false;
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
			if (noticeTimer) clearTimeout(noticeTimer);
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
				<EmptyState title="Loading genres" copy="Mapping taxonomy and recent heat." />
			</div>
		{:else if error}
			<div class="state-overlay">
				<EmptyState title="Genres unavailable" copy={error}>
					{#snippet actions()}
						<button class="btn btn-glass" onclick={() => void loadGalaxy()}>Retry</button>
					{/snippet}
				</EmptyState>
			</div>
		{:else if noGenresLoaded}
			<div class="state-overlay">
				<EmptyState title="No genres loaded yet" copy="Run genre enrichment to seed the map." />
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
					autoDrift={autoDrift}
					searchHighlightIds={searchHighlightIds}
					{artistChipMap}
					onSelect={handleSelect}
					onToggleSeed={toggleSeed}
					onMix={(id) => void handleMix(id)}
					onZoomFamily={(familyId) => void loadArtistChipsForFamily(familyId)}
					onEnterInterior={(id) => { handleSelect(id); interiorOpen = true; }}
				/>
			</div>

			<div class="hud glass-panel" aria-label="Galaxy summary">
				<div class="hud-topline">
					<span class="hud-card-title">Genre Galaxy</span>
					<span class="mode-chip">{viewMode}</span>
				</div>
				<p class="hud-status">
					{refreshingTopology
						? 'Refreshing mapped genres...'
						: refreshingHeat
							? 'Refreshing heat...'
							: activeModeCopy}
				</p>
				<p class="hud-meta-line">
					<strong>{taxonomy.length}</strong> families
					<span>{galaxyData.nodes.length}</span> genres
					<span>{activeThisMonthCount}</span> active
					<span>{rediscoveryCount}</span> rediscover
				</p>
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

			{#if viewMode === 'heat' || viewMode === 'rediscover'}
				<div class="mode-actions glass-panel" aria-label="Mode actions">
					{#if viewMode === 'rediscover'}
						<button
							class="btn btn-primary"
							disabled={modeActionBusy || rediscoverCandidates.length === 0}
							onclick={() => void playRediscover()}
						>
							{modeActionBusy
								? 'Building mix...'
								: selectedId !== null
									? 'Play rediscover in selection'
									: `Play rediscover (${rediscoverCandidates.length} genres)`}
						</button>
					{:else if viewMode === 'heat'}
						<button
							class="btn btn-primary"
							disabled={modeActionBusy || hottestNodes.length === 0}
							onclick={() => void playHottest()}
						>
							{modeActionBusy ? 'Building mix...' : 'Play hottest'}
						</button>
						<button
							class="btn btn-glass"
							disabled={modeActionBusy || hottestNodes.length === 0}
							onclick={() => void saveHeatPlaylist()}
						>
							Save as playlist
						</button>
					{/if}
				</div>
			{/if}

			{#if actionError}
				<div class="error-toast glass-panel" role="status" aria-live="polite">{actionError}</div>
			{/if}

			{#if actionNotice}
				<div class="notice-toast glass-panel" role="status" aria-live="polite">{actionNotice}</div>
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

				<button class:active={labelsEnabled} class="dock-btn" onclick={() => (labelsEnabled = !labelsEnabled)}>
					Labels
				</button>
				<button class:active={autoDrift} class="dock-btn" onclick={() => (autoDrift = !autoDrift)}>
					Auto drift
				</button>
				<button
					class:active={libraryOnly}
					class="dock-btn"
					onclick={() => (libraryOnly = !libraryOnly)}
					title="Hide genres with no tracks in your library"
				>
					Library only
				</button>
				<a class="dock-link" href="/tidal/genres">TIDAL genres</a>
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
					<div class="seed-actions">
						<button
							class="btn btn-glass"
							onclick={() => (selectedSeedIds = [])}
							aria-label="Clear all seeds"
						>
							Clear
						</button>
						<button class="btn btn-primary seed-mix-btn" onclick={() => void handleSeedMix()}>
							▶ Build seed mix
						</button>
					</div>
				</div>
			{/if}

			<GenrePanel
				node={selectedNode}
				listenHeat={selectedHeat}
				tracks={selectedTracks}
				nearbyGenres={nearbyGenres}
				isSeed={selectedNode !== null && selectedSeedIds.includes(selectedNode.id)}
				loading={selectedTrackLoading}
				error={selectedTrackError}
				open={selectedNode !== null && !interiorOpen}
				onClose={() => handleSelect(null)}
				onMix={() => selectedNode && void handleMix(selectedNode.id)}
				onToggleSeed={() => selectedNode && toggleSeed(selectedNode.id)}
				onOpenInterior={() => { if (selectedNode) interiorOpen = true; }}
				onSelectNearby={(id) => { handleSelect(id); void focusNode(id); }}
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
	.mode-actions,
	.control-dock,
	.seed-builder,
	.error-toast,
	.notice-toast {
		position: absolute;
		z-index: 6;
	}

	.mode-actions {
		/* Bottom-center, above the control dock: the genre panel opens on the
		   right and was burying the action right after the user armed it. */
		left: 50%;
		bottom: 78px;
		transform: translateX(-50%);
		padding: 8px;
		display: inline-flex;
		align-items: center;
		gap: 8px;
	}

	/* Seed builder shares that slot; lift it when a mode action bar is up. */
	.galaxy-stage:has(.mode-actions) .seed-builder {
		bottom: 150px;
	}

	.mode-actions .btn:disabled {
		opacity: 0.45;
		cursor: not-allowed;
	}

	.notice-toast {
		top: 74px;
		left: 20px;
		padding: 10px 12px;
		max-width: min(340px, calc(100% - 40px));
		color: var(--text-primary);
		background: color-mix(in srgb, var(--accent-soft) 30%, var(--panel-bg));
		border-color: color-mix(in srgb, var(--accent-line) 55%, transparent);
		font-size: var(--font-size-xs);
	}

	.hud {
		top: 20px;
		left: 20px;
		width: min(292px, calc(100% - 40px));
		padding: 10px 12px;
		display: flex;
		flex-direction: column;
		gap: 7px;
		border-color: color-mix(in srgb, var(--instrument-border) 72%, transparent);
		background:
			linear-gradient(180deg, color-mix(in srgb, var(--instrument-surface-strong) 38%, transparent), color-mix(in srgb, var(--instrument-surface) 26%, transparent)),
			var(--panel-bg);
		box-shadow:
			0 12px 32px rgba(0, 0, 0, 0.24),
			inset 0 1px 0 color-mix(in srgb, var(--instrument-edge) 28%, transparent);
	}

	.hud-topline {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 10px;
	}

	.hud-card-title {
		color: var(--text-primary);
		font-family: var(--font-display);
		font-size: var(--font-size-lg);
		font-weight: var(--font-weight-bold);
		line-height: var(--line-height-tight);
	}

	.mode-chip {
		padding: 4px 8px;
		border-radius: 999px;
		border: 1px solid color-mix(in srgb, var(--instrument-border) 58%, transparent);
		background: color-mix(in srgb, var(--instrument-surface-strong) 70%, transparent);
		color: var(--text-primary);
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-bold);
		letter-spacing: 0.1em;
		text-transform: uppercase;
		white-space: nowrap;
	}

	.hud-status,
	.hud-meta-line {
		margin: 0;
		color: var(--signal-text);
	}

	.hud-status {
		font-size: var(--font-size-xs);
		line-height: var(--line-height-snug);
	}

	.hud-meta-line {
		display: flex;
		align-items: center;
		gap: 7px;
		flex-wrap: wrap;
		font-size: var(--font-size-2xs);
		text-transform: uppercase;
		letter-spacing: 0.08em;
	}

	.hud-meta-line strong,
	.hud-meta-line span {
		color: var(--text-primary);
		font-size: var(--font-size-xs);
		font-variant-numeric: tabular-nums;
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
	.dock-link,
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
		font-size: var(--font-size-xs);
		text-transform: uppercase;
		letter-spacing: 0.08em;
	}

	.mode-btn.active,
	.dock-btn.active,
	.dock-link,
	.seed-chip {
		background: color-mix(in srgb, var(--accent-soft) 78%, var(--instrument-surface));
		border-color: color-mix(in srgb, var(--accent-line) 92%, transparent);
		color: var(--text-primary);
		box-shadow: 0 0 18px color-mix(in srgb, var(--accent-glow) 34%, transparent);
	}

	.mode-btn:hover,
	.dock-btn:hover,
	.dock-link:hover,
	.seed-chip:hover {
		transform: translateY(-1px);
	}

	.dock-link {
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		text-decoration: none;
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

	.search-shell input {
		height: 40px;
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
		font-size: var(--font-size-sm);
	}

	.seed-chips {
		display: flex;
		align-items: center;
		gap: 8px;
		flex-wrap: wrap;
		flex: 1;
	}

	.seed-chip {
		font-size: var(--font-size-xs);
	}

	.seed-mix-btn {
		flex-shrink: 0;
	}

	.seed-actions {
		display: flex;
		gap: 8px;
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
			width: min(292px, calc(100% - 32px));
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
			width: min(292px, calc(100% - 32px));
		}

		.mode-switcher {
			top: auto;
			right: 16px;
			bottom: 120px;
		}

		.mode-actions {
			left: 50%;
			right: auto;
			top: auto;
			transform: translateX(-50%);
			bottom: 82px;
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
		.mode-actions,
		.control-dock,
		.seed-builder,
		.error-toast,
		.notice-toast {
			position: relative;
			inset: auto;
			left: auto;
			right: auto;
			top: auto;
			bottom: auto;
			transform: none;
			width: 100%;
		}

		.mode-actions {
			order: 3;
			flex-wrap: wrap;
		}

		.hud {
			order: 1;
			gap: 8px;
			padding: 12px;
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

		.seed-builder {
			order: 5;
			align-items: flex-start;
		}

		.seed-chips {
			width: 100%;
		}
	}
</style>
