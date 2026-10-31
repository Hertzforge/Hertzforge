/**
 * Series in a view each carry their own x. uPlot draws from one shared x, so the kit merges the
 * x values and leaves gaps (null) where a series has no sample; gaps are spanned when drawn.
 * This is layout, not numerics: no value is created or changed.
 */

import type { NumericArray, Series } from '$lib/protocol/types';
import { binValueAt } from './steps';

export type AlignedData = [Float64Array, ...(Float64Array | (number | null)[])[]];

function sameValues(a: NumericArray, b: NumericArray): boolean {
	if (a === b) return true;
	if (a.length !== b.length) return false;
	for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
	return true;
}

/**
 * Align series on the union of their x values. `positiveOnly` drops x or y that cannot sit on a
 * log axis (zero, negative, non-finite).
 */
export function alignSeries(series: Series[], logX: boolean, logY: boolean): AlignedData {
	if (series.length === 0) return [new Float64Array(0)];
	const keepX = (x: number) => Number.isFinite(x) && (!logX || x > 0);
	const keepY = (y: number) => Number.isFinite(y) && (!logY || y > 0);

	// Fast path: every series on the same x (the common case for one measurement).
	const x0 = series[0].x;
	if (series.every((s) => sameValues(s.x, x0)) && isAscending(x0)) {
		const xs: number[] = [];
		const idx: number[] = [];
		for (let i = 0; i < x0.length; i++) if (keepX(x0[i])) (xs.push(x0[i]), idx.push(i));
		const out: AlignedData = [Float64Array.from(xs)];
		for (const s of series) out.push(idx.map((i) => (keepY(s.y[i]) ? s.y[i] : null)));
		return out;
	}

	const all = new Set<number>();
	for (const s of series) for (let i = 0; i < s.x.length; i++) if (keepX(s.x[i])) all.add(s.x[i]);
	const xs = Float64Array.from(all).sort();
	const pos = new Map<number, number>();
	xs.forEach((x, i) => pos.set(x, i));
	const out: AlignedData = [xs];
	for (const s of series) {
		const col: (number | null)[] = new Array(xs.length).fill(null);
		const n = Math.min(s.x.length, s.y.length);
		for (let i = 0; i < n; i++) {
			const j = pos.get(s.x[i]);
			if (j !== undefined && keepY(s.y[i])) col[j] = s.y[i];
		}
		out.push(col);
	}
	return out;
}

function isAscending(x: NumericArray): boolean {
	for (let i = 1; i < x.length; i++) if (!(x[i] > x[i - 1])) return false;
	return true;
}

/**
 * The value a series shows at `x`, read along the straight segment the pane draws between its
 * neighbouring samples (in the pane's own axis space, so log-log specifications read true).
 * Returns null outside the series' x span.
 */
export function valueAt(s: Series, x: number, logX: boolean, logY: boolean): number | null {
	// A binned series reads the bin under the cursor, flat across it, as it is drawn.
	if (s.shape === 'steps' && s.edges) return binValueAt(s.edges, s.y, x);
	const n = Math.min(s.x.length, s.y.length);
	if (n === 0 || !Number.isFinite(x)) return null;
	if (x < s.x[0] || x > s.x[n - 1]) return null;
	let lo = 0;
	let hi = n - 1;
	while (hi - lo > 1) {
		const mid = (lo + hi) >> 1;
		if (s.x[mid] <= x) lo = mid;
		else hi = mid;
	}
	const x0 = s.x[lo];
	const x1 = s.x[hi];
	const y0 = s.y[lo];
	const y1 = s.y[hi];
	if (x1 === x0 || x === x0) return y0;
	if (x === x1) return y1;
	if ((logY && (y0 <= 0 || y1 <= 0)) || (logX && (x0 <= 0 || x <= 0))) return Math.abs(x - x0) < Math.abs(x1 - x) ? y0 : y1;
	const t = logX ? Math.log(x / x0) / Math.log(x1 / x0) : (x - x0) / (x1 - x0);
	return logY ? Math.exp(Math.log(y0) + t * Math.log(y1 / y0)) : y0 + t * (y1 - y0);
}
