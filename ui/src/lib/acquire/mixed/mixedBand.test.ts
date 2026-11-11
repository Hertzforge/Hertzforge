import { describe, expect, it } from 'vitest';
import type { MixedConfig } from '$lib/protocol/mixed';
import type { SpectrumView } from '$lib/protocol/types';
import { floorBand, toBand } from './mixedBand';
import { MixedViewsState } from './mixedViews.svelte';

/** A frame's Control view as the service sends it: every line of a 0 to 4096 Hz grid at 1 Hz. */
function controlView(): SpectrumView {
	const n = 4096;
	const x = new Float64Array(n);
	const edges = new Float64Array(n + 1);
	const y = new Float64Array(n);
	const ref = new Float64Array(n);
	for (let k = 0; k < n; k++) {
		x[k] = k;
		edges[k] = k - 0.5;
		const inBand = k >= 20 && k <= 2000;
		y[k] = inBand ? 0.003 : k < 20 ? 1e-10 : 1e-11;
		ref[k] = inBand ? 0.0025 : 0;
	}
	edges[n] = n - 0.5;
	return {
		view: 'spectrum',
		title: 'Control',
		x: { label: 'Frequency', unit: 'Hz', scale: 'log' } as SpectrumView['x'],
		y: { label: 'PSD', unit: 'g²/Hz', scale: 'log' } as SpectrumView['y'],
		series: [
			{ name: 'Control', role: 'measured', x, y, shape: 'steps', edges },
			{ name: 'Reference', role: 'reference', x, y: ref, shape: 'steps', edges }
		],
		regions: [{ x0: 2, x1: 30, kind: 'cut', label: 'Left out' } as never]
	};
}

const extent = (v: SpectrumView) => {
	let lo = Infinity;
	let hi = -Infinity;
	for (const s of v.series) for (const y of s.y) if (y > 0) [lo, hi] = [Math.min(lo, y), Math.max(hi, y)];
	return [lo, hi];
};

const config = { parts: [{ id: 'floor', kind: 'floor', breakpoints: [{ freq_hz: 20, psd_g2_hz: 1e-4 }, { freq_hz: 2000, slope_db_oct: 0 }] }] } as unknown as MixedConfig;

describe('Mixed spectra held to the band', () => {
	it("takes the band from the floor's first and last breakpoints", () => {
		expect(floorBand(config)).toEqual([20, 2000]);
		expect(floorBand(null)).toBeNull();
	});

	it('cuts the out-of-band lines, still stepped on the frame edges', () => {
		const [v] = toBand([controlView()], [20, 2000]) as SpectrumView[];
		const c = v.series[0];
		expect(c.x[0]).toBe(20);
		expect(c.x[c.x.length - 1]).toBe(2000);
		expect(c.edges!.length).toBe(c.y.length + 1);
		expect(c.edges![0]).toBe(19.5);
		expect(c.edges![c.edges!.length - 1]).toBe(2000.5);
		// the data the plot ranges on is the band's: within a decade of the reference
		const [lo, hi] = extent(v);
		expect(hi / lo).toBeLessThan(10);
		expect(v.regions).toEqual([{ x0: 20, x1: 30, kind: 'cut', label: 'Left out' }]);
	});

	it("is what the run's views hand a plot, frame by frame", () => {
		const control = { send: () => Promise.resolve(), on: () => () => {} } as never;
		const views = new MixedViewsState(control, { setupTitle: () => null, showing: () => false, storage: null, session: null, events: null, runId: () => 'r1', band: () => floorBand(config) });
		(views as unknown as { data: unknown }).data = { run_id: 'r1', plots: [{ id: 'p1', views: [controlView()] }] };
		const [v] = views.views('p1') as SpectrumView[];
		const [lo, hi] = extent(v);
		expect(lo).toBeGreaterThan(1e-4);
		expect(hi).toBeLessThan(0.01);
	});
});
