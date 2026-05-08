import { describe, it, expect } from 'vitest';
import { computeWindow } from './VirtualList.svelte';

describe('computeWindow', () => {
    it('returns first window when scrollTop is 0', () => {
        // 1000 items, 40px each, viewport 600px, overscan 5
        const r = computeWindow({ scrollTop: 0, itemHeight: 40, viewportH: 600, total: 1000, overscan: 5 });
        expect(r.start).toBe(0);
        expect(r.end).toBe(20); // ceil(600/40)=15 + 5 overscan
        expect(r.padTop).toBe(0);
        expect(r.padBottom).toBe((1000 - 20) * 40);
    });

    it('windows correctly mid-scroll', () => {
        const r = computeWindow({ scrollTop: 4000, itemHeight: 40, viewportH: 600, total: 1000, overscan: 5 });
        // first visible = floor(4000/40) = 100
        expect(r.start).toBe(95); // 100 - 5 overscan
        expect(r.end).toBe(120); // 100 + 15 visible + 5 overscan
        expect(r.padTop).toBe(95 * 40);
    });

    it('clamps end to total', () => {
        const r = computeWindow({ scrollTop: 100_000, itemHeight: 40, viewportH: 600, total: 1000, overscan: 5 });
        expect(r.end).toBe(1000);
        expect(r.padBottom).toBe(0);
    });

    it('clamps start to 0', () => {
        const r = computeWindow({ scrollTop: -50, itemHeight: 40, viewportH: 600, total: 1000, overscan: 5 });
        expect(r.start).toBe(0);
    });

    it('returns empty window when itemHeight is 0', () => {
        const r = computeWindow({ scrollTop: 0, itemHeight: 0, viewportH: 600, total: 1000, overscan: 5 });
        expect(r).toEqual({ start: 0, end: 0, padTop: 0, padBottom: 0 });
    });

    it('returns empty window when total is 0', () => {
        const r = computeWindow({ scrollTop: 0, itemHeight: 40, viewportH: 600, total: 0, overscan: 5 });
        expect(r).toEqual({ start: 0, end: 0, padTop: 0, padBottom: 0 });
    });

    it('handles total smaller than visible window', () => {
        const r = computeWindow({ scrollTop: 0, itemHeight: 40, viewportH: 600, total: 3, overscan: 5 });
        expect(r.start).toBe(0);
        expect(r.end).toBe(3);
        expect(r.padTop).toBe(0);
        expect(r.padBottom).toBe(0);
    });
});
