export type LabelInput = {
	depth: number;
	/** camera.scale. Nodes draw at a fixed screen size, so zoom is what spaces them out. */
	zoom: number;
	selected: boolean;
	inLineage: boolean;
	labelsEnabled: boolean;
	compact: boolean;
};

/** Zoom range over which sub-genre names fade in, per depth. */
const ZOOM_FADE: Record<number, [number, number]> = { 1: [0.8, 1.1], 2: [1.7, 2.2] };
const DEEP_ZOOM_FADE: [number, number] = [2.8, 3.4];

/** One rule for label visibility: families always, sub-genres by zoom. */
export function labelAlpha(input: LabelInput): number {
	if (input.selected) return 1;
	if (input.depth === 0) return 0.92;
	if (input.compact) return input.inLineage && input.depth === 1 ? 0.8 : 0;
	if (input.inLineage) return 0.85;
	if (!input.labelsEnabled) return 0;
	const [start, end] = ZOOM_FADE[input.depth] ?? DEEP_ZOOM_FADE;
	const t = Math.max(0, Math.min(1, (input.zoom - start) / (end - start)));
	return t * (input.depth === 1 ? 0.88 : 0.72);
}

/** Who wins a collision: selection, then families, then the selected lineage, then shallow and hot. */
export function labelPriority(depth: number, selected: boolean, inLineage: boolean, heatNorm: number): number {
	if (selected) return 1000;
	if (depth === 0) return 500 + heatNorm * 10;
	if (inLineage) return 300 + heatNorm * 10;
	return 200 - depth * 50 + heatNorm * 10;
}

export type LabelRect = { id: number; x: number; y: number; width: number; height: number; priority: number };

/**
 * Greedy placement, highest priority first. A label is dropped when it would
 * overlap an accepted one (with `gap` px clearance) or is not fully on screen.
 */
export function placeLabels(
	candidates: LabelRect[],
	viewport: { width: number; height: number },
	gap = 4
): Set<number> {
	const accepted: LabelRect[] = [];
	const ordered = [...candidates].sort((a, b) => b.priority - a.priority || a.id - b.id);
	for (const label of ordered) {
		const onScreen =
			label.x >= 0 &&
			label.y >= 0 &&
			label.x + label.width <= viewport.width &&
			label.y + label.height <= viewport.height;
		if (!onScreen) continue;
		const collides = accepted.some(
			(other) =>
				label.x < other.x + other.width + gap &&
				other.x < label.x + label.width + gap &&
				label.y < other.y + other.height + gap &&
				other.y < label.y + label.height + gap
		);
		if (!collides) accepted.push(label);
	}
	return new Set(accepted.map((label) => label.id));
}
