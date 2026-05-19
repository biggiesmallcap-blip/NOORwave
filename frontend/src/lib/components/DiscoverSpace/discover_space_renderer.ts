// Canvas drawing functions for the DiscoverSpace visualization.
// All functions are pure: they take (ctx, world, camera) and produce pixels.
// No Svelte reactivity inside. Import types only.

import type {
	DiscoverTrackNode,
	DiscoverEdge,
	DiscoverLens,
	DiscoverRouteStep,
	VisitedRegion,
	Camera,
} from './discover_space_types';

// ─── Design constants ─────────────────────────────────────────────────────────

const EDGE_COLORS: Record<string, string> = {
	harmonic:   '#b4a0ff',
	behavioral: '#64b4ff',
	bpm:        '#ffdc50',
	artist:     '#ffa050',
	album:      '#ffa050',
	genre:      '#50dc78',
	energy:     '#50dc78',
	external:   '#7864dc',
	unknown:    '#787888',
};

// Distinct source ring colors — visible against both artwork and star fills
const SOURCE_RING: Record<string, string> = {
	library: '#40c8a8',  // teal
	lastfm:  '#c060e8',  // purple
	engine:  '#4090e0',  // blue
	external:'#5ee6c8',  // mint
	mixed:   '#e09040',  // amber
};

const NEBULA_RGB: Record<string, [number, number, number]> = {
	ambient:     [40,  80, 200],
	pop:         [240, 80, 160],
	punk:        [255, 40,  80],
	rock:        [150, 50, 220],
	alternative: [100,100, 240],
	indie:       [40, 180, 160],
	metal:       [160, 20,  20],
	electronic:  [120, 50, 220],
	dance:       [100, 40, 200],
	house:       [100, 50, 200],
	techno:      [80,  40, 180],
	'hip-hop':   [200,140,  30],
	rap:         [200,140,  30],
	jazz:        [180,140,  40],
	soul:        [200,120,  40],
	country:     [180,140,  60],
	folk:        [140,160,  80],
	classical:   [160,160, 220],
	experimental:[120,180, 220],
};

function nebulaRgb(genre: string): [number, number, number] {
	const g = genre.toLowerCase();
	for (const [key, rgb] of Object.entries(NEBULA_RGB)) {
		if (g.includes(key)) return rgb;
	}
	return [80, 80, 120];
}

function energyColor(energy: number): string {
	const e = Math.max(0, Math.min(1, energy));
	const h = 240 - e * 200;
	return `hsl(${h},${60 + e * 30}%,${45 + e * 15}%)`;
}

function genreFamilyColor(genre: string): string {
	const g = genre.toLowerCase();
	if (g.includes('hip-hop') || g.includes('hip hop') || g.includes('hiphop') || g.includes('rap')) return 'hsl(38,75%,55%)';
	if (g.includes('pop punk') || g.includes('poppunk'))  return 'hsl(20,85%,60%)';
	if (g.includes('punk'))        return 'hsl(350,85%,55%)';
	if (g.includes('metal'))       return 'hsl(0,70%,40%)';
	if (g.includes('rock'))        return 'hsl(270,65%,55%)';
	if (g.includes('alternative') || g.includes('alt-')) return 'hsl(240,60%,60%)';
	if (g.includes('indie'))       return 'hsl(175,55%,50%)';
	if (g.includes('electronic') || g.includes('edm') || g.includes('techno') || g.includes('house') || g.includes('trance')) return 'hsl(270,70%,55%)';
	if (g.includes('dance'))       return 'hsl(260,65%,55%)';
	if (g.includes('ambient'))     return 'hsl(220,70%,55%)';
	if (g.includes('pop'))         return 'hsl(330,75%,65%)';
	if (g.includes('jazz'))        return 'hsl(44,65%,52%)';
	if (g.includes('soul') || g.includes('r&b') || g.includes('rnb')) return 'hsl(30,70%,55%)';
	if (g.includes('country') || g.includes('folk')) return 'hsl(45,55%,50%)';
	if (g.includes('classical') || g.includes('orchestral')) return 'hsl(230,40%,65%)';
	if (g.includes('experimental') || g.includes('avant')) return 'hsl(190,50%,60%)';
	return 'hsl(240,25%,50%)';
}

function nodeColor(node: DiscoverTrackNode, lens: DiscoverLens): string {
	switch (lens) {
		case 'energy':     return energyColor(node.energy ?? 0.5);
		case 'reason':     return EDGE_COLORS[node.primaryReason] ?? EDGE_COLORS.unknown!;
		case 'confidence': return `hsl(220,${30 + node.confidence * 50}%,${30 + node.confidence * 30}%)`;
		case 'source':     return SOURCE_RING[node.source] ?? SOURCE_RING.mixed!;
		case 'genre':      return genreFamilyColor(node.genres[0] ?? node.topGenre ?? '');
		default:           return energyColor(node.energy ?? 0.5);
	}
}

function focusOpacity(trackId: number, hoveredId: number | null, connectedIds: Set<number>): number {
	if (!hoveredId) return 1.0;
	if (trackId === hoveredId) return 1.0;
	if (connectedIds.has(trackId)) return 0.85;
	return 0.38;
}

// ─── Camera transform ─────────────────────────────────────────────────────────
// Hot-loop callers (per-edge, per-node, per-label) inline the math directly to
// avoid a fresh 2-tuple allocation every call (~1k+/frame at 60fps). This
// exported helper is kept for cold paths (selection ripple, route overlay).

export function worldToCanvas(wx: number, wy: number, camera: Camera, cw: number, ch: number): [number, number] {
	return [
		cw / 2 + (wx - camera.x) * camera.zoom,
		ch / 2 + (wy - camera.y) * camera.zoom,
	];
}

// ─── Artwork image cache ──────────────────────────────────────────────────────

const imgCache = new Map<string, HTMLImageElement | 'loading' | 'error'>();

function getCachedImage(url: string): HTMLImageElement | null {
	const cached = imgCache.get(url);
	if (cached instanceof HTMLImageElement) return cached;
	if (cached === 'loading' || cached === 'error') return null;
	imgCache.set(url, 'loading');
	const img = new Image();
	img.onload = () => imgCache.set(url, img);
	img.onerror = () => imgCache.set(url, 'error');
	img.src = url;
	return null;
}

// Zoom-gated: which nodes earn artwork rendering
function shouldShowArtwork(
	node: DiscoverTrackNode,
	zoom: number,
	hoveredId: number | null,
	selectedId: number | null,
	routeTrackIds: Set<number>
): boolean {
	if (!node.artworkUrl) return false;
	// Priority nodes always get artwork
	if (node.isSeed || node.isPlaying) return true;
	if (node.trackId === hoveredId || node.trackId === selectedId) return true;
	if (routeTrackIds.has(node.trackId)) return true;
	// Zoom-gated for regular nodes
	if (zoom >= 1.3) return true;
	if (zoom >= 0.85 && node.score >= 0.55) return true;
	if (zoom >= 0.55 && node.score >= 0.76) return true;
	return false;
}

// Draws artwork image clipped to a circle with an edge vignette so rings stay legible
function drawArtworkFill(
	ctx: CanvasRenderingContext2D,
	img: HTMLImageElement,
	sx: number, sy: number, r: number,
	alpha: number
): void {
	ctx.save();
	ctx.globalAlpha = alpha;
	ctx.beginPath();
	ctx.arc(sx, sy, r, 0, Math.PI * 2);
	ctx.clip();
	// Cover-fit
	const ar = img.naturalWidth / (img.naturalHeight || 1);
	let dw = r * 2, dh = r * 2;
	if (ar > 1) dw = dh * ar; else dh = dw / ar;
	ctx.drawImage(img, sx - dw / 2, sy - dh / 2, dw, dh);
	// Edge vignette — darkens rim so source/status rings read clearly
	const vg = ctx.createRadialGradient(sx, sy, r * 0.45, sx, sy, r);
	vg.addColorStop(0, 'rgba(0,0,0,0)');
	vg.addColorStop(1, 'rgba(0,0,0,0.42)');
	ctx.fillStyle = vg;
	ctx.beginPath();
	ctx.arc(sx, sy, r, 0, Math.PI * 2);
	ctx.fill();
	ctx.restore();
}

// ─── Background + particles ───────────────────────────────────────────────────

const PARTICLE_COUNT = 80;
const particles = Array.from({ length: PARTICLE_COUNT }, () => ({
	x: (Math.random() - 0.5) * 4000,
	y: (Math.random() - 0.5) * 4000,
	r: 0.5 + Math.random() * 1.5,
	speed: 0.1 + Math.random() * 0.3,
	angle: Math.random() * Math.PI * 2,
	phase: Math.random() * Math.PI * 2,
}));
let particleT = 0;

export function drawBackground(ctx: CanvasRenderingContext2D, w: number, h: number, prefersReducedMotion: boolean): void {
	ctx.fillStyle = '#0a0a14';
	ctx.fillRect(0, 0, w, h);

	const grad = ctx.createRadialGradient(w / 2, h / 2, 0, w / 2, h / 2, Math.max(w, h) * 0.7);
	grad.addColorStop(0, 'rgba(20,15,50,0.0)');
	grad.addColorStop(1, 'rgba(5,5,15,0.6)');
	ctx.fillStyle = grad;
	ctx.fillRect(0, 0, w, h);

	if (prefersReducedMotion) return;

	particleT += 0.005;
	ctx.save();
	for (const p of particles) {
		p.x += Math.cos(p.angle) * p.speed;
		p.y += Math.sin(p.angle) * p.speed;
		// Wrap into [-w/2, w/2) / [-h/2, h/2) so screen position stays bounded.
		const wx = ((p.x % w) + w) % w - w / 2;
		const wy = ((p.y % h) + h) % h - h / 2;
		const sx = w / 2 + wx;
		const sy = h / 2 + wy;
		ctx.beginPath();
		ctx.arc(sx, sy, p.r, 0, Math.PI * 2);
		ctx.fillStyle = `rgba(180,180,220,${0.1 + Math.sin(particleT + p.phase) * 0.05})`;
		ctx.fill();
	}
	ctx.restore();
}

// ─── Orbit rings ──────────────────────────────────────────────────────────────

export function drawOrbitRings(ctx: CanvasRenderingContext2D, camera: Camera, cw: number, ch: number): void {
	const sx = cw / 2 + (0 - camera.x) * camera.zoom;
	const sy = ch / 2 + (0 - camera.y) * camera.zoom;
	const rings = [
		{ r: 190, alpha: 0.09 },
		{ r: 350, alpha: 0.06 },
		{ r: 540, alpha: 0.04 },
	];
	ctx.save();
	ctx.strokeStyle = 'rgba(124,128,255,1)';
	ctx.lineWidth = 0.8;
	ctx.setLineDash([6, 14]);
	for (const ring of rings) {
		ctx.globalAlpha = ring.alpha;
		ctx.beginPath();
		ctx.arc(sx, sy, ring.r * camera.zoom, 0, Math.PI * 2);
		ctx.stroke();
	}
	ctx.setLineDash([]);
	ctx.restore();
}

// ─── Selection ripple ─────────────────────────────────────────────────────────

export function drawSelectionRipple(
	ctx: CanvasRenderingContext2D,
	sx: number, sy: number,
	r: number,
	progress: number
): void {
	const rippleR = r + progress * 55;
	ctx.save();
	ctx.globalAlpha = (1 - progress) * 0.55;
	ctx.strokeStyle = 'rgba(255,255,255,0.9)';
	ctx.lineWidth = Math.max(0.5, 2 - progress * 1.5);
	ctx.beginPath();
	ctx.arc(sx, sy, rippleR, 0, Math.PI * 2);
	ctx.stroke();
	ctx.restore();
}

// ─── Visited regions ──────────────────────────────────────────────────────────

export function drawVisitedRegions(
	ctx: CanvasRenderingContext2D,
	regions: VisitedRegion[],
	camera: Camera,
	cw: number, ch: number
): void {
	for (const region of regions) {
		const sx = cw / 2 + (region.centroid.x - camera.x) * camera.zoom;
		const sy = ch / 2 + (region.centroid.y - camera.y) * camera.zoom;
		const sr = region.centroid.radius * camera.zoom;
		const grad = ctx.createRadialGradient(sx, sy, 0, sx, sy, sr);
		grad.addColorStop(0, 'rgba(100,100,200,0.08)');
		grad.addColorStop(0.6, 'rgba(80,80,180,0.04)');
		grad.addColorStop(1, 'rgba(60,60,160,0.0)');
		ctx.fillStyle = grad;
		ctx.beginPath();
		ctx.arc(sx, sy, sr, 0, Math.PI * 2);
		ctx.fill();
		if (camera.zoom > 0.5) {
			ctx.save();
			ctx.fillStyle = 'rgba(160,160,200,0.35)';
			ctx.font = `${Math.max(9, 11 * camera.zoom)}px system-ui, sans-serif`;
			ctx.textAlign = 'center';
			ctx.textBaseline = 'middle';
			ctx.fillText(region.label, sx, sy - sr * 0.8);
			ctx.restore();
		}
	}
}

// ─── Genre nebulae ────────────────────────────────────────────────────────────

interface NebulaCluster { genre: string; cx: number; cy: number; r: number; count: number; }

// Cached cluster geometry — invalidated when the node identity-tuple or
// topology changes. Recomputing every frame allocates a Map + per-genre arrays
// and does two reduce-passes per group; stable nodes don't need that.
let _nebulaCacheKey: string | null = null;
let _nebulaCache: NebulaCluster[] = [];

function nebulaCacheKey(nodes: DiscoverTrackNode[]): string {
	// Cheap fingerprint: count + first/last id + length-based bucket of total positions.
	if (nodes.length === 0) return '0';
	const first = nodes[0]!;
	const last = nodes[nodes.length - 1]!;
	return `${nodes.length}:${first.trackId}:${last.trackId}`;
}

function buildNebulaClusters(nodes: DiscoverTrackNode[]): NebulaCluster[] {
	const groups = new Map<string, { sx: number; sy: number; count: number; maxD2: number; cxApprox: number; cyApprox: number }>();
	// First pass: centroids
	for (const node of nodes) {
		const key = node.genres[0] ?? node.topGenre ?? 'unknown';
		const e = groups.get(key);
		if (e) { e.sx += node.x; e.sy += node.y; e.count += 1; }
		else groups.set(key, { sx: node.x, sy: node.y, count: 1, maxD2: 0, cxApprox: 0, cyApprox: 0 });
	}
	for (const e of groups.values()) {
		e.cxApprox = e.sx / e.count;
		e.cyApprox = e.sy / e.count;
	}
	// Second pass: radius
	for (const node of nodes) {
		const key = node.genres[0] ?? node.topGenre ?? 'unknown';
		const e = groups.get(key)!;
		const dx = node.x - e.cxApprox;
		const dy = node.y - e.cyApprox;
		const d2 = dx * dx + dy * dy;
		if (d2 > e.maxD2) e.maxD2 = d2;
	}
	const out: NebulaCluster[] = [];
	for (const [genre, e] of groups) {
		if (e.count < 2) continue;
		const r = Math.max(80, Math.sqrt(e.maxD2) * 1.3);
		out.push({ genre, cx: e.cxApprox, cy: e.cyApprox, r, count: e.count });
	}
	return out;
}

export function invalidateNebulaCache(): void {
	_nebulaCacheKey = null;
	_nebulaCache = [];
}

export function drawGenreNebulae(
	ctx: CanvasRenderingContext2D,
	nodes: DiscoverTrackNode[],
	camera: Camera,
	cw: number, ch: number,
	lens: DiscoverLens
): void {
	const key = nebulaCacheKey(nodes);
	if (key !== _nebulaCacheKey) {
		_nebulaCache = buildNebulaClusters(nodes);
		_nebulaCacheKey = key;
	}
	const baseOpacity = lens === 'genre' ? 0.28 : 0.13;

	for (const cluster of _nebulaCache) {
		const sx = cw / 2 + (cluster.cx - camera.x) * camera.zoom;
		const sy = ch / 2 + (cluster.cy - camera.y) * camera.zoom;
		const sr = cluster.r * camera.zoom;

		const [nr, ng, nb] = nebulaRgb(cluster.genre);
		const grad = ctx.createRadialGradient(sx, sy, 0, sx, sy, sr);
		grad.addColorStop(0,   `rgba(${nr},${ng},${nb},${baseOpacity})`);
		grad.addColorStop(0.5, `rgba(${nr},${ng},${nb},${baseOpacity * 0.45})`);
		grad.addColorStop(1,   'rgba(0,0,0,0)');
		ctx.fillStyle = grad;
		ctx.beginPath();
		ctx.arc(sx, sy, sr, 0, Math.PI * 2);
		ctx.fill();

		if (camera.zoom > 0.55 && cluster.count >= 3) {
			ctx.save();
			ctx.fillStyle = `rgba(${nr},${ng},${nb},${Math.min(0.6, baseOpacity * 3.5)})`;
			ctx.font = `${Math.max(9, Math.min(13, 10 * camera.zoom))}px system-ui, sans-serif`;
			ctx.textAlign = 'center';
			ctx.textBaseline = 'middle';
			ctx.fillText(cluster.genre, sx, sy);
			ctx.restore();
		}
	}
}

// ─── Edges ────────────────────────────────────────────────────────────────────
//
// Three tiers:
//   strong  weight >= 0.72 — always shown, soft glow, thicker
//   medium  weight >= 0.45 — shown at normal zoom
//   weak    below 0.45     — shown only on hover / select / seed / route focus

export function drawEdges(
	ctx: CanvasRenderingContext2D,
	edges: DiscoverEdge[],
	nodeMap: Map<number, DiscoverTrackNode>,
	camera: Camera,
	cw: number, ch: number,
	zoom: number,
	seedTrackId: number | null,
	hoveredTrackId: number | null,
	selectedTrackId: number | null,
	routeTrackIds: Set<number>
): void {
	// Two-pass: strong edges first (glow), then the rest
	for (let pass = 0; pass < 2; pass++) {
		const drawingStrong = pass === 0;

		for (const edge of edges) {
			const from = nodeMap.get(edge.fromTrackId);
			const to   = nodeMap.get(edge.toTrackId);
			if (!from || !to) continue;

			const isStrong   = edge.weight >= 0.72;
			const isMedium   = edge.weight >= 0.45;

			if (drawingStrong !== isStrong) continue;

			const connectsSeed     = edge.fromTrackId === seedTrackId || edge.toTrackId === seedTrackId;
			const connectsHovered  = hoveredTrackId !== null && (edge.fromTrackId === hoveredTrackId  || edge.toTrackId === hoveredTrackId);
			const connectsSelected = selectedTrackId !== null && (edge.fromTrackId === selectedTrackId || edge.toTrackId === selectedTrackId);
			const isRouteEdge      = routeTrackIds.has(edge.fromTrackId) && routeTrackIds.has(edge.toTrackId);

			// Visibility: strong always shown; medium shown at mid+ zoom; weak only on focus
			if (isStrong) {
				// always visible
			} else if (isMedium) {
				if (!connectsSeed && !connectsHovered && !connectsSelected && !isRouteEdge && zoom < 0.5) continue;
			} else {
				if (!connectsSeed && !connectsHovered && !connectsSelected && !isRouteEdge) continue;
			}

			const x1 = cw / 2 + (from.x - camera.x) * camera.zoom;
			const y1 = ch / 2 + (from.y - camera.y) * camera.zoom;
			const x2 = cw / 2 + (to.x   - camera.x) * camera.zoom;
			const y2 = ch / 2 + (to.y   - camera.y) * camera.zoom;

			const isHighlighted = connectsHovered || connectsSelected;
			const col = EDGE_COLORS[edge.reason] ?? EDGE_COLORS.unknown!;

			let baseAlpha: number;
			let thickness: number;

			if (isStrong) {
				baseAlpha = isHighlighted ? 0.85 : connectsSeed ? 0.60 : 0.42;
				thickness = 1.0 + edge.weight * (isHighlighted ? 3.5 : 2.5);
			} else if (isMedium) {
				baseAlpha = isHighlighted ? 0.65 : connectsSeed ? 0.32 : 0.18;
				thickness = 0.4 + edge.weight * (isHighlighted ? 3.0 : 1.8);
			} else {
				baseAlpha = isHighlighted ? 0.45 : connectsSeed ? 0.22 : 0.12;
				thickness = 0.3 + edge.weight * 1.2;
			}

			const alpha = baseAlpha * (0.35 + edge.confidence * 0.65) * Math.min(1, zoom * 1.4);

			ctx.save();
			ctx.globalAlpha = alpha;
			ctx.strokeStyle = col;
			ctx.lineWidth = thickness;

			// Glow on strong edges: draw a wider, blurred shadow pass
			if (isStrong) {
				ctx.shadowColor = col;
				ctx.shadowBlur = isHighlighted ? 14 : 8;
			}

			ctx.beginPath();
			if (edge.reason === 'harmonic' || isStrong) {
				// Slight curve on harmonic and strong edges for elegance
				const mx = (x1 + x2) / 2 + (y2 - y1) * 0.12;
				const my = (y1 + y2) / 2 - (x2 - x1) * 0.12;
				ctx.moveTo(x1, y1);
				ctx.quadraticCurveTo(mx, my, x2, y2);
			} else if (edge.reason === 'bpm') {
				ctx.setLineDash([3 * Math.max(0.5, zoom), 4 * Math.max(0.5, zoom)]);
				ctx.moveTo(x1, y1);
				ctx.lineTo(x2, y2);
			} else {
				ctx.moveTo(x1, y1);
				ctx.lineTo(x2, y2);
			}

			ctx.stroke();
			ctx.shadowBlur = 0;
			ctx.setLineDash([]);
			ctx.restore();
		}
	}
}

// ─── Nodes ────────────────────────────────────────────────────────────────────

export function drawNodes(
	ctx: CanvasRenderingContext2D,
	nodes: DiscoverTrackNode[],
	camera: Camera,
	cw: number, ch: number,
	lens: DiscoverLens,
	hoveredTrackId: number | null,
	selectedTrackId: number | null,
	connectedIds: Set<number>,
	t: number,
	prefersReducedMotion: boolean,
	routeTrackIds: Set<number>
): void {
	for (const node of nodes) {
		if (node.isSeed || node.isPlaying) continue;
		drawSingleNode(ctx, node, camera, cw, ch, lens, hoveredTrackId, selectedTrackId, connectedIds, t, prefersReducedMotion, routeTrackIds);
	}
}

function drawSingleNode(
	ctx: CanvasRenderingContext2D,
	node: DiscoverTrackNode,
	camera: Camera,
	cw: number, ch: number,
	lens: DiscoverLens,
	hoveredTrackId: number | null,
	selectedTrackId: number | null,
	connectedIds: Set<number>,
	t: number,
	prefersReducedMotion: boolean,
	routeTrackIds: Set<number>
): void {
	const sx = cw / 2 + (node.x - camera.x) * camera.zoom;
	const sy = ch / 2 + (node.y - camera.y) * camera.zoom;
	const roleScale = node.role === 'external_candidate' ? 1.1 : node.role === 'library_guide' ? 0.86 : 1;
	const r = Math.max(4, node.radius * roleScale * camera.zoom);
	const isHovered  = node.trackId === hoveredTrackId;
	const isSelected = node.trackId === selectedTrackId;
	const roleOpacity = node.role === 'library_guide' ? 0.62 : node.playability === 'pending' ? 0.78 : 1;
	const fo = focusOpacity(node.trackId, hoveredTrackId, connectedIds) * roleOpacity;

	const withArtwork = shouldShowArtwork(node, camera.zoom, hoveredTrackId, selectedTrackId, routeTrackIds);
	const img = withArtwork && node.artworkUrl ? getCachedImage(node.artworkUrl) : null;

	ctx.save();

	// 1. Soft outer glow (behind everything)
	const breathe = prefersReducedMotion ? 1 : 1 + Math.sin(t * 0.02 + node.trackId * 0.1) * 0.025;
	const glowR = r * (img ? 2.2 : 2.8) * breathe;
	const glowStrength = (0.09 + node.confidence * 0.16 + (node.role === 'external_candidate' ? 0.08 : 0)) * fo;
	const glowCol = node.role === 'external_candidate' ? 'rgba(94,230,200,' : img ? 'rgba(220,220,255,' : 'rgba(180,185,255,';
	const glowGrad = ctx.createRadialGradient(sx, sy, r * 0.4, sx, sy, glowR);
	glowGrad.addColorStop(0, `${glowCol}${glowStrength})`);
	glowGrad.addColorStop(1, 'rgba(0,0,0,0)');
	ctx.fillStyle = glowGrad;
	ctx.beginPath();
	ctx.arc(sx, sy, glowR, 0, Math.PI * 2);
	ctx.fill();

	if (img) {
		// ── Artwork rendering path ─────────────────────────────────────
		// Use a slightly larger effective radius for artwork readability
		const ar = Math.max(r, 10);
		const artAlpha = (node.isColdStart ? 0.60 : 0.92) * fo;
		drawArtworkFill(ctx, img, sx, sy, ar, artAlpha);

		// Source ring (outside clip, over artwork)
		const ringCol = SOURCE_RING[node.source] ?? SOURCE_RING.mixed!;
		const ringW = Math.max(1.2, 2.0 * Math.min(1, camera.zoom));
		const ringAlpha = (node.isColdStart ? 0.45 : 0.80) * fo;
		ctx.globalAlpha = ringAlpha;
		ctx.strokeStyle = ringCol;
		ctx.lineWidth = ringW;
		ctx.beginPath();
		ctx.arc(sx, sy, ar + ringW, 0, Math.PI * 2);
		ctx.stroke();

	} else {
		// ── Star fallback rendering path ───────────────────────────────
		const col = nodeColor(node, lens);
		const coreAlpha = (node.isColdStart ? 0.55 : 0.90) * fo;

		// Cold-start shimmer ring
		if (node.isColdStart && !prefersReducedMotion) {
			const shimmer = (0.10 + Math.sin(t * 0.02 + node.trackId) * 0.05) * fo;
			ctx.strokeStyle = `rgba(180,180,210,${shimmer})`;
			ctx.lineWidth = 1;
			ctx.beginPath();
			ctx.arc(sx, sy, r + Math.min(4, 4 * camera.zoom), 0, Math.PI * 2);
			ctx.stroke();
		}

		// Core fill
		ctx.globalAlpha = coreAlpha;
		ctx.fillStyle = col;
		ctx.beginPath();
		ctx.arc(sx, sy, r, 0, Math.PI * 2);
		ctx.fill();

		// Specular highlight
		const specGrad = ctx.createRadialGradient(sx - r * 0.25, sy - r * 0.3, 0, sx, sy, r);
		specGrad.addColorStop(0, 'rgba(255,255,255,0.70)');
		specGrad.addColorStop(0.45, 'rgba(255,255,255,0.12)');
		specGrad.addColorStop(1, 'rgba(0,0,0,0)');
		ctx.fillStyle = specGrad;
		ctx.beginPath();
		ctx.arc(sx, sy, r, 0, Math.PI * 2);
		ctx.fill();

		// Thin source ring
		const ringCol = SOURCE_RING[node.source] ?? SOURCE_RING.mixed!;
		ctx.globalAlpha = (node.isColdStart ? 0.35 : 0.55) * fo;
		ctx.strokeStyle = ringCol;
		ctx.lineWidth = Math.max(0.5, 1.0 * camera.zoom);
		ctx.beginPath();
		ctx.arc(sx, sy, r + Math.max(1, 1.5 * camera.zoom), 0, Math.PI * 2);
		ctx.stroke();
	}

	// ── Status rings (same for both paths) ────────────────────────────────────

	ctx.globalAlpha = fo;

	// Playlist ring (gold)
	if (node.inPlaylistBuilder) {
		ctx.strokeStyle = 'rgba(255,200,50,0.9)';
		ctx.lineWidth = 1.8;
		ctx.beginPath();
		ctx.arc(sx, sy, (img ? Math.max(r, 10) : r) + Math.max(3.5, 4.5 * camera.zoom), 0, Math.PI * 2);
		ctx.stroke();
	}

	// Hover / selected ring (full opacity, always on top)
	if (isHovered || isSelected) {
		ctx.globalAlpha = 1;
		ctx.strokeStyle = isSelected ? 'rgba(255,255,255,0.92)' : 'rgba(255,255,255,0.52)';
		ctx.lineWidth = isSelected ? 2.2 : 1.2;
		ctx.beginPath();
		ctx.arc(sx, sy, (img ? Math.max(r, 10) : r) + Math.max(4, 5 * camera.zoom), 0, Math.PI * 2);
		ctx.stroke();
	}

	if (node.playability === 'pending') {
		ctx.globalAlpha = 0.55 * fo;
		ctx.setLineDash([3, 4]);
		ctx.strokeStyle = 'rgba(255,255,255,0.58)';
		ctx.lineWidth = Math.max(0.8, 1.1 * camera.zoom);
		ctx.beginPath();
		ctx.arc(sx, sy, (img ? Math.max(r, 10) : r) + Math.max(6, 7 * camera.zoom), 0, Math.PI * 2);
		ctx.stroke();
		ctx.setLineDash([]);
	}

	ctx.restore();
}

// ─── Seed (Anchor Star) ───────────────────────────────────────────────────────

export function drawSeedNode(
	ctx: CanvasRenderingContext2D,
	node: DiscoverTrackNode,
	camera: Camera,
	cw: number, ch: number,
	isLocked: boolean,
	t: number,
	prefersReducedMotion: boolean
): void {
	const sx = cw / 2 + (node.x - camera.x) * camera.zoom;
	const sy = ch / 2 + (node.y - camera.y) * camera.zoom;
	const r = Math.max(10, node.radius * camera.zoom);
	const img = node.artworkUrl ? getCachedImage(node.artworkUrl) : null;

	ctx.save();

	// White-violet outer halo
	if (!prefersReducedMotion) {
		const pulseR = r * (3.2 + Math.sin(t * 0.025) * 0.5);
		const haloGrad = ctx.createRadialGradient(sx, sy, r, sx, sy, pulseR);
		haloGrad.addColorStop(0, 'rgba(100,112,255,0.38)');
		haloGrad.addColorStop(0.5, 'rgba(80,92,220,0.10)');
		haloGrad.addColorStop(1, 'rgba(60,72,200,0.0)');
		ctx.fillStyle = haloGrad;
		ctx.beginPath();
		ctx.arc(sx, sy, pulseR, 0, Math.PI * 2);
		ctx.fill();

		// Heartbeat ring
		const ring2R = r * (1.8 + Math.sin(t * 0.04) * 0.4);
		ctx.strokeStyle = `rgba(200,210,255,${0.38 + Math.sin(t * 0.04) * 0.16})`;
		ctx.lineWidth = 1.8;
		ctx.beginPath();
		ctx.arc(sx, sy, ring2R, 0, Math.PI * 2);
		ctx.stroke();
	}

	if (img) {
		drawArtworkFill(ctx, img, sx, sy, r, 0.95);

		// Heavy white-violet seed border
		ctx.strokeStyle = isLocked ? 'rgba(220,200,255,0.95)' : 'rgba(180,190,255,0.85)';
		ctx.lineWidth = Math.max(2.5, 3 * Math.min(1, camera.zoom));
		ctx.beginPath();
		ctx.arc(sx, sy, r + 2, 0, Math.PI * 2);
		ctx.stroke();
	} else {
		// Star fallback
		const seedGrad = ctx.createRadialGradient(sx - r * 0.3, sy - r * 0.3, 0, sx, sy, r);
		seedGrad.addColorStop(0, '#d0d8ff');
		seedGrad.addColorStop(0.5, '#7080f0');
		seedGrad.addColorStop(1, '#3848c8');
		ctx.fillStyle = seedGrad;
		ctx.beginPath();
		ctx.arc(sx, sy, r, 0, Math.PI * 2);
		ctx.fill();

		const specGrad = ctx.createRadialGradient(sx - r * 0.3, sy - r * 0.35, 0, sx, sy, r);
		specGrad.addColorStop(0, 'rgba(255,255,255,0.80)');
		specGrad.addColorStop(0.4, 'rgba(255,255,255,0.12)');
		specGrad.addColorStop(1, 'rgba(0,0,0,0)');
		ctx.fillStyle = specGrad;
		ctx.beginPath();
		ctx.arc(sx, sy, r, 0, Math.PI * 2);
		ctx.fill();

		ctx.strokeStyle = isLocked ? 'rgba(220,200,255,0.9)' : 'rgba(160,180,255,0.7)';
		ctx.lineWidth = 2;
		ctx.beginPath();
		ctx.arc(sx, sy, r + 2, 0, Math.PI * 2);
		ctx.stroke();
	}

	if (isLocked) {
		ctx.fillStyle = 'rgba(255,255,255,0.92)';
		ctx.font = `${Math.max(10, r * 0.75)}px system-ui`;
		ctx.textAlign = 'center';
		ctx.textBaseline = 'middle';
		ctx.fillText('🔒', sx + r * 0.55, sy - r * 0.55);
	}

	ctx.restore();
}

// ─── Playing (Signal Star) ────────────────────────────────────────────────────

export function drawPlayingNode(
	ctx: CanvasRenderingContext2D,
	node: DiscoverTrackNode,
	camera: Camera,
	cw: number, ch: number,
	t: number,
	prefersReducedMotion: boolean
): void {
	const sx = cw / 2 + (node.x - camera.x) * camera.zoom;
	const sy = ch / 2 + (node.y - camera.y) * camera.zoom;
	const r = Math.max(7, node.radius * camera.zoom);
	const img = node.artworkUrl ? getCachedImage(node.artworkUrl) : null;

	ctx.save();

	// Lavender playing pulse rings
	if (!prefersReducedMotion) {
		for (let i = 0; i < 2; i++) {
			const phase = t * 0.033 + i * Math.PI;
			const pulseR = r * (1.7 + Math.sin(phase) * 0.5);
			ctx.strokeStyle = `rgba(180,160,255,${0.38 - i * 0.12})`;
			ctx.lineWidth = 1.6;
			ctx.beginPath();
			ctx.arc(sx, sy, pulseR, 0, Math.PI * 2);
			ctx.stroke();
		}
	}

	if (img) {
		drawArtworkFill(ctx, img, sx, sy, r, 0.95);

		// Lavender playing border
		ctx.strokeStyle = 'rgba(200,180,255,0.90)';
		ctx.lineWidth = Math.max(2, 2.5 * Math.min(1, camera.zoom));
		ctx.beginPath();
		ctx.arc(sx, sy, r + 2, 0, Math.PI * 2);
		ctx.stroke();
	} else {
		const playGrad = ctx.createRadialGradient(sx - r * 0.3, sy - r * 0.3, 0, sx, sy, r);
		playGrad.addColorStop(0, '#f0ecff');
		playGrad.addColorStop(0.5, '#b4a0f0');
		playGrad.addColorStop(1, '#7060c0');
		ctx.fillStyle = playGrad;
		ctx.beginPath();
		ctx.arc(sx, sy, r, 0, Math.PI * 2);
		ctx.fill();

		const specGrad = ctx.createRadialGradient(sx - r * 0.25, sy - r * 0.3, 0, sx, sy, r);
		specGrad.addColorStop(0, 'rgba(255,255,255,0.75)');
		specGrad.addColorStop(0.4, 'rgba(255,255,255,0.10)');
		specGrad.addColorStop(1, 'rgba(0,0,0,0)');
		ctx.fillStyle = specGrad;
		ctx.beginPath();
		ctx.arc(sx, sy, r, 0, Math.PI * 2);
		ctx.fill();
	}

	ctx.restore();
}

// ─── Labels ───────────────────────────────────────────────────────────────────

export function drawLabels(
	ctx: CanvasRenderingContext2D,
	nodes: DiscoverTrackNode[],
	camera: Camera,
	cw: number, ch: number,
	hoveredTrackId: number | null,
	selectedTrackId: number | null,
	zoom: number
): void {
	const isHighZoom = zoom >= 1.15;
	const isMidZoom  = zoom >= 0.70;
	const scoreThreshold = isHighZoom ? 0.65 : 0.82;
	const maxLen = isHighZoom ? 28 : 18;
	const baseFontSize = Math.max(9, Math.min(13, 11 * zoom));

	ctx.save();
	ctx.textAlign = 'center';
	ctx.textBaseline = 'top';
	ctx.font = `${baseFontSize}px system-ui, sans-serif`;

	for (const node of nodes) {
		const isPriority = node.isSeed || node.isPlaying
			|| node.trackId === hoveredTrackId
			|| node.trackId === selectedTrackId;

		const showLabel = isPriority
			|| (isMidZoom && node.score > scoreThreshold)
			|| node.inPlaylistBuilder;

		if (!showLabel) continue;

		const sx = cw / 2 + (node.x - camera.x) * camera.zoom;
		const sy = ch / 2 + (node.y - camera.y) * camera.zoom;
		const r = Math.max(4, node.radius * zoom);
		const raw = node.title;
		const label = raw.length > maxLen ? raw.slice(0, maxLen - 1) + '…' : raw;

		ctx.shadowColor = 'rgba(0,0,0,0.88)';
		ctx.shadowBlur = 4;
		ctx.fillStyle = node.isSeed           ? 'rgba(255,255,255,0.98)'
			: node.isPlaying                   ? 'rgba(220,200,255,0.95)'
			: node.trackId === selectedTrackId ? 'rgba(255,255,255,0.90)'
			: 'rgba(200,200,225,0.72)';
		ctx.fillText(label, sx, sy + r + 4);

		if (isHighZoom && (node.trackId === hoveredTrackId || node.trackId === selectedTrackId)) {
			ctx.fillStyle = 'rgba(160,160,190,0.58)';
			ctx.font = `${baseFontSize - 1}px system-ui, sans-serif`;
			ctx.fillText(node.artist, sx, sy + r + 4 + baseFontSize + 2);
			ctx.font = `${baseFontSize}px system-ui, sans-serif`;
		}
	}

	ctx.shadowBlur = 0;
	ctx.restore();
}

// ─── Radio route overlay ──────────────────────────────────────────────────────

export function drawRadioRoute(
	ctx: CanvasRenderingContext2D,
	route: DiscoverRouteStep[],
	nodeMap: Map<number, DiscoverTrackNode>,
	camera: Camera,
	cw: number, ch: number,
	zoom: number
): void {
	if (route.length < 2) return;

	const positions: Array<[number, number]> = [];
	for (const step of route) {
		const node = nodeMap.get(step.trackId);
		if (node) positions.push([
			cw / 2 + (node.x - camera.x) * camera.zoom,
			ch / 2 + (node.y - camera.y) * camera.zoom,
		]);
	}
	if (positions.length < 2) return;

	ctx.save();
	ctx.strokeStyle = 'rgba(160,140,255,0.50)';
	ctx.lineWidth = 1.5;
	ctx.beginPath();
	ctx.moveTo(positions[0]![0], positions[0]![1]);
	for (let i = 1; i < positions.length; i++) {
		const [x1, y1] = positions[i - 1]!;
		const [x2, y2] = positions[i]!;
		const mx = (x1 + x2) / 2 + (y2 - y1) * 0.1;
		const my = (y1 + y2) / 2 - (x2 - x1) * 0.1;
		ctx.quadraticCurveTo(mx, my, x2, y2);
	}
	ctx.stroke();

	if (zoom >= 1.1) {
		ctx.font = `${Math.max(8, 9 * zoom)}px system-ui`;
		ctx.textAlign = 'center';
		ctx.textBaseline = 'middle';
		for (let i = 0; i < route.length; i++) {
			const step = route[i]!;
			const pos = positions[i];
			if (!pos) continue;
			ctx.fillStyle = step.isCurrent ? 'rgba(220,200,255,0.9)' : 'rgba(140,120,220,0.6)';
			ctx.fillText(`${i + 1}`, pos[0], pos[1] - 16 * zoom);
		}
	}
	ctx.restore();
}

// ─── Warp streaks ─────────────────────────────────────────────────────────────

export function drawWarpStreaks(
	ctx: CanvasRenderingContext2D,
	progress: number,
	w: number,
	h: number
): void {
	if (progress <= 0) return;
	ctx.save();
	ctx.globalAlpha = progress;
	const cx = w / 2;
	const cy = h / 2;
	for (let i = 0; i < 40; i++) {
		const angle = (i / 40) * Math.PI * 2;
		const len = (100 + Math.random() * 200) * progress;
		const r0 = 50 * (1 - progress);
		ctx.strokeStyle = `rgba(160,140,255,${0.3 + progress * 0.4})`;
		ctx.lineWidth = 1 + progress;
		ctx.beginPath();
		ctx.moveTo(cx + Math.cos(angle) * r0, cy + Math.sin(angle) * r0);
		ctx.lineTo(cx + Math.cos(angle) * (r0 + len), cy + Math.sin(angle) * (r0 + len));
		ctx.stroke();
	}
	ctx.restore();
}
