/**
 * The run's spectra held to the test's band. The control service now sends Mixed Mode's spectra cut
 * to the floor's band, and a cut frame passes through here untouched (the same series, nothing
 * copied). A frame that carries lines outside the band (the plant's own response, orders of
 * magnitude under it, which would squash the band into a corner of the plot) is cut to the band,
 * still stepped on the frame's own bin edges.
 * The operator's axis options act on what is left, as on Random's.
 */

import type { MixedConfig } from '$lib/protocol/mixed';
import type { NumericArray, Series, View } from '$lib/protocol/types';

/** The floor's band, from its first breakpoint to its last; null without a floor. */
export function floorBand(config: MixedConfig | null | undefined): [number, number] | null {
	const floor = config?.parts.find((p) => p.kind === 'floor');
	if (!floor || floor.kind !== 'floor' || !floor.breakpoints.length) return null;
	const lo = floor.breakpoints[0].freq_hz;
	const hi = floor.breakpoints[floor.breakpoints.length - 1].freq_hz;
	return Number.isFinite(lo) && Number.isFinite(hi) && hi > lo ? [lo, hi] : null;
}

const slice = (a: NumericArray, i0: number, i1: number): NumericArray => (a as Float64Array).slice(i0, i1);

function cut(s: Series, lo: number, hi: number): Series {
	const n = s.y.length;
	let i0 = 0;
	let i1 = n;
	if (s.edges && s.edges.length === n + 1) {
		// a line is kept when its bin overlaps the band
		while (i0 < n && s.edges[i0 + 1] <= lo) i0++;
		while (i1 > i0 && s.edges[i1 - 1] >= hi) i1--;
	} else {
		while (i0 < n && s.x[i0] < lo) i0++;
		while (i1 > i0 && s.x[i1 - 1] > hi) i1--;
	}
	if (i0 === 0 && i1 === n) return s;
	const out: Series = { ...s, x: slice(s.x, i0, i1), y: slice(s.y, i0, i1) };
	if (s.edges && s.edges.length === n + 1) out.edges = slice(s.edges, i0, i1 + 1);
	if (s.y_min) out.y_min = slice(s.y_min, i0, i1);
	return out;
}

/** Each spectrum on a frequency axis cut to the band's lines; every other view as it came. */
export function toBand(views: View[], band: [number, number] | null): View[] {
	if (!band) return views;
	const [lo, hi] = band;
	return views.map((v) => {
		if (v.view !== 'spectrum' || v.x.unit !== 'Hz') return v;
		return {
			...v,
			series: v.series.map((s) => cut(s, lo, hi)),
			regions: v.regions?.filter((r) => r.x1 > lo && r.x0 < hi).map((r) => ({ ...r, x0: Math.max(r.x0, lo), x1: Math.min(r.x1, hi) }))
		};
	});
}
