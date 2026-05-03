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

// Edge color per reason (solid reference colors — alpha applied separately)
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

// Genre-family nebula background: RGB tuples, opacity applied per-draw
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

// ─── Genre family colors (solid, for Genre lens node rendering) ───────────────

export function energyColor(energy: number): string {
	const e = Math.max(0, Math.min(1, energy));
	const h = 240 - e * 200; // blue → orange
	const s = 60 + e * 30;
	const l = 45 + e * 15;
	return `hsl(${h},${s}%,${l}%)`;
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
		case 'energy':
			return energyColor(node.energy ?? 0.5);
		case 'reason':
			return EDGE_COLORS[node.primaryReason] ?? EDGE_COLORS.unknown!;
		case 'confidence':
			return `hsl(220,${30 + node.confidence * 50}%,${30 + node.confidence * 30}%)`;
		case 'source':
			return node.source === 'library' ? 'hsl(230,65%,60%)'
				 : node.source === 'lastfm'  ? 'hsl(350,65%,55%)'
				 : node.source === 'engine'  ? 'hsl(145,50%,50%)'
				 : 'hsl(280,45%,55%)';
		case 'genre':
			return genreFamilyColor(node.genres[0] ?? node.topGenre ?? '');
		default:
			return energyColor(node.energy ?? 0.5);
	}
}

// ─── Focus opacity (hover dimming) ───────────────────────────────────────────

function focusOpacity(
	trackId: number,
	hoveredId: number | null,
	connectedIds: Set<number>
): number {
	if (!hoveredId) return 1.0;
	if (trackId === hoveredId) return 1.0;
	if (connectedIds.has(trackId)) return 0.85;
	return 0.38;
}

// ─── Camera transform helpers ─────────────────────────────────────────────────

export function worldToCanvas(
	wx: number, wy: number, camera: Camera, cw: number, ch: number
): [number, number] {
	return [
		cw / 2 + (wx - camera.x) * camera.zoom,
		ch / 2 + (wy - camera.y) * camera.zoom,
	];
}

// ─── Background + slow particles ─────────────────────────────────────────────

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

export function drawBackground(
	ctx: CanvasRenderingContext2D,
	w: number,
	h: number,
	prefersReducedMotion: boolean
): void {
	ctx.fillStyle = '#0a0a14';
	ctx.fillRect(0, 0, w, h);

	// Radial vignette
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
		if (Math.abs(p.x) > 2200) p.x *= -1;
		if (Math.abs(p.y) > 2200) p.y *= -1;
		const sx = w / 2 + (p.x % w);
		const sy = h / 2 + (p.y % h);
		ctx.beginPath();
		ctx.arc(sx, sy, p.r, 0, Math.PI * 2);
		ctx.fillStyle = `rgba(180,180,220,${0.1 + Math.sin(particleT + p.phase) * 0.05})`;
		ctx.fill();
	}
	ctx.restore();
}

// ─── Orbit rings (static decorative rings around the Anchor Star) ─────────────

export function drawOrbitRings(
	ctx: CanvasRenderingContext2D,
	camera: Camera,
	cw: number,
	ch: number
): void {
	// Seed is always at world origin (0,0)
	const [sx, sy] = worldToCanvas(0, 0, camera, cw, ch);
	// Radii chosen to match the three initial-orbit tiers in the adapter
	const rings = [
		{ r: 190, alpha: 0.09 },  // near orbit  (high-score library)
		{ r: 350, alpha: 0.06 },  // related field (mid-score)
		{ r: 540, alpha: 0.04 },  // deep signal   (cold-start / external)
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

// ─── Selection ripple (one-shot ring on click) ────────────────────────────────

export function drawSelectionRipple(
	ctx: CanvasRenderingContext2D,
	sx: number,
	sy: number,
	r: number,
	progress: number // 0..1
): void {
	const rippleR = r + progress * 55;
	const alpha   = (1 - progress) * 0.55;
	ctx.save();
	ctx.globalAlpha = alpha;
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
	cw: number,
	ch: number
): void {
	for (const region of regions) {
		const [sx, sy] = worldToCanvas(region.centroid.x, region.centroid.y, camera, cw, ch);
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

export function drawGenreNebulae(
	ctx: CanvasRenderingContext2D,
	nodes: DiscoverTrackNode[],
	camera: Camera,
	cw: number,
	ch: number,
	lens: DiscoverLens
): void {
	const groups = new Map<string, { x: number; y: number }[]>();
	for (const node of nodes) {
		const key = node.genres[0] ?? node.topGenre ?? 'unknown';
		const arr = groups.get(key) ?? [];
		arr.push({ x: node.x, y: node.y });
		groups.set(key, arr);
	}

	const baseOpacity = lens === 'genre' ? 0.28 : 0.13;

	for (const [genre, positions] of groups) {
		if (positions.length < 2) continue;
		const cx = positions.reduce((s, p) => s + p.x, 0) / positions.length;
		const cy = positions.reduce((s, p) => s + p.y, 0) / positions.length;
		const maxDist = Math.max(...positions.map((p) => Math.sqrt((p.x - cx) ** 2 + (p.y - cy) ** 2)));
		const r = Math.max(80, maxDist * 1.3);
		const [sx, sy] = worldToCanvas(cx, cy, camera, cw, ch);
		const sr = r * camera.zoom;

		const [nr, ng, nb] = nebulaRgb(genre);
		const grad = ctx.createRadialGradient(sx, sy, 0, sx, sy, sr);
		grad.addColorStop(0,   `rgba(${nr},${ng},${nb},${baseOpacity})`);
		grad.addColorStop(0.5, `rgba(${nr},${ng},${nb},${baseOpacity * 0.45})`);
		grad.addColorStop(1,   'rgba(0,0,0,0)');
		ctx.fillStyle = grad;
		ctx.beginPath();
		ctx.arc(sx, sy, sr, 0, Math.PI * 2);
		ctx.fill();

		// Genre label (only with ≥ 3 nodes and enough zoom)
		if (camera.zoom > 0.55 && positions.length >= 3) {
			ctx.save();
			ctx.fillStyle = `rgba(${nr},${ng},${nb},${Math.min(0.6, baseOpacity * 3.5)})`;
			ctx.font = `${Math.max(9, Math.min(13, 10 * camera.zoom))}px system-ui, sans-serif`;
			ctx.textAlign = 'center';
			ctx.textBaseline = 'middle';
			ctx.fillText(genre, sx, sy);
			ctx.restore();
		}
	}
}

// ─── Edges ────────────────────────────────────────────────────────────────────

export function drawEdges(
	ctx: CanvasRenderingContext2D,
	edges: DiscoverEdge[],
	nodeMap: Map<number, DiscoverTrackNode>,
	camera: Camera,
	cw: number,
	ch: number,
	zoom: number,
	lens: DiscoverLens,
	seedTrackId: number | null,
	hoveredTrackId: number | null,
	selectedTrackId: number | null,
	routeTrackIds: Set<number>
): void {
	for (const edge of edges) {
		const from = nodeMap.get(edge.fromTrackId);
		const to   = nodeMap.get(edge.toTrackId);
		if (!from || !to) continue;

		// Visibility filter: skip low-weight edges unless they're meaningful
		const connectsSeed     = edge.fromTrackId === seedTrackId || edge.toTrackId === seedTrackId;
		const connectsHovered  = hoveredTrackId !== null && (edge.fromTrackId === hoveredTrackId  || edge.toTrackId === hoveredTrackId);
		const connectsSelected = selectedTrackId !== null && (edge.fromTrackId === selectedTrackId || edge.toTrackId === selectedTrackId);
		const isRouteEdge      = routeTrackIds.has(edge.fromTrackId) && routeTrackIds.has(edge.toTrackId);

		if (!connectsSeed && !connectsHovered && !connectsSelected && !isRouteEdge && edge.weight <= 0.45) continue;

		const [x1, y1] = worldToCanvas(from.x, from.y, camera, cw, ch);
		const [x2, y2] = worldToCanvas(to.x, to.y, camera, cw, ch);

		const isHighlighted = connectsHovered || connectsSelected;
		// Base alpha: highlighted > seed > normal; also scales with zoom (edges fade at low zoom)
		const baseAlpha = isHighlighted ? 0.65 : connectsSeed ? 0.32 : 0.18;
		const alpha = baseAlpha * (0.35 + edge.confidence * 0.65) * Math.min(1, zoom * 1.4);
		const thickness = 0.4 + edge.weight * (isHighlighted ? 3.0 : 1.8);
		const col = EDGE_COLORS[edge.reason] ?? EDGE_COLORS.unknown!;

		ctx.save();
		ctx.globalAlpha = alpha;
		ctx.strokeStyle = col;
		ctx.lineWidth = thickness;
		ctx.beginPath();

		if (edge.reason === 'harmonic') {
			const mx = (x1 + x2) / 2 + (y2 - y1) * 0.15;
			const my = (y1 + y2) / 2 - (x2 - x1) * 0.15;
			ctx.moveTo(x1, y1);
			ctx.quadraticCurveTo(mx, my, x2, y2);
		} else if (edge.reason === 'bpm') {
			ctx.setLineDash([3 * Math.max(0.5, camera.zoom), 4 * Math.max(0.5, camera.zoom)]);
			ctx.moveTo(x1, y1);
			ctx.lineTo(x2, y2);
		} else {
			ctx.moveTo(x1, y1);
			ctx.lineTo(x2, y2);
		}

		ctx.stroke();
		ctx.setLineDash([]);
		ctx.restore();
	}
}

// ─── Nodes ────────────────────────────────────────────────────────────────────

export function drawNodes(
	ctx: CanvasRenderingContext2D,
	nodes: DiscoverTrackNode[],
	camera: Camera,
	cw: number,
	ch: number,
	lens: DiscoverLens,
	hoveredTrackId: number | null,
	selectedTrackId: number | null,
	connectedIds: Set<number>,
	t: number,
	prefersReducedMotion: boolean
): void {
	for (const node of nodes) {
		if (node.isSeed || node.isPlaying) continue;
		drawSingleNode(ctx, node, camera, cw, ch, lens, hoveredTrackId, selectedTrackId, connectedIds, t, prefersReducedMotion);
	}
}

function drawSingleNode(
	ctx: CanvasRenderingContext2D,
	node: DiscoverTrackNode,
	camera: Camera,
	cw: number,
	ch: number,
	lens: DiscoverLens,
	hoveredTrackId: number | null,
	selectedTrackId: number | null,
	connectedIds: Set<number>,
	t: number,
	prefersReducedMotion: boolean
): void {
	const [sx, sy] = worldToCanvas(node.x, node.y, camera, cw, ch);
	const r = Math.max(3, node.radius * camera.zoom);
	const isHovered  = node.trackId === hoveredTrackId;
	const isSelected = node.trackId === selectedTrackId;
	const col = nodeColor(node, lens);
	// Focus opacity: dim non-connected nodes when something is hovered
	const fo = focusOpacity(node.trackId, hoveredTrackId, connectedIds);
	const coreAlpha = (node.isColdStart ? 0.55 : 0.90) * fo;

	ctx.save();

	// 1. Soft outer star glow (slow breathe at 5.2s period when settled)
	const breathe = prefersReducedMotion ? 1 : 1 + Math.sin(t * 0.02 + node.trackId * 0.1) * 0.025;
	const glowR = r * 2.8 * breathe;
	const glowStrength = (0.09 + node.confidence * 0.16) * fo;
	const glowGrad = ctx.createRadialGradient(sx, sy, r * 0.4, sx, sy, glowR);
	glowGrad.addColorStop(0, `rgba(180,185,255,${glowStrength})`);
	glowGrad.addColorStop(1, 'rgba(0,0,0,0)');
	ctx.fillStyle = glowGrad;
	ctx.beginPath();
	ctx.arc(sx, sy, glowR, 0, Math.PI * 2);
	ctx.fill();

	// 2. Cold-start shimmer ring (very slow pulse)
	if (node.isColdStart && !prefersReducedMotion) {
		const shimmer = (0.10 + Math.sin(t * 0.02 + node.trackId) * 0.05) * fo;
		ctx.strokeStyle = `rgba(180,180,210,${shimmer})`;
		ctx.lineWidth = 1;
		ctx.beginPath();
		ctx.arc(sx, sy, r + Math.min(4, 4 * camera.zoom), 0, Math.PI * 2);
		ctx.stroke();
	}

	// 3. Filled core
	ctx.globalAlpha = coreAlpha;
	ctx.fillStyle = col;
	ctx.beginPath();
	ctx.arc(sx, sy, r, 0, Math.PI * 2);
	ctx.fill();

	// 4. Specular highlight
	const specGrad = ctx.createRadialGradient(sx - r * 0.25, sy - r * 0.3, 0, sx, sy, r);
	specGrad.addColorStop(0, 'rgba(255,255,255,0.70)');
	specGrad.addColorStop(0.45, 'rgba(255,255,255,0.12)');
	specGrad.addColorStop(1, 'rgba(0,0,0,0)');
	ctx.fillStyle = specGrad;
	ctx.beginPath();
	ctx.arc(sx, sy, r, 0, Math.PI * 2);
	ctx.fill();

	// 5. Thin source ring
	ctx.globalAlpha = 0.55 * fo;
	const ringCol = node.source === 'library' ? '#6080e0'
		: node.source === 'lastfm' ? '#e04060'
		: node.source === 'engine' ? '#50c070'
		: '#a080c0';
	ctx.strokeStyle = ringCol;
	ctx.lineWidth = Math.max(0.5, 1.0 * camera.zoom);
	ctx.beginPath();
	ctx.arc(sx, sy, r + Math.max(1, 1.5 * camera.zoom), 0, Math.PI * 2);
	ctx.stroke();

	ctx.globalAlpha = fo;

	// 6. Playlist ring (gold)
	if (node.inPlaylistBuilder) {
		ctx.strokeStyle = 'rgba(255,200,50,0.9)';
		ctx.lineWidth = 1.5;
		ctx.beginPath();
		ctx.arc(sx, sy, r + Math.max(3, 3.5 * camera.zoom), 0, Math.PI * 2);
		ctx.stroke();
	}

	// 7. Hover / selected ring (always full opacity — visual selection feedback)
	if (isHovered || isSelected) {
		ctx.globalAlpha = 1;
		ctx.strokeStyle = isSelected ? 'rgba(255,255,255,0.90)' : 'rgba(255,255,255,0.50)';
		ctx.lineWidth = isSelected ? 2 : 1;
		ctx.beginPath();
		ctx.arc(sx, sy, r + Math.max(3.5, 4 * camera.zoom), 0, Math.PI * 2);
		ctx.stroke();
	}

	ctx.restore();
}

// ─── Seed (Anchor Star) ───────────────────────────────────────────────────────

export function drawSeedNode(
	ctx: CanvasRenderingContext2D,
	node: DiscoverTrackNode,
	camera: Camera,
	cw: number,
	ch: number,
	isLocked: boolean,
	t: number,
	prefersReducedMotion: boolean
): void {
	const [sx, sy] = worldToCanvas(node.x, node.y, camera, cw, ch);
	const r = Math.max(8, node.radius * camera.zoom);

	ctx.save();

	// Pulsing outer halo
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

		// Secondary heartbeat ring
		const ring2R = r * (1.8 + Math.sin(t * 0.04) * 0.4);
		ctx.strokeStyle = `rgba(140,150,255,${0.32 + Math.sin(t * 0.04) * 0.14})`;
		ctx.lineWidth = 1.5;
		ctx.beginPath();
		ctx.arc(sx, sy, ring2R, 0, Math.PI * 2);
		ctx.stroke();
	}

	// Core gradient
	const seedGrad = ctx.createRadialGradient(sx - r * 0.3, sy - r * 0.3, 0, sx, sy, r);
	seedGrad.addColorStop(0, '#d0d8ff');
	seedGrad.addColorStop(0.5, '#7080f0');
	seedGrad.addColorStop(1, '#3848c8');
	ctx.fillStyle = seedGrad;
	ctx.beginPath();
	ctx.arc(sx, sy, r, 0, Math.PI * 2);
	ctx.fill();

	// Specular highlight
	const specGrad = ctx.createRadialGradient(sx - r * 0.3, sy - r * 0.35, 0, sx, sy, r);
	specGrad.addColorStop(0, 'rgba(255,255,255,0.80)');
	specGrad.addColorStop(0.4, 'rgba(255,255,255,0.12)');
	specGrad.addColorStop(1, 'rgba(0,0,0,0)');
	ctx.fillStyle = specGrad;
	ctx.beginPath();
	ctx.arc(sx, sy, r, 0, Math.PI * 2);
	ctx.fill();

	// Lock icon
	if (isLocked) {
		ctx.fillStyle = 'rgba(255,255,255,0.9)';
		ctx.font = `${Math.max(10, r * 0.9)}px system-ui`;
		ctx.textAlign = 'center';
		ctx.textBaseline = 'middle';
		ctx.fillText('🔒', sx, sy);
	}

	ctx.restore();
}

// ─── Playing (Signal Star) ────────────────────────────────────────────────────

export function drawPlayingNode(
	ctx: CanvasRenderingContext2D,
	node: DiscoverTrackNode,
	camera: Camera,
	cw: number,
	ch: number,
	t: number,
	prefersReducedMotion: boolean
): void {
	const [sx, sy] = worldToCanvas(node.x, node.y, camera, cw, ch);
	const r = Math.max(6, node.radius * camera.zoom);

	ctx.save();

	if (!prefersReducedMotion) {
		// Two offset rings at 3200ms period — slow, breathing pulse
		for (let i = 0; i < 2; i++) {
			const phase = t * 0.033 + i * Math.PI;
			const pulseR = r * (1.7 + Math.sin(phase) * 0.5);
			ctx.strokeStyle = `rgba(180,160,255,${0.35 - i * 0.10})`;
			ctx.lineWidth = 1.5;
			ctx.beginPath();
			ctx.arc(sx, sy, pulseR, 0, Math.PI * 2);
			ctx.stroke();
		}
	}

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

	ctx.restore();
}

// ─── Labels ───────────────────────────────────────────────────────────────────

export function drawLabels(
	ctx: CanvasRenderingContext2D,
	nodes: DiscoverTrackNode[],
	camera: Camera,
	cw: number,
	ch: number,
	hoveredTrackId: number | null,
	selectedTrackId: number | null,
	zoom: number
): void {
	const isHighZoom = zoom >= 1.15;
	const isMidZoom  = zoom >= 0.70;
	// Score thresholds gate which non-priority nodes show labels
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

		const [sx, sy] = worldToCanvas(node.x, node.y, camera, cw, ch);
		const r = Math.max(3, node.radius * zoom);
		const raw = node.title;
		const label = raw.length > maxLen ? raw.slice(0, maxLen - 1) + '…' : raw;

		ctx.shadowColor = 'rgba(0,0,0,0.85)';
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
	cw: number,
	ch: number,
	zoom: number,
	t: number,
	prefersReducedMotion: boolean
): void {
	if (route.length < 2) return;

	const positions: Array<[number, number]> = [];
	for (const step of route) {
		const node = nodeMap.get(step.trackId);
		if (node) positions.push(worldToCanvas(node.x, node.y, camera, cw, ch));
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

// ─── Warp streaks (hyperspace animation) ─────────────────────────────────────

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
