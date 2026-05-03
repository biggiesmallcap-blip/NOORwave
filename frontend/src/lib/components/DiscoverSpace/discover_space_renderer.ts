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

const ACCENT = '#7c80ff';

// Energy hue ramp: low energy → cool blue, high energy → hot red-orange
export function energyColor(energy: number): string {
	const e = Math.max(0, Math.min(1, energy));
	const h = 240 - e * 200;  // 240 (blue) → 40 (orange)
	const s = 60 + e * 30;
	const l = 40 + e * 15;
	return `hsl(${h}, ${s}%, ${l}%)`;
}

// Edge colors by reason
const EDGE_COLORS: Record<string, string> = {
	harmonic: 'rgba(180,160,255,0.6)',
	behavioral: 'rgba(100,180,255,0.55)',
	bpm: 'rgba(255,220,80,0.55)',
	artist: 'rgba(255,160,80,0.7)',
	album: 'rgba(255,160,80,0.7)',
	genre: 'rgba(80,220,120,0.45)',
	energy: 'rgba(80,220,120,0.45)',
	external: 'rgba(120,100,220,0.4)',
	unknown: 'rgba(120,120,140,0.25)',
};

// Genre family nebula colors
const NEBULA_COLORS: Record<string, string> = {
	ambient: 'rgba(40,80,180,0.06)',
	house: 'rgba(100,60,200,0.06)',
	techno: 'rgba(100,60,200,0.06)',
	electronic: 'rgba(100,60,200,0.06)',
	'hip-hop': 'rgba(200,140,30,0.06)',
	jazz: 'rgba(180,140,40,0.06)',
	metal: 'rgba(120,20,20,0.06)',
	pop: 'rgba(220,80,160,0.06)',
	experimental: 'rgba(140,180,220,0.05)',
	default: 'rgba(80,80,100,0.05)',
};

function nebulaColor(genre: string): string {
	const g = genre.toLowerCase();
	for (const [key, col] of Object.entries(NEBULA_COLORS)) {
		if (g.includes(key)) return col;
	}
	return NEBULA_COLORS.default;
}

// Lens-aware node core color
function nodeColor(node: DiscoverTrackNode, lens: DiscoverLens): string {
	switch (lens) {
		case 'energy':
			return energyColor(node.energy ?? 0.5);
		case 'reason': {
			const c = EDGE_COLORS[node.primaryReason] ?? EDGE_COLORS.unknown;
			return c.replace(/[\d.]+\)$/, '0.9)');
		}
		case 'confidence':
			return `hsl(220, ${30 + node.confidence * 50}%, ${25 + node.confidence * 30}%)`;
		case 'source':
			return node.source === 'library' ? '#6080e0'
				: node.source === 'lastfm' ? '#e04060'
				: node.source === 'engine' ? '#60c080'
				: '#a080c0';
		case 'genre': {
			const col = nebulaColor(node.genres[0] ?? node.topGenre ?? '');
			return col.replace('rgba', 'rgb').replace(/[\d.]+\)$/, ')').replace('rgb', 'rgba').replace(')', ', 0.9)');
		}
		default:
			return energyColor(node.energy ?? 0.5);
	}
}

// ─── Camera transform helpers ─────────────────────────────────────────────────

function worldToCanvas(
	wx: number, wy: number, camera: Camera, cw: number, ch: number
): [number, number] {
	return [
		cw / 2 + (wx - camera.x) * camera.zoom,
		ch / 2 + (wy - camera.y) * camera.zoom,
	];
}

// ─── Slow background particles ────────────────────────────────────────────────

let particleT = 0;
const PARTICLE_COUNT = 80;
const particles = Array.from({ length: PARTICLE_COUNT }, () => ({
	x: (Math.random() - 0.5) * 4000,
	y: (Math.random() - 0.5) * 4000,
	r: 0.5 + Math.random() * 1.5,
	speed: 0.1 + Math.random() * 0.3,
	angle: Math.random() * Math.PI * 2,
}));

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

	// Slow-drifting star particles (in screen space — decorative)
	particleT += 0.005;
	ctx.save();
	for (const p of particles) {
		if (!prefersReducedMotion) {
			p.x += Math.cos(p.angle) * p.speed;
			p.y += Math.sin(p.angle) * p.speed;
			if (Math.abs(p.x) > 2200) p.x *= -1;
			if (Math.abs(p.y) > 2200) p.y *= -1;
		}
		const sx = (w / 2) + (p.x % w);
		const sy = (h / 2) + (p.y % h);
		ctx.beginPath();
		ctx.arc(sx, sy, p.r, 0, Math.PI * 2);
		ctx.fillStyle = `rgba(180,180,220,${0.1 + Math.sin(particleT + p.speed * 10) * 0.05})`;
		ctx.fill();
	}
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
			ctx.fillStyle = 'rgba(160,160,200,0.4)';
			ctx.font = `${Math.max(9, 11 * camera.zoom)}px system-ui, sans-serif`;
			ctx.textAlign = 'center';
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
	// Group nodes by primary genre
	const groups = new Map<string, { x: number; y: number }[]>();
	for (const node of nodes) {
		const key = node.genres[0] ?? node.topGenre ?? 'unknown';
		const arr = groups.get(key) ?? [];
		arr.push({ x: node.x, y: node.y });
		groups.set(key, arr);
	}

	const opacity = lens === 'genre' ? 1.6 : 1.0;

	for (const [genre, positions] of groups) {
		if (positions.length < 2) continue;
		const cx = positions.reduce((s, p) => s + p.x, 0) / positions.length;
		const cy = positions.reduce((s, p) => s + p.y, 0) / positions.length;
		const maxDist = Math.max(...positions.map((p) => Math.sqrt((p.x - cx) ** 2 + (p.y - cy) ** 2)));
		const r = Math.max(60, maxDist * 1.2);
		const [sx, sy] = worldToCanvas(cx, cy, camera, cw, ch);
		const sr = r * camera.zoom;

		const col = nebulaColor(genre);
		const grad = ctx.createRadialGradient(sx, sy, 0, sx, sy, sr);
		grad.addColorStop(0, col.replace(/[\d.]+\)$/, `${0.12 * opacity})`));
		grad.addColorStop(0.5, col.replace(/[\d.]+\)$/, `${0.06 * opacity})`));
		grad.addColorStop(1, 'rgba(0,0,0,0)');
		ctx.fillStyle = grad;
		ctx.beginPath();
		ctx.arc(sx, sy, sr, 0, Math.PI * 2);
		ctx.fill();
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
	lens: DiscoverLens
): void {
	for (const edge of edges) {
		const from = nodeMap.get(edge.fromTrackId);
		const to = nodeMap.get(edge.toTrackId);
		if (!from || !to) continue;

		const [x1, y1] = worldToCanvas(from.x, from.y, camera, cw, ch);
		const [x2, y2] = worldToCanvas(to.x, to.y, camera, cw, ch);

		const baseColor = EDGE_COLORS[edge.reason] ?? EDGE_COLORS.unknown;
		const alpha = (0.25 + edge.confidence * 0.75) * Math.min(1, zoom / 0.5);
		const thickness = 0.5 + edge.weight * 2.5;

		ctx.save();
		ctx.globalAlpha = alpha;
		ctx.strokeStyle = baseColor;
		ctx.lineWidth = thickness;

		// Curved path for harmonic edges; dotted for bpm; straight for others
		ctx.beginPath();
		if (edge.reason === 'harmonic') {
			const mx = (x1 + x2) / 2 + (y2 - y1) * 0.15;
			const my = (y1 + y2) / 2 - (x2 - x1) * 0.15;
			ctx.moveTo(x1, y1);
			ctx.quadraticCurveTo(mx, my, x2, y2);
		} else if (edge.reason === 'bpm') {
			ctx.setLineDash([4 * camera.zoom, 4 * camera.zoom]);
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
	t: number,
	prefersReducedMotion: boolean
): void {
	for (const node of nodes) {
		if (node.isSeed || node.isPlaying) continue; // drawn separately
		drawSingleNode(ctx, node, camera, cw, ch, lens, hoveredTrackId, selectedTrackId, t, prefersReducedMotion);
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
	t: number,
	prefersReducedMotion: boolean
): void {
	const [sx, sy] = worldToCanvas(node.x, node.y, camera, cw, ch);
	const r = node.radius * camera.zoom;
	const isHovered = node.trackId === hoveredTrackId;
	const isSelected = node.trackId === selectedTrackId;

	ctx.save();

	// Danceability halo (outer ring glow)
	if (node.danceability != null && node.danceability > 0.5) {
		const haloAlpha = (node.danceability - 0.5) * node.confidence * 0.4;
		const haloGrad = ctx.createRadialGradient(sx, sy, r, sx, sy, r * 2.5);
		haloGrad.addColorStop(0, `rgba(124,128,255,${haloAlpha})`);
		haloGrad.addColorStop(1, 'rgba(124,128,255,0)');
		ctx.fillStyle = haloGrad;
		ctx.beginPath();
		ctx.arc(sx, sy, r * 2.5, 0, Math.PI * 2);
		ctx.fill();
	}

	// Cold-start / low-confidence shimmer
	if (node.isColdStart && !prefersReducedMotion) {
		const shimmerAlpha = 0.15 + Math.sin(t * 0.04 + node.trackId) * 0.08;
		ctx.strokeStyle = `rgba(180,180,200,${shimmerAlpha})`;
		ctx.lineWidth = 1;
		ctx.beginPath();
		ctx.arc(sx, sy, r + 4 * camera.zoom, 0, Math.PI * 2);
		ctx.stroke();
	}

	// Source ring
	const ringColor = node.source === 'library' ? '#6080e0'
		: node.source === 'lastfm' ? '#e04060'
		: node.source === 'engine' ? '#60c080'
		: '#a080c0';
	ctx.strokeStyle = ringColor;
	ctx.lineWidth = 1.5;
	ctx.beginPath();
	ctx.arc(sx, sy, r + 2 * camera.zoom, 0, Math.PI * 2);
	ctx.stroke();

	// Core
	ctx.fillStyle = nodeColor(node, lens);
	ctx.globalAlpha = node.isColdStart ? 0.6 : 0.9;
	ctx.beginPath();
	ctx.arc(sx, sy, r, 0, Math.PI * 2);
	ctx.fill();

	// Playlist ring
	if (node.inPlaylistBuilder) {
		ctx.strokeStyle = 'rgba(255,200,50,0.9)';
		ctx.lineWidth = 2;
		ctx.beginPath();
		ctx.arc(sx, sy, r + 4 * camera.zoom, 0, Math.PI * 2);
		ctx.stroke();
	}

	// Hover/selected highlight
	if (isHovered || isSelected) {
		ctx.strokeStyle = isSelected ? '#ffffff' : 'rgba(255,255,255,0.6)';
		ctx.lineWidth = isSelected ? 2 : 1;
		ctx.globalAlpha = 1;
		ctx.beginPath();
		ctx.arc(sx, sy, r + 3 * camera.zoom, 0, Math.PI * 2);
		ctx.stroke();
	}

	ctx.restore();
}

// ─── Seed (Anchor Star) decoration ───────────────────────────────────────────

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
	const r = node.radius * camera.zoom;

	ctx.save();

	// Pulsing halo
	if (!prefersReducedMotion) {
		const pulseR = r * (2.5 + Math.sin(t * 0.03) * 0.5);
		const haloGrad = ctx.createRadialGradient(sx, sy, r, sx, sy, pulseR);
		haloGrad.addColorStop(0, 'rgba(124,128,255,0.3)');
		haloGrad.addColorStop(1, 'rgba(124,128,255,0)');
		ctx.fillStyle = haloGrad;
		ctx.beginPath();
		ctx.arc(sx, sy, pulseR, 0, Math.PI * 2);
		ctx.fill();
	}

	// Core
	const seedGrad = ctx.createRadialGradient(sx - r * 0.3, sy - r * 0.3, 0, sx, sy, r);
	seedGrad.addColorStop(0, '#c0c8ff');
	seedGrad.addColorStop(1, '#5060e0');
	ctx.fillStyle = seedGrad;
	ctx.beginPath();
	ctx.arc(sx, sy, r, 0, Math.PI * 2);
	ctx.fill();

	// Lock icon overlay
	if (isLocked) {
		ctx.fillStyle = 'rgba(255,255,255,0.9)';
		ctx.font = `${Math.max(10, r * 1.1)}px system-ui`;
		ctx.textAlign = 'center';
		ctx.textBaseline = 'middle';
		ctx.fillText('🔒', sx, sy);
	}

	ctx.restore();
}

// ─── Playing (Signal Star) decoration ────────────────────────────────────────

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
	const r = node.radius * camera.zoom;

	ctx.save();

	// Double lavender pulse rings
	if (!prefersReducedMotion) {
		for (let i = 0; i < 2; i++) {
			const phase = (t * 0.04 + i * Math.PI) % (Math.PI * 2);
			const pulseR = r * (1.8 + Math.sin(phase) * 0.8);
			ctx.strokeStyle = `rgba(180,160,255,${0.4 - i * 0.15})`;
			ctx.lineWidth = 1.5;
			ctx.beginPath();
			ctx.arc(sx, sy, pulseR, 0, Math.PI * 2);
			ctx.stroke();
		}
	}

	// Core (bright lavender)
	const playGrad = ctx.createRadialGradient(sx, sy, 0, sx, sy, r);
	playGrad.addColorStop(0, '#e0d8ff');
	playGrad.addColorStop(1, '#9080e0');
	ctx.fillStyle = playGrad;
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
	zoom: number
): void {
	const minFontSize = 9;
	const baseFontSize = Math.max(minFontSize, 11 * zoom);

	ctx.save();
	ctx.font = `${baseFontSize}px system-ui, sans-serif`;
	ctx.textAlign = 'center';
	ctx.textBaseline = 'top';

	for (const node of nodes) {
		const showLabel =
			node.isSeed ||
			node.isPlaying ||
			node.trackId === hoveredTrackId ||
			(zoom >= 1.1 && node.score > 0.75) ||
			node.inPlaylistBuilder;

		if (!showLabel) continue;

		const [sx, sy] = worldToCanvas(node.x, node.y, camera, cw, ch);
		const r = node.radius * zoom;
		const label = node.title.length > 22 ? node.title.slice(0, 20) + '…' : node.title;

		ctx.fillStyle = node.isSeed ? 'rgba(255,255,255,0.95)'
			: node.isPlaying ? 'rgba(220,200,255,0.9)'
			: 'rgba(200,200,220,0.75)';
		ctx.fillText(label, sx, sy + r + 4);

		if (zoom >= 1.1 && node.trackId === hoveredTrackId) {
			ctx.fillStyle = 'rgba(160,160,180,0.6)';
			ctx.font = `${baseFontSize - 1}px system-ui, sans-serif`;
			ctx.fillText(node.artist, sx, sy + r + 4 + baseFontSize + 2);
			ctx.font = `${baseFontSize}px system-ui, sans-serif`;
		}
	}

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

	// Draw curved route path
	ctx.save();
	ctx.strokeStyle = 'rgba(160,140,255,0.5)';
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

	// Step numbers at zoom >= 1.1
	if (zoom >= 1.1) {
		ctx.font = `${Math.max(8, 9 * zoom)}px system-ui`;
		ctx.textAlign = 'center';
		ctx.textBaseline = 'middle';
		for (let i = 0; i < route.length; i++) {
			const step = route[i]!;
			const pos = positions[i];
			if (!pos) continue;
			const isCurrentStep = step.isCurrent;
			ctx.fillStyle = isCurrentStep ? 'rgba(220,200,255,0.9)' : 'rgba(140,120,220,0.6)';
			ctx.fillText(`${i + 1}`, pos[0], pos[1] - 16 * zoom);
		}
	}

	ctx.restore();
}

// ─── Warp streaks (hyperspace animation) ─────────────────────────────────────

export function drawWarpStreaks(
	ctx: CanvasRenderingContext2D,
	progress: number,  // 0..1
	w: number,
	h: number
): void {
	if (progress <= 0) return;
	ctx.save();
	ctx.globalAlpha = progress;
	const streakCount = 40;
	const cx = w / 2;
	const cy = h / 2;
	for (let i = 0; i < streakCount; i++) {
		const angle = (i / streakCount) * Math.PI * 2;
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
