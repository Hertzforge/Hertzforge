/**
 * The error heatmap's arithmetic, apart from the canvas: one pixel of color per cell, each row's
 * summary, the abort runs the view hatches, the row order, and which cell is under a point. The
 * values are the service's (decimated to the plot's width by the nearest line); nothing here
 * changes a number, it only places and colors it.
 */

import { bandClass, DEFAULT_BAND, errorRgb, sequentialRgb, type RowBand, type ThemeKind } from './scales';

export type HeatMode = 'error' | 'coherence';

export interface HeatData {
	rows: number;
	cols: number;
	/** Row-major, `rows * cols`. */
	values: ArrayLike<number>;
	/** One per row; missing rows use the default band. */
	bands: RowBand[];
}

/**
 * Write one RGBA pixel per cell into `out` (`rows * cols * 4`), rows in `order` (display row k
 * shows data row `order[k]`). A cell with no value is left transparent.
 */
export function rasterize(d: HeatData, mode: HeatMode, theme: ThemeKind, order: ArrayLike<number>, out: Uint8ClampedArray): void {
	const { cols, values } = d;
	for (let k = 0; k < order.length; k++) {
		const r = order[k];
		const band = d.bands[r] ?? DEFAULT_BAND;
		const base = r * cols;
		let p = k * cols * 4;
		for (let c = 0; c < cols; c++, p += 4) {
			const v = values[base + c];
			const ok = mode === 'error' ? errorRgb(v, band, theme, out, p) : sequentialRgb(v, theme, out, p);
			out[p + 3] = ok ? 255 : 0;
		}
	}
}

export interface RowSummary {
	/** RMS of the row's dB errors (or coherence errors) over its finite cells. */
	rms: number | null;
	/** The largest magnitude and its sign. */
	worst: number | null;
	/** Cells beyond the alarm band (including those beyond abort), and beyond the abort band. */
	alarm: number;
	abort: number;
}

/** One row's numbers for the right-hand column. */
export function rowSummary(d: HeatData, r: number, mode: HeatMode): RowSummary {
	const band = d.bands[r] ?? DEFAULT_BAND;
	let sum = 0;
	let n = 0;
	let worst: number | null = null;
	let alarm = 0;
	let abort = 0;
	const base = r * d.cols;
	for (let c = 0; c < d.cols; c++) {
		const v = d.values[base + c];
		if (!Number.isFinite(v)) continue;
		sum += v * v;
		n++;
		if (worst === null || Math.abs(v) > Math.abs(worst)) worst = v;
		if (mode === 'error') {
			const k = bandClass(v, band);
			if (k >= 1) alarm++;
			if (k === 2) abort++;
		}
	}
	return { rms: n ? Math.sqrt(sum / n) : null, worst, alarm, abort };
}

/** Every row's summary. */
export function summaries(d: HeatData, mode: HeatMode): RowSummary[] {
	return Array.from({ length: d.rows }, (_, r) => rowSummary(d, r, mode));
}

/** Runs of columns `[start, end)` in row `r` beyond the abort band: what the view hatches. */
export function abortRuns(d: HeatData, r: number): [number, number][] {
	const band = d.bands[r] ?? DEFAULT_BAND;
	const runs: [number, number][] = [];
	let start = -1;
	const base = r * d.cols;
	for (let c = 0; c <= d.cols; c++) {
		const v = c < d.cols ? d.values[base + c] : NaN;
		const beyond = Number.isFinite(v) && bandClass(v, band) === 2;
		if (beyond && start < 0) start = c;
		else if (!beyond && start >= 0) {
			runs.push([start, c]);
			start = -1;
		}
	}
	return runs;
}

/**
 * The display order of the rows: as the setup lists them, or the largest rms error first (ties
 * keep the setup's order).
 */
export function rowOrder(rows: number, order: 'setup' | 'worst', sums?: RowSummary[]): Int32Array {
	const idx = Int32Array.from({ length: rows }, (_, i) => i);
	if (order === 'worst' && sums) {
		const key = (i: number) => sums[i]?.rms ?? -1;
		idx.sort((a, b) => key(b) - key(a) || a - b);
	}
	return idx;
}

export interface HeatLayout {
	/** The cell area on the canvas, in CSS pixels. */
	x: number;
	y: number;
	w: number;
	h: number;
	rows: number;
	cols: number;
}

/** The display row and column under a point, or null outside the cells. */
export function cellAt(px: number, py: number, l: HeatLayout): { row: number; col: number } | null {
	if (px < l.x || py < l.y || px >= l.x + l.w || py >= l.y + l.h || l.rows <= 0 || l.cols <= 0) return null;
	return {
		row: Math.min(l.rows - 1, Math.floor(((py - l.y) / l.h) * l.rows)),
		col: Math.min(l.cols - 1, Math.floor(((px - l.x) / l.w) * l.cols))
	};
}

/**
 * Where frequency `f` sits across the columns (0 at the first column's center, `cols - 1` at the
 * last), for columns spaced evenly on a log axis as the service picks them; null outside.
 */
export function columnOf(f: number, x: ArrayLike<number>): number | null {
	const n = x.length;
	if (n < 2 || !(f > 0) || !(x[0] > 0)) return n === 1 ? 0 : null;
	const lo = Math.log(x[0]);
	const hi = Math.log(x[n - 1]);
	if (!(hi > lo)) return null;
	const c = ((Math.log(f) - lo) / (hi - lo)) * (n - 1);
	return c < -0.5 || c > n - 0.5 ? null : c;
}

/** How many rows apart the labels go so that they never overlap at `rowPx` per row. */
export function labelStride(rowPx: number, textPx = 12): number {
	return rowPx >= textPx ? 1 : Math.ceil(textPx / Math.max(0.5, rowPx));
}
