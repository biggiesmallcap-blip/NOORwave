<script lang="ts">
	// The Atlas stage: the map, its HUD and its cards. The stage is a night sky
	// in every theme (like the video stage); the chrome on it uses stage-local
	// colours. Interactions: drag to pan (with a fling), wheel and pinch to zoom
	// around the pointer, hover for a name, click a scene or an artist for its
	// card, the section search field to fly to an artist, Journeys to travel
	// across the map (somewhere new), and the two looks:
	// Picture (artists in the colours of their own videos) and You (your
	// footprints and the frontier next to them). Keys: arrows pan, +/- zoom,
	// F fits, J opens Journeys, Esc closes.
	import { onDestroy, onMount, tick, untrack } from 'svelte';
	import { replaceState } from '$app/navigation';
	import { page } from '$app/state';
	import { api } from '$lib/api/client';
	import type {
		AtlasArtistCard as ArtistCardData,
		AtlasJourneyResponse,
		AtlasPicturesResponse,
		AtlasSceneCard as SceneCardData,
		AtlasYouResponse,
		ExploreVideo,
		JourneyKind,
		Light,
	} from '$lib/api/video_explore_types';
	import { buildArtistMenu } from '$lib/player/artist_menu';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { reduceMotion } from '$lib/stores/motion';
	import { videoSession } from '$lib/stores/video_session';
	import { videoSectionQuery } from '$lib/video/section';
	import AtlasArtistCard from './AtlasArtistCard.svelte';
	import AtlasJourneyCard from './AtlasJourneyCard.svelte';
	import AtlasSceneCard from './AtlasSceneCard.svelte';
	import { atlasLook, atlasPictures, atlasYou, rememberName, type LoadedAtlas } from './atlas_data';
	import type { AtlasData } from './decode';
	import { ATLAS_SEARCH_LABEL, atlasUrl } from './links';
	import { playAtlasQueue, probeMessage } from './play';
	import { AtlasRenderer } from './renderer';
	import { buildNameIndex, searchArtists, searchScenes, type AtlasSceneHit, type AtlasSearchHit } from './search';
	import { sceneAt } from './spatial';
	import {
		ATLAS_LOOKS,
		FOOTPRINT_COLORS,
		FRONTIER_COLORS,
		LIGHT_HELP,
		footprintStep,
		formatCount,
		lightSwatch,
		type AtlasLook,
	} from './style';

	let {
		loaded,
		focus,
		rebuilding = false,
	}: {
		loaded: LoadedAtlas;
		focus: { artist: number | null; scene: number | null; trail: number[] };
		/** A newer map is being drawn in the background. */
		rebuilding?: boolean;
	} = $props();

	const CARD_WIDTH = 380;
	const KEY_PAN = 90;

	let data = $derived(loaded.data);
	let names = $derived(loaded.names);
	let nameIndex = $derived(buildNameIndex(data, names));
	let query = $derived($videoSectionQuery.trim());

	let stageEl = $state<HTMLDivElement>();
	let canvasEl = $state<HTMLCanvasElement>();
	let resultsEl = $state<HTMLUListElement>();
	let renderer: AtlasRenderer | null = null;
	let ready = $state(false);
	let compact = $state(false);
	let legendOpen = $state(false);

	type Card = { kind: 'scene'; index: number } | { kind: 'artist'; index: number } | { kind: 'journey' };
	let card = $state<Card | null>(null);
	let sceneCard = $state<SceneCardData | null>(null);
	let sceneLoading = $state(false);
	let sceneError = $state<string | null>(null);
	let artistCard = $state<ArtistCardData | null>(null);
	let artistLoading = $state(false);
	let artistError = $state<string | null>(null);
	let probe = $state({ busy: false, message: null as string | null, sent: false });

	// Journeys: where from, the answer, and whether a map click is being
	// waited for (the start).
	let journeyFrom = $state(-1);
	let journeyKind = $state<JourneyKind | null>(null);
	let journey = $state<AtlasJourneyResponse | null>(null);
	let journeyLoading = $state(false);
	let journeyError = $state<string | null>(null);
	let picking = $state<'from' | null>(null);
	// Scenes come up by name, channel number or genre; not while a journey
	// start is being picked (that takes an artist).
	let sceneHits = $derived<AtlasSceneHit[]>(query && !picking ? searchScenes(data.scenes, query, 6) : []);
	// Twelve lines in all: the artists fill what the scenes leave.
	let hits = $derived<AtlasSearchHit[]>(query ? searchArtists(nameIndex, data, query, Math.max(6, 12 - sceneHits.length)) : []);
	/** Scenes lead unless an artist is named exactly (and no scene is):
	 *  a word that names or tags scenes is most likely a place on the map. */
	let scenesFirst = $derived(sceneHits.length > 0 && (hits.length === 0 || hits[0].tier > 0 || sceneHits[0].tier === 0));
	let anyHits = $derived(hits.length + sceneHits.length > 0);
	/** The journey playing, followed on the map while its label is what
	 *  the session says is playing. */
	let following = $state<{ label: string; ids: number[] } | null>(null);
	let followedArtist = -1;

	// The looks' own data (shared with the mini map).
	// Raw state: each answer is replaced whole, and the renderer reads them
	// in tight loops where a deep proxy costs hundreds of milliseconds.
	let pictures = $state.raw<AtlasPicturesResponse | null>(atlasPictures.cached());
	let you = $state.raw<AtlasYouResponse | null>(atlasYou.cached());

	let tip = $state<{ x: number; y: number; title: string; meta: string; hint: string } | null>(null);
	let hereScene = $state(-1);
	let dragging = $state(false);

	const sceneCache = new Map<number, SceneCardData>();
	const artistCache = new Map<number, ArtistCardData>();
	let sceneSeq = 0;
	let artistSeq = 0;
	let journeySeq = 0;
	let dwellTimer: ReturnType<typeof setTimeout> | null = null;

	// ------------------------------------------------------------ helpers

	function nameOf(index: number): string | null {
		return index >= 0 ? (names.get(data.ids[index]) ?? null) : null;
	}

	function hueOf(index: number): number | null {
		const s = index >= 0 ? data.scene[index] : -1;
		return s >= 0 ? data.scenes[s].hue : null;
	}

	/** Where the card covers the stage, so flights frame the subject beside it. */
	function offsets(): [number, number] {
		if (!renderer) return [0, 0];
		if (compact) return [0, Math.min(renderer.viewport.height * 0.27, 220)];
		return [(CARD_WIDTH + 24) / 2, 0];
	}

	function syncUrl(params: { artist?: number | null; scene?: number | null }) {
		const trail = focus.trail;
		const url = atlasUrl({ ...params, trail: trail.length ? trail : null });
		if (page.url.pathname + page.url.search === url) return;
		try {
			replaceState(url, page.state);
		} catch {
			// Not ready (first render): the URL just stays as it was.
		}
	}

	// ------------------------------------------------------------ cards

	async function loadScene(index: number) {
		const scene = data.scenes[index];
		const seq = ++sceneSeq;
		sceneError = null;
		sceneCard = sceneCache.get(scene.id) ?? null;
		if (sceneCard) return;
		sceneLoading = true;
		try {
			const result = await api.getVideoAtlasScene(scene.id);
			sceneCache.set(scene.id, result);
			if (seq === sceneSeq) sceneCard = result;
		} catch {
			if (seq === sceneSeq) sceneError = 'Could not load this scene.';
		} finally {
			if (seq === sceneSeq) sceneLoading = false;
		}
	}

	async function fetchArtist(id: number): Promise<ArtistCardData> {
		const hit = artistCache.get(id);
		if (hit) return hit;
		const result = await api.getVideoAtlasArtist(id);
		artistCache.set(id, result);
		if (result.name && !names.has(id)) {
			rememberName(id, result.name);
			names.set(id, result.name);
			renderer?.setNames(names);
		}
		return result;
	}

	async function loadArtist(index: number) {
		const id = data.ids[index];
		const seq = ++artistSeq;
		artistError = null;
		artistCard = artistCache.get(id) ?? null;
		if (artistCard) return;
		artistLoading = true;
		try {
			const result = await fetchArtist(id);
			if (seq === artistSeq) artistCard = result;
		} catch {
			if (seq === artistSeq) artistError = 'Could not load this artist.';
		} finally {
			if (seq === artistSeq) artistLoading = false;
		}
	}

	function selectScene(index: number, fly = true) {
		if (index < 0 || index >= data.scenes.length) return;
		card = { kind: 'scene', index };
		renderer?.setSelection(-1, index);
		if (fly) {
			const [ox, oy] = offsets();
			renderer?.flyToScene(index, ox, oy);
		}
		void loadScene(index);
		syncUrl({ scene: data.scenes[index].id });
	}

	function selectArtist(index: number, fly = true) {
		if (index < 0 || index >= data.count) return;
		card = { kind: 'artist', index };
		probe = { busy: false, message: null, sent: false };
		renderer?.setSelection(index, -1);
		if (fly) {
			const [ox, oy] = offsets();
			renderer?.flyToArtist(index, compact ? 7 : 9, ox, oy);
		}
		void loadArtist(index);
		syncUrl({ artist: data.ids[index] });
	}

	function selectArtistById(id: number) {
		const index = data.indexById.get(id);
		if (index !== undefined) selectArtist(index);
	}

	function closeCard() {
		if (card?.kind === 'journey') {
			closeJourneys();
			return;
		}
		card = null;
		renderer?.setSelection(-1, -1);
		syncUrl({});
	}

	async function sendProbe() {
		if (!card || card.kind !== 'artist') return;
		const index = card.index;
		const id = data.ids[index];
		const name = artistCard?.name ?? nameOf(index) ?? 'this artist';
		probe = { busy: true, message: null, sent: false };
		try {
			const result = await api.probeVideoAtlasArtist(id);
			if (card?.kind !== 'artist' || card.index !== index) return;
			probe = { busy: false, message: probeMessage(name, result.queued, result.reason), sent: result.queued };
		} catch {
			probe = { busy: false, message: 'The probe did not go out. Try again in a moment.', sent: false };
		}
	}

	// ------------------------------------------------------------ journeys

	/** Where a journey starts by default: the artist playing, if it is on
	 *  the map, else the artist whose card is open. */
	function defaultFrom(): number {
		const playing = $videoSession.current?.artist_id;
		if (playing != null) {
			const index = data.indexById.get(playing);
			if (index !== undefined) return index;
		}
		return card?.kind === 'artist' ? card.index : -1;
	}

	function openJourneys(from = defaultFrom()) {
		journeySeq += 1;
		card = { kind: 'journey' };
		journeyFrom = from;
		journeyKind = null;
		journey = null;
		journeyError = null;
		journeyLoading = false;
		picking = from < 0 ? 'from' : null;
		if (!following) renderer?.setRoute(null);
		renderer?.setSelection(from, -1);
	}

	function closeJourneys() {
		journeySeq += 1;
		picking = null;
		journey = null;
		journeyKind = null;
		journeyLoading = false;
		journeyError = null;
		if (!following) renderer?.setRoute(null);
		if (card?.kind === 'journey') card = null;
		renderer?.setSelection(-1, -1);
	}

	function toggleJourneys() {
		if (card?.kind === 'journey') closeJourneys();
		else openJourneys();
	}

	/** A map (or search) pick while Journeys waits for one. */
	function journeyPick(index: number) {
		if (picking === 'from') {
			journeyFrom = index;
			picking = null;
			renderer?.setSelection(index, -1);
		}
	}

	function stopIndex(id: number): number {
		return data.indexById.get(id) ?? -1;
	}

	async function runJourney(kind: JourneyKind, scene: number | null = null) {
		if (journeyFrom < 0) return;
		const seq = ++journeySeq;
		journeyKind = kind;
		journeyLoading = true;
		journeyError = null;
		journey = null;
		try {
			const result = await api.getVideoAtlasJourney({
				kind,
				from: data.ids[journeyFrom],
				scene: scene != null ? data.scenes[scene].id : undefined,
			});
			if (seq !== journeySeq) return;
			journey = result;
			for (const stop of result.stops) if (stop.name) rememberName(stop.artist_id, stop.name);
			const indices = result.found ? result.stops.map((stop) => stopIndex(stop.artist_id)).filter((i) => i >= 0) : [];
			renderer?.setSelection(-1, -1);
			renderer?.setRoute(indices.length > 1 ? indices : null);
			const [ox, oy] = offsets();
			if (indices.length > 1) renderer?.flyToIndices(indices, ox, oy);
		} catch {
			if (seq === journeySeq) journeyError = 'Could not plan that journey.';
		} finally {
			if (seq === journeySeq) journeyLoading = false;
		}
	}

	function playJourney(queue: ExploreVideo[], label: string, start?: ExploreVideo) {
		if (!journey) return;
		following = { label, ids: journey.stops.map((stop) => stop.artist_id) };
		followedArtist = -1;
		renderer?.setTrail(null);
		void playAtlasQueue(start ?? queue[0], queue, label);
	}

	// The open artist's collabs, drawn from them across the map.
	$effect(() => {
		const index = card?.kind === 'artist' ? card.index : -1;
		const open = artistCard;
		if (!ready || !renderer) return;
		const collabs = index >= 0 && open && open.id === data.ids[index] ? open.collabs : [];
		const targets = collabs.map((collab) => data.indexById.get(collab.id) ?? -1).filter((i) => i >= 0);
		renderer.setCollabs(index, targets);
	});

	// While a journey plays, the map follows it: the camera goes to each
	// stop's artist as its video starts and the trail lights up behind it.
	// Picking anything else to play ends the following.
	$effect(() => {
		const session = $videoSession;
		const journeyPlaying = following;
		if (!journeyPlaying || !renderer) return;
		if (session.sourceLabel !== journeyPlaying.label) {
			if (session.current) untrack(() => (following = null));
			return;
		}
		const artist = session.current?.artist_id ?? null;
		const k = artist != null ? journeyPlaying.ids.indexOf(artist) : -1;
		if (k < 0 || artist === followedArtist) return;
		followedArtist = artist ?? -1;
		untrack(() => {
			const trail = journeyPlaying.ids.slice(0, k + 1).map(stopIndex).filter((i) => i >= 0);
			renderer?.setTrail(trail.length ? trail : null);
			const index = stopIndex(journeyPlaying.ids[k]);
			if (index >= 0) {
				const [ox, oy] = offsets();
				renderer?.flyToArtist(index, compact ? 6 : 7, ox, oy);
			}
		});
	});

	// ------------------------------------------------------------ search

	function pickHit(hit: AtlasSearchHit) {
		videoSectionQuery.set('');
		if (picking) journeyPick(hit.index);
		else selectArtist(hit.index);
		// Leave the field so the map keys (arrows, +/-, Esc) work at once.
		if (document.activeElement instanceof HTMLElement) document.activeElement.blur();
	}

	function pickScene(hit: AtlasSceneHit) {
		videoSectionQuery.set('');
		selectScene(hit.index);
		if (document.activeElement instanceof HTMLElement) document.activeElement.blur();
	}

	function sceneMeta(hit: AtlasSceneHit): string {
		const on = hit.channelNo != null ? `Channel ${hit.channelNo}` : 'Not on air yet';
		return hit.tag ? `${hit.tag}, ${on.toLowerCase()}` : on;
	}

	// ------------------------------------------------------------ pointer

	const pointers = new Map<number, { x: number; y: number }>();
	let downAt = { x: 0, y: 0, t: 0 };
	let moved = 0;
	let velocity = { x: 0, y: 0, t: 0 };
	let pinch: { dist: number; mx: number; my: number } | null = null;

	function local(event: PointerEvent | MouseEvent | WheelEvent): { x: number; y: number } {
		const rect = canvasEl!.getBoundingClientRect();
		return { x: event.clientX - rect.left, y: event.clientY - rect.top };
	}

	function pinchState() {
		const [a, b] = [...pointers.values()];
		return { dist: Math.hypot(a.x - b.x, a.y - b.y), mx: (a.x + b.x) / 2, my: (a.y + b.y) / 2 };
	}

	function onPointerDown(event: PointerEvent) {
		if (!renderer || event.button > 0) return;
		const p = local(event);
		pointers.set(event.pointerId, p);
		canvasEl?.setPointerCapture(event.pointerId);
		renderer.cancelMotion();
		if (pointers.size === 1) {
			downAt = { x: p.x, y: p.y, t: performance.now() };
			moved = 0;
			velocity = { x: 0, y: 0, t: performance.now() };
		} else if (pointers.size === 2) {
			pinch = pinchState();
			moved = 99;
		}
	}

	function onPointerMove(event: PointerEvent) {
		if (!renderer) return;
		const p = local(event);
		const prev = pointers.get(event.pointerId);
		if (!prev) {
			hover(p.x, p.y);
			return;
		}
		pointers.set(event.pointerId, p);
		if (pointers.size >= 2 && pinch) {
			const next = pinchState();
			renderer.panBy(next.mx - pinch.mx, next.my - pinch.my);
			if (pinch.dist > 0) renderer.zoomAt(next.mx, next.my, next.dist / pinch.dist, true);
			pinch = next;
			return;
		}
		const dx = p.x - prev.x;
		const dy = p.y - prev.y;
		moved += Math.abs(dx) + Math.abs(dy);
		if (moved > 4) {
			if (!dragging) {
				dragging = true;
				tip = null;
			}
			renderer.panBy(dx, dy);
			const now = performance.now();
			const dt = Math.max(1, now - velocity.t);
			// Smoothed release velocity in px per ms.
			velocity = { x: velocity.x * 0.6 + (dx / dt) * 0.4, y: velocity.y * 0.6 + (dy / dt) * 0.4, t: now };
		}
	}

	function onPointerUp(event: PointerEvent) {
		if (!renderer) return;
		const p = local(event);
		const had = pointers.delete(event.pointerId);
		if (pointers.size < 2) pinch = null;
		if (!had) return;
		if (pointers.size > 0) return;
		if (dragging) {
			dragging = false;
			if (performance.now() - velocity.t < 80) renderer.startFling(velocity.x, velocity.y);
			return;
		}
		if (moved <= 4) click(p.x, p.y);
	}

	function onPointerCancel(event: PointerEvent) {
		pointers.delete(event.pointerId);
		pinch = null;
		dragging = false;
	}

	function onPointerLeave() {
		if (pointers.size === 0) {
			renderer?.setHover(-1, -1);
			renderer?.setPointer(null);
			tip = null;
		}
	}

	function click(x: number, y: number) {
		if (!renderer) return;
		const hit = renderer.hitTest(x, y, compact ? 9 : 5);
		if (hit.kind === 'artist') {
			if (picking) journeyPick(hit.index);
			else selectArtist(hit.index);
		} else if (hit.kind === 'scene' && !picking) {
			// With Journeys open and a start chosen, a scene is a destination.
			if (card?.kind === 'journey' && journeyFrom >= 0 && !journeyLoading) void runJourney('new', hit.index);
			else selectScene(hit.index);
		} else if (hit.kind === 'none' && card && card.kind !== 'journey' && !picking) {
			closeCard();
		}
	}

	function hover(x: number, y: number) {
		if (!renderer || dragging) return;
		// Artist names gather around the pointer (labels.ts).
		renderer.setPointer(x, y);
		const hit = renderer.hitTest(x, y);
		if (dwellTimer) clearTimeout(dwellTimer);
		dwellTimer = null;
		if (hit.kind === 'artist') {
			const i = hit.index;
			renderer.setHover(i, -1);
			const scene = data.scene[i] >= 0 ? data.scenes[data.scene[i]].name : 'Uncharted';
			const light = data.light[i] as Light;
			tip = {
				x,
				y,
				title: nameOf(i) ?? 'Unnamed artist',
				meta: `${formatCount(data.videos[i])} ${data.videos[i] === 1 ? 'video' : 'videos'} in ${scene}`,
				hint: picking === 'from' ? 'Click to start from here' : lookHint(i, light),
			};
			if (!nameOf(i)) {
				// Names may be partial: learn this one after a short dwell.
				const id = data.ids[i];
				dwellTimer = setTimeout(() => {
					void fetchArtist(id)
						.then((result) => {
							if (renderer?.hover === i && tip) tip = { ...tip, title: result.name };
						})
						.catch(() => {});
				}, 350);
			}
		} else if (hit.kind === 'scene') {
			renderer.setHover(-1, hit.index);
			const scene = data.scenes[hit.index];
			tip = {
				x,
				y,
				title: scene.name,
				meta: `${formatCount(scene.artists)} artists, ${formatCount(scene.videos)} videos`,
				hint: picking
					? 'Pick an artist'
					: card?.kind === 'journey' && journeyFrom >= 0
						? 'Click to travel here'
						: scene.channel_no != null
							? `Channel ${scene.channel_no}`
							: 'Open the scene',
			};
		} else {
			renderer.setHover(-1, -1);
			tip = null;
		}
	}

	/** What the tooltip adds about an artist in the look on show. */
	function lookHint(index: number, light: Light): string {
		if ($atlasLook === 'picture') return light === 0 ? LIGHT_HELP[0] : '';
		const id = data.ids[index];
		const foot = you?.footprints.find((f) => f.artist_id === id);
		if (foot) {
			const days = Math.floor((Date.now() / 1000 - foot.last_at) / 86_400);
			return days < 1 ? 'Watched today' : days < 2 ? 'Watched yesterday' : `Watched ${days} days ago`;
		}
		const edge = you?.frontier.find((f) => f.artist_id === id);
		if (edge) {
			const via = names.get(edge.via);
			return via ? `Frontier: next to ${via}` : 'Frontier: not watched yet';
		}
		return '';
	}

	function onContextMenu(event: MouseEvent) {
		if (!renderer) return;
		const p = local(event);
		const hit = renderer.hitTest(p.x, p.y);
		if (hit.kind !== 'artist') return;
		const id = data.ids[hit.index];
		const name = nameOf(hit.index) ?? artistCache.get(id)?.name ?? 'Artist';
		openContextMenu(event, buildArtistMenu({ tidal_id: id, name }), name);
	}

	function onWheel(event: WheelEvent) {
		if (!renderer || pageTakesWheel(event)) return;
		event.preventDefault();
		const p = local(event);
		// A sideways trackpad swipe pans; everything else zooms.
		if (!event.ctrlKey && event.deltaMode === 0 && Math.abs(event.deltaX) > Math.abs(event.deltaY) * 1.5) {
			renderer.panBy(-event.deltaX, -event.deltaY);
			tip = null;
			return;
		}
		const unit = event.deltaMode === 1 ? 0.05 : event.deltaMode === 2 ? 0.5 : event.ctrlKey ? 0.01 : 0.0018;
		const factor = Math.exp(-event.deltaY * unit);
		renderer.zoomAt(p.x, p.y, Math.min(4, Math.max(0.25, factor)));
		tip = null;
	}

	function onDoubleClick(event: MouseEvent) {
		if (!renderer) return;
		const p = local(event);
		renderer.zoomAt(p.x, p.y, event.shiftKey ? 0.5 : 2.2);
	}

	// ------------------------------------------------------------ keyboard

	function escape(): boolean {
		if (query) {
			videoSectionQuery.set('');
			return true;
		}
		if (picking) {
			picking = null;
			return true;
		}
		if (card?.kind === 'journey') {
			closeJourneys();
			return true;
		}
		if (card) {
			closeCard();
			return true;
		}
		return false;
	}

	function onKeydown(event: KeyboardEvent) {
		if (!renderer || event.defaultPrevented || event.ctrlKey || event.metaKey || event.altKey) return;
		const target = event.target as HTMLElement | null;
		// The section's search field finds artists here.
		if (target instanceof HTMLInputElement && target.getAttribute('aria-label') === ATLAS_SEARCH_LABEL) {
			if (event.key === 'Enter' && anyHits) {
				event.preventDefault();
				resultsEl?.querySelector('button')?.click();
			} else if (event.key === 'ArrowDown' && anyHits) {
				event.preventDefault();
				resultsEl?.querySelector('button')?.focus();
			}
			return;
		}
		const onPage = !target || target === document.body || target === document.documentElement;
		const inStage = Boolean(target && stageEl?.contains(target));
		if (!onPage && !inStage) return;
		if (target?.closest('input, textarea, select, [contenteditable="true"]')) return;
		const { width, height } = renderer.viewport;
		let handled = true;
		switch (event.key) {
			case 'ArrowLeft':
				renderer.panBy(KEY_PAN, 0);
				break;
			case 'ArrowRight':
				renderer.panBy(-KEY_PAN, 0);
				break;
			case 'ArrowUp':
				if (target?.closest('.results')) handled = moveResult(target, -1);
				else renderer.panBy(0, KEY_PAN);
				break;
			case 'ArrowDown':
				if (target?.closest('.results')) handled = moveResult(target, 1);
				else renderer.panBy(0, -KEY_PAN);
				break;
			case '+':
			case '=':
				renderer.zoomAt(width / 2, height / 2, 1.6);
				break;
			case '-':
			case '_':
				renderer.zoomAt(width / 2, height / 2, 1 / 1.6);
				break;
			case 'f':
			case 'F':
				renderer.fit();
				break;
			case 'j':
			case 'J':
				toggleJourneys();
				break;
			case 'Escape':
				handled = escape();
				break;
			default:
				handled = false;
		}
		if (handled) {
			event.preventDefault();
			event.stopPropagation();
		}
	}

	function moveResult(target: HTMLElement, step: number): boolean {
		const buttons = [...(resultsEl?.querySelectorAll('button') ?? [])];
		const at = buttons.indexOf(target as HTMLButtonElement);
		const next = buttons[at + step];
		if (next) next.focus();
		return true;
	}

	// ------------------------------------------------------------ setup

	/** The stage fills the window between what is above it and what is
	 *  below it (the section's and the workspace's bottom insets), so the
	 *  page itself never scrolls under the map. */
	function measureTop() {
		if (!stageEl) return;
		const workspace = scroller();
		const rect = stageEl.getBoundingClientRect();
		const top = rect.top + (workspace?.scrollTop ?? 0);
		stageEl.style.setProperty('--atlas-top', `${Math.max(0, Math.round(top))}px`);
		if (workspace) {
			const view = workspace.getBoundingClientRect();
			const bottomInContent = rect.bottom - view.top + workspace.scrollTop;
			const below = workspace.scrollHeight - bottomInContent;
			stageEl.style.setProperty('--atlas-bottom', `${Math.max(0, Math.round(below))}px`);
		}
	}

	function scroller(): HTMLElement | null {
		return stageEl?.closest<HTMLElement>('main.workspace') ?? document.querySelector<HTMLElement>('main.workspace');
	}

	/** When the window is too short for the map's minimum height, the page
	 *  does scroll: then a wheel over the map moves the page first (back to
	 *  the top going up, until the map's foot is in view going down) and
	 *  only then zooms, so the map can never trap you below the header. */
	function pageTakesWheel(event: WheelEvent): boolean {
		const workspace = scroller();
		if (!workspace || !stageEl || event.ctrlKey) return false;
		const view = workspace.getBoundingClientRect();
		const rect = stageEl.getBoundingClientRect();
		if (event.deltaY < 0) return workspace.scrollTop > 0;
		if (event.deltaY > 0) {
			const room = workspace.scrollHeight - workspace.clientHeight - workspace.scrollTop;
			return room > 1 && rect.bottom > view.bottom + 1;
		}
		return false;
	}

	function onCamera() {
		if (!renderer) return;
		const k = renderer.zoomLevel();
		const s = k > 2.2 ? sceneAt(data.scenes, renderer.camera.cx, renderer.camera.cy, 1.35) : -1;
		if (s !== hereScene) hereScene = s;
		if (tip && !dragging) tip = null;
	}

	function applyFocus() {
		if (!renderer) return;
		const trail = focus.trail.map((id) => data.indexById.get(id) ?? -1).filter((i) => i >= 0);
		renderer.setTrail(trail.length ? trail : null);
		if (focus.artist != null) {
			const index = data.indexById.get(focus.artist);
			if (index !== undefined) {
				selectArtist(index);
				return;
			}
		}
		if (focus.scene != null) {
			const index = data.sceneIndexById.get(focus.scene);
			if (index !== undefined) {
				selectScene(index);
				return;
			}
		}
		if (trail.length > 1) renderer.flyToIndices(trail);
	}

	let resizeObserver: ResizeObserver | null = null;
	/** Watches the page around the map: a player bar arriving changes the
	 *  bottom inset, and the map is resized to match. */
	let insetObserver: ResizeObserver | null = null;
	let overlayObserver: ResizeObserver | null = null;
	let unsubscribeMotion: (() => void) | null = null;

	/** The toolbars, legend and open card cover parts of the map; names are
	 *  placed around them instead of hiding underneath. */
	function syncObstacles() {
		if (!renderer || !stageEl || !canvasEl) return;
		const base = canvasEl.getBoundingClientRect();
		const boxes = [...stageEl.querySelectorAll<HTMLElement>('.hud, .legend, .panel')]
			.map((el) => {
				const r = el.getBoundingClientRect();
				return { x: r.left - base.left - 6, y: r.top - base.top - 4, width: r.width + 12, height: r.height + 8 };
			})
			.filter((box) => box.width > 12 && box.height > 8);
		renderer.setObstacles(boxes);
	}

	// Overlays come and go with the card, the legend and route mode.
	$effect(() => {
		void card;
		void legendOpen;
		void compact;
		void picking;
		if (!ready || !stageEl) return;
		void tick().then(() => {
			overlayObserver?.disconnect();
			overlayObserver = new ResizeObserver(() => syncObstacles());
			stageEl?.querySelectorAll('.hud, .legend, .panel').forEach((el) => overlayObserver?.observe(el));
			syncObstacles();
		});
	});

	onMount(() => {
		if (!canvasEl || !stageEl) return;
		try {
			renderer = new AtlasRenderer(canvasEl, { onCamera });
		} catch {
			return;
		}
		unsubscribeMotion = reduceMotion.subscribe((reduce) => renderer?.setReducedMotion(reduce));
		const rect = canvasEl.getBoundingClientRect();
		compact = rect.width < 640;
		renderer.resize(rect.width, rect.height, window.devicePixelRatio || 1);
		renderer.setLook($atlasLook);
		renderer.setPictures(pictures);
		renderer.setYou(you);
		const hasFocus = focus.artist != null || focus.scene != null || focus.trail.length > 0;
		renderer.setData(data, names, { intro: !hasFocus });
		drawnData = data;
		legendOpen = !compact && readKeyOpen();
		applyFocus();
		ready = true;
		measureTop();

		resizeObserver = new ResizeObserver(() => {
			if (!canvasEl || !renderer) return;
			const r = canvasEl.getBoundingClientRect();
			compact = r.width < 640;
			renderer.resize(r.width, r.height, window.devicePixelRatio || 1);
			syncObstacles();
		});
		resizeObserver.observe(canvasEl);
		canvasEl.addEventListener('wheel', onWheel, { passive: false });
		window.addEventListener('keydown', onKeydown, { capture: true });
		window.addEventListener('resize', measureTop);
		const workspace = scroller();
		if (workspace) {
			insetObserver = new ResizeObserver(() => measureTop());
			for (const child of workspace.children) insetObserver.observe(child);
		}

		// Canvas text needs the web fonts loaded before it measures labels.
		if (document.fonts) {
			void Promise.all([
				document.fonts.load('650 14px Manrope'),
				document.fonts.load('600 11px Manrope'),
				document.fonts.load('500 10px "JetBrains Mono"'),
			])
				.catch(() => {})
				.then(() => renderer?.fontsChanged());
		}
	});

	onDestroy(() => {
		renderer?.destroy();
		renderer = null;
		resizeObserver?.disconnect();
		insetObserver?.disconnect();
		overlayObserver?.disconnect();
		unsubscribeMotion?.();
		if (dwellTimer) clearTimeout(dwellTimer);
		if (typeof window !== 'undefined') {
			window.removeEventListener('keydown', onKeydown, { capture: true });
			window.removeEventListener('resize', measureTop);
		}
		canvasEl?.removeEventListener('wheel', onWheel);
	});

	// A refreshed payload (light changes with every watch) redraws in place.
	let drawnData: AtlasData | null = null;
	$effect(() => {
		const next = data;
		if (!renderer || next === drawnData) return;
		drawnData = next;
		renderer.setData(next, names);
	});

	$effect(() => {
		renderer?.setLook($atlasLook);
	});

	// Each look loads its own data when shown (and again after a minute:
	// Picture fills in as the artwork is read, You changes with every
	// watch). A fresh atlas asks for fresh You.
	async function loadLookData(look: AtlasLook, fresh = false) {
		if (look === 'picture') {
			const next = await atlasPictures.load(fresh);
			if (next) {
				pictures = next;
				renderer?.setPictures(next);
			}
		} else {
			const next = await atlasYou.load(fresh);
			if (next) {
				you = next;
				renderer?.setYou(next);
			}
		}
	}

	$effect(() => {
		const look = $atlasLook;
		void data;
		if (!ready) return;
		void untrack(() => loadLookData(look));
	});

	$effect(() => {
		const look = $atlasLook;
		if (!ready) return;
		const timer = setInterval(() => {
			// A few thumbnails can fail for good; 99.5% read is done.
			if (look === 'picture' && pictures && pictures.ready >= pictures.total * 0.995) return;
			void loadLookData(look, true);
		}, 60_000);
		return () => clearInterval(timer);
	});

	function setLook(look: AtlasLook) {
		atlasLook.set(look);
	}

	let footCounts = $derived.by(() => {
		const counts = [0, 0, 0];
		const now = Date.now() / 1000;
		for (const f of you?.footprints ?? []) {
			if (!data.indexById.has(f.artist_id)) continue;
			counts[footprintStep(Math.max(0, (now - f.last_at) / 86_400))] += 1;
		}
		return counts;
	});
	let frontierCount = $derived((you?.frontier ?? []).filter((f) => data.indexById.has(f.artist_id)).length);
	let footTotal = $derived(footCounts[0] + footCounts[1] + footCounts[2]);

	// The key stays shut unless it was left open: it explains the look once,
	// it does not greet every visit.
	const KEY_STORE = 'noor.atlas.key.v1';

	function readKeyOpen(): boolean {
		try {
			return localStorage.getItem(KEY_STORE) === 'open';
		} catch {
			return false;
		}
	}

	function setKeyOpen(open: boolean) {
		legendOpen = open;
		try {
			localStorage.setItem(KEY_STORE, open ? 'open' : 'closed');
		} catch {
			// The key just opens closed next time.
		}
	}

	let selectedIndex = $derived(card?.kind === 'artist' ? card.index : -1);
	let selectedScene = $derived(card?.kind === 'scene' ? data.scenes[card.index] : null);
	let pickBanner = $derived.by(() => {
		if (picking === 'from') return 'Journeys: click the artist to start from';
		return null;
	});
	let journeyPlaying = $derived(following !== null && journey !== null && $videoSession.sourceLabel === following.label);
</script>

<div
	class="stage"
	class:ready
	class:compact
	class:dragging
	class:route-mode={picking !== null}
	class:has-card={card !== null}
	bind:this={stageEl}
	tabindex="-1"
	role="application"
	aria-roledescription="map"
	aria-label="Atlas of video artists. Drag to pan, scroll to zoom, arrows pan, plus and minus zoom, F fits, J opens Journeys."
>
	<canvas
		bind:this={canvasEl}
		class="map"
		aria-hidden="true"
		onpointerdown={onPointerDown}
		onpointermove={onPointerMove}
		onpointerup={onPointerUp}
		onpointercancel={onPointerCancel}
		onpointerleave={onPointerLeave}
		oncontextmenu={onContextMenu}
		ondblclick={onDoubleClick}
	></canvas>

	<!-- HUD: look, where you are -->
	<div class="hud hud-left">
		<div class="looks" role="radiogroup" aria-label="Map look">
			{#each ATLAS_LOOKS as look (look.value)}
				<button
					type="button"
					role="radio"
					aria-checked={$atlasLook === look.value}
					class:active={$atlasLook === look.value}
					onclick={() => setLook(look.value)}
					title={look.help}
				>
					{look.label}
				</button>
			{/each}
		</div>
		{#if hereScene >= 0 && !compact}
			<button type="button" class="here" style={`--hue: ${data.scenes[hereScene].hue}`} onclick={() => selectScene(hereScene)}>
				<span class="here-dot" aria-hidden="true"></span>{data.scenes[hereScene].name}
			</button>
		{/if}
	</div>

	<!-- HUD: tools -->
	<div class="hud hud-right" role="toolbar" aria-label="Map tools">
		<button type="button" class="tool" class:on={card?.kind === 'journey'} onclick={toggleJourneys} aria-pressed={card?.kind === 'journey'} title="Journeys (J)">
			<svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="5" cy="18" r="2.2" /><circle cx="19" cy="6" r="2.2" /><path d="M7 17c5-1 3-9 10-10" /></svg>
			{#if !compact}<span>Journeys</span>{/if}
		</button>
		<div class="zoom">
			<button type="button" class="tool icon" onclick={() => renderer?.zoomAt(renderer.viewport.width / 2, renderer.viewport.height / 2, 1.6)} title="Zoom in (+)" aria-label="Zoom in">
				<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 5v14M5 12h14" /></svg>
			</button>
			<button type="button" class="tool icon" onclick={() => renderer?.zoomAt(renderer.viewport.width / 2, renderer.viewport.height / 2, 1 / 1.6)} title="Zoom out (-)" aria-label="Zoom out">
				<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M5 12h14" /></svg>
			</button>
			<button type="button" class="tool icon" onclick={() => renderer?.fit()} title="Whole map (F)" aria-label="Show the whole map">
				<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5" /></svg>
			</button>
		</div>
	</div>

	<!-- Search results from the section's field -->
	{#if query}
		<div class="results-wrap">
			{#if anyHits}
				{#snippet sceneGroup(label: boolean)}
					{#if label}<li class="r-group" aria-hidden="true">Scenes and channels</li>{/if}
					{#each sceneHits as hit (`s${hit.id}`)}
						<li>
							<button type="button" onclick={() => pickScene(hit)}>
								<svg class="r-scene" viewBox="0 0 16 16" aria-hidden="true"><circle cx="5" cy="6" r="2.4" /><circle cx="11" cy="5" r="1.8" /><circle cx="9" cy="11" r="2.1" /></svg>
								<span class="r-name">{hit.name}</span>
								<span class="r-meta">{sceneMeta(hit)}</span>
							</button>
						</li>
					{/each}
				{/snippet}
				{#snippet artistGroup(label: boolean)}
					{#if label}<li class="r-group" aria-hidden="true">Artists</li>{/if}
					{#each hits as hit (hit.id)}
						{@const s = data.scene[hit.index]}
						<li>
							<button type="button" onclick={() => pickHit(hit)}>
								<span class="dot" style={`background: ${lightSwatch(data.light[hit.index] as Light, hueOf(hit.index))}`} aria-hidden="true"></span>
								<span class="r-name">{hit.name}</span>
								<span class="r-meta">{s >= 0 ? data.scenes[s].name : 'Uncharted'}</span>
							</button>
						</li>
					{/each}
				{/snippet}
				<!-- Both kinds, the closer match first; labels only when both show. -->
				<ul class="results" bind:this={resultsEl} aria-label="Found on the map">
					{#if scenesFirst}
						{@render sceneGroup(hits.length > 0)}
						{#if hits.length > 0}{@render artistGroup(true)}{/if}
					{:else}
						{@render artistGroup(sceneHits.length > 0)}
						{#if sceneHits.length > 0}{@render sceneGroup(true)}{/if}
					{/if}
				</ul>
			{:else}
				<p class="results empty">Nothing called "{query}" on the map yet.</p>
			{/if}
		</div>
	{/if}

	<!-- Legend -->
	<!-- The key: shut by default, a small "How to read this" until asked.
	     Open, it says what the look shows in words and shapes (brightness,
	     rings, lines, size), never by colour alone. -->
	<div class="legend" class:closed={!legendOpen}>
		{#if legendOpen}
			<header class="key-head">
				<h3 class="key-title">{$atlasLook === 'picture' ? 'Picture' : 'You'}</h3>
				<button type="button" class="key-close" onclick={() => setKeyOpen(false)} aria-label="Close the key">
					<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M6 6l12 12M18 6L6 18" /></svg>
				</button>
			</header>
			{#if $atlasLook === 'picture'}
				<p class="legend-copy">Each artist is a dot in the colour of their own videos. The colour is only the picture, there is no code to learn. Zoom in close and the dots become the stills.</p>
				<ul class="legend-rows">
					<li>
						<span class="shape sizes" aria-hidden="true"><i></i><i></i><i></i></span>
						<span class="l-text">The bigger the dot, the more videos.</span>
					</li>
					<li>
						<span class="shape speck" aria-hidden="true"><i></i></span>
						<span class="l-text">Faint specks on the outer ring are not linked to the map yet.<span class="l-count">{formatCount(data.lightCounts[0])}</span></span>
					</li>
				</ul>
				{#if !pictures || pictures.ready < pictures.total * 0.995}
					<p class="legend-foot">
						{!pictures || pictures.ready === 0
							? "The stills are being read in the background; until then a dot takes its scene's colour."
							: `Still reading ${formatCount(pictures.total - pictures.ready)} artists' stills.`}
					</p>
				{/if}
			{:else}
				<p class="legend-copy">Your watching, drawn on the map. Everything you have not been near steps back into grey.</p>
				<ul class="legend-rows">
					<li>
						<span class="shape fade" aria-hidden="true">{#each FOOTPRINT_COLORS as color (color)}<i style:background={color}></i>{/each}</span>
						<span class="l-text">Gold dots: artists you watched. Brightest this week, fading over six months.<span class="l-count">{formatCount(footTotal)}</span></span>
					</li>
					<li>
						<span class="shape ring" aria-hidden="true"><i style:background={FRONTIER_COLORS[0]} style:color={FRONTIER_COLORS[0]}></i></span>
						<span class="l-text">Pale blue dots, the nearest ringed: next door to what you watch, not tried yet.<span class="l-count">{formatCount(frontierCount)}</span></span>
					</li>
					<li>
						<span class="shape line" aria-hidden="true"><i></i></span>
						<span class="l-text">Lines join videos you watched one after another.<span class="l-count">{formatCount(you?.trails.length ?? 0)}</span></span>
					</li>
				</ul>
			{/if}
			{#if data.recent.length > 0}
				<p class="legend-foot">{formatCount(data.recent.length)} artists charted in the last hour.</p>
			{/if}
			{#if rebuilding}
				<p class="legend-foot">A fresh map is being drawn.</p>
			{/if}
		{:else}
			<button type="button" class="key-open" onclick={() => setKeyOpen(true)} aria-expanded="false">
				<svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="8.5" /><path d="M12 11v5" /><circle class="dot-fill" cx="12" cy="8" r="0.6" /></svg>
				How to read this
			</button>
		{/if}
	</div>

	{#if pickBanner}
		<div class="banner" role="status">
			<span class="route-line" aria-hidden="true"></span>
			{pickBanner}
			<button type="button" class="link" onclick={() => (picking = null)}>Cancel</button>
		</div>
	{/if}

	{#if tip && !dragging}
		<div class="tip" style={`transform: translate(${Math.round(tip.x)}px, ${Math.round(tip.y)}px)`} role="tooltip">
			<strong>{tip.title}</strong>
			<span>{tip.meta}</span>
			{#if tip.hint}<em>{tip.hint}</em>{/if}
		</div>
	{/if}

	{#if card}
		<aside class="panel" class:sheet={compact} aria-label="Details">
			{#key card.kind === 'journey' ? 'journey' : `${card.kind}:${card.index}`}
				<div class="panel-inner">
					{#if card.kind === 'scene' && selectedScene}
						<AtlasSceneCard
							scene={selectedScene}
							card={sceneCard}
							loading={sceneLoading}
							error={sceneError}
							onclose={closeCard}
							onretry={() => card?.kind === 'scene' && void loadScene(card.index)}
							onartist={selectArtistById}
						/>
					{:else if card.kind === 'artist'}
						{@const s = data.scene[selectedIndex]}
						<AtlasArtistCard
							id={data.ids[selectedIndex]}
							name={nameOf(selectedIndex)}
							videos={data.videos[selectedIndex]}
							light={data.light[selectedIndex] as Light}
							hue={hueOf(selectedIndex)}
							sceneName={s >= 0 ? data.scenes[s].name : null}
							card={artistCard}
							loading={artistLoading}
							error={artistError}
							{probe}
							onclose={closeCard}
							onretry={() => void loadArtist(selectedIndex)}
							onscene={s >= 0 ? () => selectScene(s) : null}
							onneighbor={selectArtistById}
							onjourney={() => {
								openJourneys(selectedIndex);
								void runJourney('new');
							}}
							onprobe={() => void sendProbe()}
						/>
					{:else if card.kind === 'journey'}
						<AtlasJourneyCard
							fromName={journeyFrom >= 0 ? (nameOf(journeyFrom) ?? 'This artist') : null}
							{picking}
							{journey}
							loading={journeyLoading}
							error={journeyError}
							playing={journeyPlaying}
							onnew={() => void runJourney('new')}
							onpickfrom={() => (picking = 'from')}
							onplay={playJourney}
							onagain={() => openJourneys(journeyFrom)}
							onclose={closeJourneys}
							onretry={() => journeyKind && void runJourney(journeyKind)}
							onstop={(id) => {
								const index = data.indexById.get(id);
								if (index !== undefined) {
									const [ox, oy] = offsets();
									renderer?.flyToArtist(index, 9, ox, oy);
								}
							}}
						/>
					{/if}
				</div>
			{/key}
		</aside>
	{/if}
</div>

<style>
	/* The stage is dark in every theme; its chrome uses these. */
	.stage {
		--stage-glass: rgba(9, 11, 21, 0.78);
		--stage-glass-strong: rgba(9, 11, 21, 0.94);
		--stage-line: rgba(255, 255, 255, 0.08);
		--stage-hover: rgba(255, 255, 255, 0.07);
		--stage-text: rgba(240, 242, 250, 0.96);
		--stage-text-2: rgba(222, 226, 240, 0.72);
		--stage-text-3: rgba(206, 212, 230, 0.56);
		--card-pad: var(--space-5);
		position: relative;
		height: max(440px, calc(100dvh - var(--atlas-top, 230px) - var(--atlas-bottom, 104px)));
		border-radius: var(--radius-lg);
		overflow: hidden;
		background: #04050a;
		color: var(--stage-text);
		outline: none;
		isolation: isolate;
		user-select: none;
		-webkit-user-select: none;
	}

	:global([data-theme='light']) .stage {
		box-shadow: 0 18px 50px -24px rgba(10, 14, 30, 0.55);
	}

	/* The stage takes focus only so map keys reach it after a click; it is
	   not in the tab order, and a ring around the whole map reads as an
	   error. Its controls keep their own focus rings. */
	.stage:focus,
	.stage:focus-visible {
		outline: none;
	}

	.map {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
		display: block;
		cursor: grab;
		touch-action: none;
		opacity: 0;
		transition: opacity var(--motion-slow);
	}

	.ready .map {
		opacity: 1;
	}

	.dragging .map {
		cursor: grabbing;
	}

	.route-mode .map {
		cursor: crosshair;
	}

	/* ---------------------------------------------------------- HUD */

	.hud {
		position: absolute;
		top: var(--space-3);
		display: flex;
		align-items: center;
		gap: var(--space-2);
		z-index: 2;
		pointer-events: none;
	}

	.hud > * {
		pointer-events: auto;
	}

	.hud-left {
		left: var(--space-3);
		flex-wrap: wrap;
	}

	.hud-right {
		right: var(--space-3);
		transition: right var(--motion-base);
	}

	/* The card owns the right edge; the tools step left of it. */
	.has-card:not(.compact) .hud-right {
		right: calc(380px + var(--space-3) * 2);
	}

	.looks,
	.zoom {
		display: flex;
		padding: 3px;
		gap: 2px;
		border-radius: 999px;
		background: var(--stage-glass);
		border: 1px solid var(--stage-line);
		backdrop-filter: blur(var(--blur-overlay, 14px));
	}

	.looks button {
		height: 30px;
		padding: 0 var(--space-3);
		border-radius: 999px;
		color: var(--stage-text-2);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		white-space: nowrap;
		transition: background var(--motion-fast), color var(--motion-fast);
	}

	.looks button:hover {
		color: var(--stage-text);
	}

	.looks button.active {
		background: rgba(255, 255, 255, 0.13);
		color: var(--stage-text);
	}

	.here {
		display: inline-flex;
		align-items: center;
		gap: var(--space-2);
		height: 36px;
		padding: 0 var(--space-3);
		border-radius: 999px;
		background: var(--stage-glass);
		border: 1px solid var(--stage-line);
		backdrop-filter: blur(var(--blur-overlay, 14px));
		color: hsl(var(--hue), 22%, 84%);
		font-family: var(--font-body);
		text-transform: uppercase;
		letter-spacing: 0.14em;
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
	}

	.here-dot {
		width: 7px;
		height: 7px;
		border-radius: 50%;
		background: hsl(var(--hue), 85%, 66%);
		box-shadow: 0 0 10px hsl(var(--hue), 90%, 60%);
	}

	.tool {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		height: 36px;
		padding: 0 var(--space-3);
		border-radius: 999px;
		background: var(--stage-glass);
		border: 1px solid var(--stage-line);
		backdrop-filter: blur(var(--blur-overlay, 14px));
		color: var(--stage-text-2);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		transition: background var(--motion-fast), color var(--motion-fast);
	}

	.tool:hover {
		color: var(--stage-text);
		background: var(--stage-glass-strong);
	}

	.tool.on {
		color: #ffe6a8;
		border-color: rgba(255, 230, 168, 0.4);
	}

	.zoom .tool.icon {
		width: 30px;
		height: 30px;
		padding: 0;
		justify-content: center;
		background: transparent;
		border: 0;
		backdrop-filter: none;
	}

	.zoom .tool.icon:hover {
		background: var(--stage-hover);
	}

	.tool svg,
	.stage :global(.close svg) {
		width: 16px;
		height: 16px;
		fill: none;
		stroke: currentColor;
		stroke-width: 1.8;
		stroke-linecap: round;
		stroke-linejoin: round;
	}

	.tool circle {
		fill: currentColor;
		stroke: none;
	}

	/* ---------------------------------------------------------- search */

	.results-wrap {
		position: absolute;
		top: var(--space-3);
		left: 50%;
		transform: translateX(-50%);
		width: min(420px, calc(100% - 2 * var(--space-3)));
		z-index: 4;
	}

	.results {
		display: grid;
		gap: 1px;
		margin: 0;
		padding: var(--space-1);
		list-style: none;
		border-radius: var(--radius-md);
		background: var(--stage-glass-strong);
		border: 1px solid var(--stage-line);
		backdrop-filter: blur(var(--blur-modal, 18px));
		box-shadow: 0 18px 40px -18px rgba(0, 0, 0, 0.8);
		animation: rise var(--motion-fast) both;
	}

	.results.empty {
		padding: var(--space-3) var(--space-4);
		color: var(--stage-text-2);
		font-size: var(--font-size-sm);
	}

	.results button {
		display: grid;
		grid-template-columns: 12px minmax(0, 1fr) auto;
		align-items: center;
		gap: var(--space-3);
		width: 100%;
		padding: 9px var(--space-3);
		border-radius: var(--radius-sm);
		text-align: left;
		color: var(--stage-text);
	}

	.results button:hover,
	.results button:focus-visible {
		background: var(--stage-hover);
		outline: none;
	}

	.r-name {
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.r-meta {
		color: var(--stage-text-3);
		font-size: var(--font-size-xs);
		white-space: nowrap;
	}

	/* Group names, only when scenes and artists both show. */
	.r-group {
		padding: var(--space-2) var(--space-3) 2px;
		color: var(--stage-text-3);
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-semibold);
		letter-spacing: 0.08em;
		text-transform: uppercase;
	}

	/* A scene is a small cluster, an artist a single dot. */
	.r-scene {
		width: 12px;
		height: 12px;
		fill: var(--stage-text-2);
	}

	/* ---------------------------------------------------------- legend */

	/* The key: a small glass card at the bottom left, shut by default. */
	.legend {
		position: absolute;
		left: var(--space-3);
		bottom: var(--space-3);
		z-index: 2;
		width: min(300px, calc(100% - 2 * var(--space-3)));
		padding: var(--space-3) var(--space-4) var(--space-4);
		border-radius: var(--radius-md);
		background: var(--stage-glass-strong);
		border: 1px solid var(--stage-line);
		backdrop-filter: blur(var(--blur-overlay, 14px));
		animation: rise var(--motion-fast) both;
	}

	.legend.closed {
		width: auto;
		padding: 0;
		border: 0;
		background: none;
		backdrop-filter: none;
		animation: none;
	}

	.key-open {
		display: inline-flex;
		align-items: center;
		gap: 7px;
		height: 32px;
		padding: 0 14px 0 10px;
		border-radius: 999px;
		background: var(--stage-glass);
		border: 1px solid var(--stage-line);
		backdrop-filter: blur(var(--blur-overlay, 14px));
		color: var(--stage-text-2);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		white-space: nowrap;
		transition: background var(--motion-fast), color var(--motion-fast);
	}

	.key-open:hover {
		background: var(--stage-glass-strong);
		color: var(--stage-text);
	}

	.key-open svg,
	.key-close svg {
		width: 16px;
		height: 16px;
		fill: none;
		stroke: currentColor;
		stroke-width: 1.8;
		stroke-linecap: round;
		stroke-linejoin: round;
	}

	.key-open .dot-fill {
		fill: currentColor;
		stroke-width: 1.4;
	}

	.key-head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-2);
	}

	/* The look's name in the titles' italic serif, as the page openers. */
	.key-title {
		margin: 0;
		color: var(--stage-text);
		font-family: var(--font-display);
		font-size: var(--font-size-xl);
		font-style: italic;
		font-weight: var(--font-weight-medium);
		line-height: 1.1;
	}

	.key-close {
		display: grid;
		place-items: center;
		width: 28px;
		height: 28px;
		margin-right: -6px;
		border-radius: 50%;
		color: var(--stage-text-2);
		transition: background var(--motion-fast), color var(--motion-fast);
	}

	.key-close:hover {
		background: var(--stage-hover);
		color: var(--stage-text);
	}

	.key-open:focus-visible,
	.key-close:focus-visible {
		outline: 2px solid var(--stage-text);
		outline-offset: 2px;
	}

	.legend-copy {
		margin: var(--space-2) 0 0;
		color: var(--stage-text-2);
		font-size: var(--font-size-xs);
		line-height: var(--line-height-normal);
	}

	.legend-rows {
		display: grid;
		gap: 10px;
		margin: var(--space-3) 0 0;
		padding: 0;
		list-style: none;
	}

	.legend-rows li {
		display: grid;
		grid-template-columns: 30px minmax(0, 1fr);
		align-items: start;
		gap: var(--space-2);
		font-size: var(--font-size-xs);
		line-height: var(--line-height-normal);
	}

	.l-text {
		color: var(--stage-text-2);
	}

	.l-count {
		margin-left: 6px;
		color: var(--stage-text-3);
		font-variant-numeric: tabular-nums;
	}

	/* Each row's mark draws what the map draws: sizes, a speck, dots that
	   fade, a ringed dot, a line. Shape and brightness carry the meaning. */
	.shape {
		display: flex;
		align-items: center;
		gap: 3px;
		height: 18px;
	}

	.shape i {
		display: block;
		flex: none;
		border-radius: 50%;
	}

	.shape.sizes i {
		background: rgba(236, 239, 247, 0.78);
	}

	.shape.sizes i:nth-child(1) {
		width: 4px;
		height: 4px;
	}

	.shape.sizes i:nth-child(2) {
		width: 7px;
		height: 7px;
	}

	.shape.sizes i:nth-child(3) {
		width: 10px;
		height: 10px;
	}

	.shape.speck i {
		width: 4px;
		height: 4px;
		margin-left: 3px;
		background: hsla(222, 12%, 66%, 0.55);
	}

	.shape.fade i {
		width: 7px;
		height: 7px;
	}

	.shape.ring i {
		width: 7px;
		height: 7px;
		margin-left: 4px;
		box-shadow: 0 0 0 2px var(--stage-glass-strong), 0 0 0 3px currentColor;
	}

	.shape.line i {
		width: 22px;
		height: 2px;
		border-radius: 2px;
		background: hsla(38, 80%, 76%, 0.7);
	}

	.legend-foot {
		margin: var(--space-3) 0 0;
		color: var(--stage-text-3);
		font-size: var(--font-size-2xs);
		line-height: var(--line-height-normal);
	}

	@media (prefers-reduced-motion: reduce) {
		.legend {
			animation: none;
		}
	}

	/* ---------------------------------------------------------- banner, tip */

	.banner {
		position: absolute;
		left: 50%;
		bottom: var(--space-4);
		transform: translateX(-50%);
		z-index: 3;
		display: flex;
		align-items: center;
		gap: var(--space-3);
		max-width: calc(100% - 2 * var(--space-3));
		padding: var(--space-2) var(--space-4);
		border-radius: 999px;
		background: var(--stage-glass-strong);
		border: 1px solid rgba(255, 230, 168, 0.32);
		color: #ffeec8;
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		white-space: nowrap;
		animation: rise var(--motion-base) both;
	}

	.route-line {
		width: 18px;
		height: 2px;
		border-radius: 2px;
		background: #ffe6a8;
		box-shadow: 0 0 8px #ffd27a;
	}

	.stage .link,
	.stage :global(.link) {
		color: #ffe6a8;
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
	}

	.tip {
		position: absolute;
		left: 0;
		top: 0;
		z-index: 5;
		display: grid;
		gap: 2px;
		margin: 14px 0 0 14px;
		padding: var(--space-2) var(--space-3);
		max-width: 280px;
		border-radius: var(--radius-sm);
		background: var(--stage-glass-strong);
		border: 1px solid var(--stage-line);
		box-shadow: 0 12px 30px -14px rgba(0, 0, 0, 0.9);
		pointer-events: none;
		white-space: nowrap;
	}

	.tip strong {
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.tip span {
		color: var(--stage-text-2);
		font-size: var(--font-size-xs);
	}

	.tip em {
		color: var(--stage-text-3);
		font-size: var(--font-size-2xs);
		font-style: normal;
		text-transform: uppercase;
		letter-spacing: 0.12em;
	}

	/* ---------------------------------------------------------- panel */

	.panel {
		position: absolute;
		top: var(--space-3);
		right: var(--space-3);
		bottom: var(--space-3);
		z-index: 3;
		width: 380px;
		max-width: calc(100% - 2 * var(--space-3));
		display: flex;
		flex-direction: column;
		pointer-events: none;
	}

	.panel-inner {
		max-height: 100%;
		overflow-y: auto;
		overscroll-behavior: contain;
		scrollbar-width: thin;
		scrollbar-color: rgba(255, 255, 255, 0.18) transparent;
		border-radius: var(--radius-lg);
		background: var(--stage-glass-strong);
		border: 1px solid var(--stage-line);
		backdrop-filter: blur(var(--blur-modal, 18px)) saturate(1.2);
		box-shadow: 0 30px 60px -30px rgba(0, 0, 0, 0.9);
		pointer-events: auto;
		user-select: text;
		-webkit-user-select: text;
		animation: panel-in var(--motion-base) both;
	}

	.panel.sheet {
		top: auto;
		left: var(--space-2);
		right: var(--space-2);
		bottom: var(--space-2);
		width: auto;
		max-width: none;
		height: min(58%, 520px);
	}

	.panel.sheet .panel-inner {
		animation-name: sheet-in;
	}

	@keyframes panel-in {
		from {
			opacity: 0;
			transform: translateX(14px);
		}
		to {
			opacity: 1;
			transform: none;
		}
	}

	@keyframes sheet-in {
		from {
			opacity: 0;
			transform: translateY(16px);
		}
		to {
			opacity: 1;
			transform: none;
		}
	}

	@keyframes rise {
		from {
			opacity: 0;
			transform: translate(var(--rise-x, 0), 6px);
		}
	}

	.banner {
		--rise-x: -50%;
	}

	/* Shared card anatomy for the scene, artist and route cards. */
	.panel :global(.card) {
		display: grid;
		gap: var(--space-3);
		padding: var(--space-4) var(--card-pad) var(--card-pad);
	}

	.panel :global(.head) {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-2);
	}

	.panel :global(.kicker) {
		display: inline-flex;
		align-items: center;
		gap: var(--space-2);
		margin: 0;
		color: var(--stage-text-2);
		font-size: var(--font-size-label);
		font-weight: var(--font-weight-semibold);
		text-transform: uppercase;
		letter-spacing: 0.12em;
	}

	.panel :global(.close) {
		display: grid;
		place-items: center;
		width: 30px;
		height: 30px;
		border-radius: 50%;
		color: var(--stage-text-2);
		transition: background var(--motion-fast), color var(--motion-fast);
	}

	.panel :global(.close:hover) {
		background: var(--stage-hover);
		color: var(--stage-text);
	}

	.panel :global(.title) {
		margin: 0;
		color: var(--stage-text);
		font-family: var(--font-display);
		font-style: italic;
		font-size: var(--font-size-2xl);
		font-weight: var(--font-weight-semibold);
		line-height: var(--line-height-tight);
		text-wrap: balance;
	}

	/* The artist and scene cards open on a picture: a still that runs to the
	   panel's edges and fades into it at the bottom, then the name and a
	   few plain sentences, as Today's pieces do. */
	.panel :global(.hero) {
		position: relative;
		margin: calc(var(--space-4) * -1) calc(var(--card-pad) * -1) calc(var(--space-1) * -1);
	}

	.panel :global(.hero-pic) {
		position: relative;
		display: block;
		width: 100%;
		aspect-ratio: 16 / 9;
		padding: 0;
		border: 0;
		overflow: hidden;
		background: rgba(255, 255, 255, 0.04);
		cursor: pointer;
		-webkit-mask-image: linear-gradient(to bottom, #000 62%, transparent);
		mask-image: linear-gradient(to bottom, #000 62%, transparent);
	}

	.panel :global(.hero-wait) {
		cursor: default;
		animation: hero-breathe 1.6s ease-in-out infinite alternate;
	}

	@keyframes hero-breathe {
		from { background: rgba(255, 255, 255, 0.03); }
		to { background: rgba(255, 255, 255, 0.07); }
	}

	.panel :global(.hero-img) {
		display: block;
		width: 100%;
		height: 100%;
		object-fit: cover;
		transition: transform var(--motion-slow);
	}

	.panel :global(.hero-pic:hover .hero-img) {
		transform: scale(1.02);
	}

	.panel :global(.hero-pic:focus-visible) {
		outline: 2px solid var(--stage-text);
		outline-offset: -4px;
	}

	.panel :global(.hero-play) {
		position: absolute;
		left: var(--card-pad);
		top: 50%;
		display: grid;
		place-items: center;
		width: 42px;
		height: 42px;
		margin-top: -21px;
		border-radius: 50%;
		background: rgba(0, 0, 0, 0.55);
		color: #fff;
		opacity: 0;
		transition: opacity var(--motion-base);
	}

	.panel :global(.hero-play svg) {
		width: 17px;
		height: 17px;
		fill: currentColor;
	}

	.panel :global(.hero-pic:hover .hero-play),
	.panel :global(.hero-pic:focus-visible .hero-play) {
		opacity: 1;
	}

	/* Close on the picture sits on a dark disc so it reads on any still. */
	.panel :global(.close.on-pic) {
		position: absolute;
		top: var(--space-3);
		right: var(--space-3);
		background: rgba(0, 0, 0, 0.5);
		color: #fff;
	}

	.panel :global(.close.on-pic:hover) {
		background: rgba(0, 0, 0, 0.72);
	}

	.panel :global(.name-row) {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: var(--space-2);
	}

	.panel :global(.name-row .title) {
		font-size: var(--font-size-3xl);
		font-weight: var(--font-weight-medium);
		letter-spacing: -0.01em;
	}

	/* A few plain sentences instead of stats, chips and coloured badges. */
	.panel :global(.lede) {
		margin: 0;
		color: var(--stage-text-2);
		font-size: var(--font-size-sm);
		line-height: var(--line-height-normal);
		text-wrap: pretty;
	}

	/* Rows without a leading mark: the name, then what is known. */
	.panel :global(.row.plain) {
		grid-template-columns: minmax(0, 1fr) auto;
	}

	/* Known artists say so in a word, not a colour. */
	.panel :global(.watched) {
		margin-left: 8px;
		color: var(--stage-text-3);
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-regular, 400);
	}

	.panel :global(.actions) {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space-2);
		margin-top: var(--space-1);
	}

	.panel :global(.actions .btn) {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		height: 36px;
		padding: 0 var(--space-4);
		border-radius: 999px;
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		text-decoration: none;
	}

	.panel :global(.actions .btn svg) {
		width: 15px;
		height: 15px;
		fill: none;
		stroke: currentColor;
		stroke-width: 2;
		stroke-linejoin: round;
	}

	.panel :global(.stage-secondary),
	.stage :global(.probe .stage-secondary) {
		background: rgba(255, 255, 255, 0.1);
		color: var(--stage-text);
	}

	.panel :global(.stage-secondary:hover) {
		background: rgba(255, 255, 255, 0.16);
	}

	.panel :global(.stage-ghost) {
		color: var(--stage-text-2);
	}

	.panel :global(.stage-ghost:hover) {
		color: var(--stage-text);
		background: var(--stage-hover);
	}

	.panel :global(.btn:disabled) {
		opacity: 0.45;
		cursor: default;
	}

	.panel :global(.note) {
		margin: 0;
		color: var(--stage-text-3);
		font-size: var(--font-size-xs);
		line-height: var(--line-height-normal);
	}

	.panel :global(.group) {
		display: grid;
		gap: var(--space-2);
		margin-top: var(--space-2);
	}

	.panel :global(.label) {
		margin: 0;
		color: var(--stage-text-3);
		font-size: var(--font-size-label);
		font-weight: var(--font-weight-semibold);
		text-transform: uppercase;
		letter-spacing: 0.12em;
	}

	.panel :global(.rows) {
		display: grid;
		gap: 2px;
		margin: 0 calc(var(--space-2) * -1);
		padding: 0;
		list-style: none;
	}

	.panel :global(.row) {
		display: grid;
		grid-template-columns: 10px minmax(0, 1fr) auto;
		align-items: center;
		gap: var(--space-3);
		width: 100%;
		min-height: 34px;
		padding: 4px var(--space-2);
		border-radius: var(--radius-sm);
		text-align: left;
		color: var(--stage-text);
		transition: background var(--motion-fast);
	}

	.panel :global(.row.video-row) {
		grid-template-columns: 88px minmax(0, 1fr) auto;
		min-height: 58px;
	}

	.panel :global(.row:hover),
	.panel :global(.row:focus-visible) {
		background: var(--stage-hover);
	}

	.panel :global(.row-text) {
		display: grid;
		gap: 2px;
		min-width: 0;
	}

	.panel :global(.row-name) {
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.panel :global(.row-sub) {
		color: var(--stage-text-3);
		font-size: var(--font-size-2xs);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.panel :global(.row-meta) {
		color: var(--stage-text-3);
		font-family: var(--font-mono);
		font-size: var(--font-size-2xs);
		font-variant-numeric: tabular-nums;
	}

	.panel :global(.row-tag) {
		padding: 2px 7px;
		border-radius: 999px;
		background: rgba(255, 255, 255, 0.06);
		color: var(--stage-text-3);
		font-size: var(--font-size-2xs);
		line-height: 1;
		white-space: nowrap;
	}

	.stage :global(.dot) {
		width: 8px;
		height: 8px;
		border-radius: 50%;
		flex: none;
	}

	.panel :global(.art) {
		display: block;
		aspect-ratio: 16 / 9;
		border-radius: var(--radius-sm);
		overflow: hidden;
		background: rgba(255, 255, 255, 0.05);
	}

	.panel :global(.thumb) {
		display: block;
		width: 100%;
		height: 100%;
		object-fit: cover;
		transition: transform var(--motion-base);
	}

	.panel :global(.thumb.fallback) {
		display: grid;
		place-items: center;
		color: rgba(255, 255, 255, 0.85);
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-semibold);
	}

	.panel :global(.tile:hover .thumb),
	.panel :global(.row:hover .thumb) {
		transform: scale(1.03);
	}

	.panel :global(.error) {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-3);
		padding: var(--space-3);
		border-radius: var(--radius-sm);
		background: rgba(255, 120, 120, 0.08);
	}

	.panel :global(.error p) {
		margin: 0;
		color: var(--stage-text-2);
		font-size: var(--font-size-sm);
	}

	.panel :global(.skeleton) {
		display: grid;
		gap: var(--space-2);
		margin-top: var(--space-2);
	}

	.panel :global(.sk-row) {
		display: block;
		height: 30px;
		border-radius: var(--radius-sm);
		background: linear-gradient(90deg, rgba(255, 255, 255, 0.04), rgba(255, 255, 255, 0.08), rgba(255, 255, 255, 0.04));
		background-size: 200% 100%;
		animation: shimmer 1.4s linear infinite;
	}

	.panel :global(.sk-row.tall) {
		height: 50px;
	}

	@keyframes shimmer {
		from {
			background-position: 100% 0;
		}
		to {
			background-position: -100% 0;
		}
	}

	@media (prefers-reduced-motion: reduce) {
		.panel-inner,
		.results,
		.banner {
			animation: none;
		}

		.panel :global(.sk-row),
		.panel :global(.hero-wait) {
			animation: none;
		}

		.panel :global(.hero-img) {
			transition: none;
		}
	}

	:global([data-motion='reduce']) .panel-inner,
	:global([data-motion='reduce']) .results,
	:global([data-motion='reduce']) .banner {
		animation: none;
	}

	/* ---------------------------------------------------------- phone */

	.compact {
		--card-pad: var(--space-4);
		border-radius: var(--radius-md);
	}

	.compact .legend {
		width: auto;
		max-width: calc(100% - 2 * var(--space-3));
	}


	.compact .hud-right {
		top: auto;
		bottom: var(--space-3);
	}

	.compact .zoom {
		display: none;
	}

	.compact .banner {
		bottom: calc(var(--space-3) + 48px);
	}
</style>
