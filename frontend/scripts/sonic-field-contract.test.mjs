import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const source = readFileSync('src/lib/components/charts/SonicField.svelte', 'utf8');

describe('SonicField interaction contract', () => {
	it('does not expose chart zoom or pan controls', () => {
		expect(source).not.toContain('onwheel={handleWheel}');
		expect(source).not.toContain('onmousedown={handleMouseDown}');
		expect(source).not.toContain('onmousemove={handleMouseMove}');
		expect(source).not.toContain('onmouseup={handleMouseUp}');
		expect(source).not.toContain('class:zoomed');
		expect(source).not.toContain('class:dragging');
		expect(source).not.toContain('dataTransform');
		expect(source).not.toContain('resetZoom');
		expect(source).not.toContain('dragDist');
		expect(source).toContain('<g class="data-layer">');
	});

	it('keeps track dots accessible without requiring zoom state', () => {
		expect(source).toContain('function handleDotKeydown');
		expect(source).toContain("if (event.key !== 'Enter' && event.key !== ' ') return;");
		expect(source).toContain('tabindex="0"');
		expect(source).toContain('onkeydown={(event) => handleDotKeydown(event, track)}');
	});
});
