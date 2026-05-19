import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const kpiStrip = readFileSync('src/lib/components/analytics/KpiStrip.svelte', 'utf8');
const kpiCell = readFileSync('src/lib/components/analytics/KpiCell.svelte', 'utf8');
const rankList = readFileSync('src/lib/components/analytics/RankList.svelte', 'utf8');
const listenRidgeline = readFileSync('src/lib/components/charts/ListenRidgeline.svelte', 'utf8');
const analyticsPage = readFileSync('src/routes/analytics/+page.svelte', 'utf8');
const cohortPreview = readFileSync('src/routes/analytics/preview/cohorts/+page.svelte', 'utf8');

describe('analytics dashboard contract', () => {
	it('uses daily session counts for the Sessions sparkline and inverse polarity for skip rate', () => {
		expect(kpiStrip).toContain('const sessionsSeries');
		expect(kpiStrip).toContain('kpis.daily.map((d) => d.sessions');
		expect(kpiStrip).toContain('series={sessionsSeries}');
		expect(kpiStrip).toContain('polarity="inverse"');
		expect(kpiCell).toContain("polarity?: 'positive' | 'inverse' | 'neutral'");
		expect(kpiCell).toContain('data-tone={deltaTone}');
		expect(kpiStrip).not.toContain('series={listensSeries}');
	});

	it('renders genre percentages from the server window denominator', () => {
		expect(rankList).toContain('share_of_window_listens');
		expect(rankList).not.toContain('genreTotal');
		expect(rankList).not.toContain('reduce((s, g) => s + g.listens');
	});

	it('uses selected-window cohort copy in the frontend preview surface', () => {
		expect(cohortPreview).toContain('New in selected window');
		expect(cohortPreview).not.toContain('New this month');
	});

	it('keeps Listening Pulse chart-first with a stat rail, volume cues, and visible controls', () => {
		expect(listenRidgeline).not.toContain('<aside class="spine">');
		expect(listenRidgeline).toContain('class="stat-rail"');
		expect(listenRidgeline).toContain('class="volume-tick"');
		expect(listenRidgeline).toContain('stroke-opacity={rowStrokeOpacity(i)}');
		expect(listenRidgeline).toContain('Ridge shape is normalized per row; side ticks show volume.');
		expect(listenRidgeline).toContain('class="chart-footer"');
		expect(listenRidgeline).toContain('class="sigma-row"');

		const chartIndex = listenRidgeline.indexOf('<svg');
		const statRailIndex = listenRidgeline.indexOf('class="stat-rail"');
		const captionIndex = listenRidgeline.indexOf('class="caption"');
		const controlIndex = listenRidgeline.indexOf('class="sigma-row"');
		expect(chartIndex).toBeGreaterThan(-1);
		expect(statRailIndex).toBeGreaterThan(chartIndex);
		expect(captionIndex).toBeGreaterThan(statRailIndex);
		expect(controlIndex).toBeGreaterThan(captionIndex);
	});

	it('labels capped long-window ridgelines from response metadata', () => {
		expect(analyticsPage).toContain('signals.window.display_caps.ridgeline_days');
		expect(analyticsPage).toContain('windowLabel={ridgelineWindowLabel}');
		expect(listenRidgeline).toContain('windowLabel?: string | null');
		expect(listenRidgeline).toContain('{windowLabel}');
	});
});
