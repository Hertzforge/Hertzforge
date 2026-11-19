/**
 * The matrix grid's arithmetic, apart from the canvas: which controls the window shows (pinned
 * by name, the rest scrolled), what each cell draws (the diagonal a control's spectrum against
 * its reference and bands, above it coherence, below it phase, or the cross-spectrum magnitude
 * on both sides), the cells' places on the canvas, and the polylines in pixels.
 *
 * Cell data come from two subscriptions the window holds: one block of the spectral matrix for
 * the window's controls (measured and reference, packed) and each window control's own spectrum
 * (stepped, with its alarm and abort lines). Coherence, phase and magnitude are read from the
 * block element by element; no number is smoothed, fitted or rescaled.
 */

import { packedElement } from '$lib/protocol/mimo';
import type { NumericArray, Role, SdmView, SpectrumView } from '$lib/protocol/types';
import { binValueAt } from './steps';

/** The largest window the grid shows: 8 by 8. */
export const GRID_MAX = 8;

// ---- the window ---------------------------------------------------------------------------

export interface GridWindowSpec {
	/** Controls kept in the window by name, in the order they were pinned. */
	pinned: string[];
	/** How far the unpinned part of the window has scrolled, in controls. */
	offset: number;
	/** How many controls the window shows, 1 to 8. */
	size: number;
}

export const defaultWindow = (): GridWindowSpec => ({ pinned: [], offset: 0, size: GRID_MAX });

function clampSize(size: number): number {
	return Math.min(GRID_MAX, Math.max(1, Math.round(Number.isFinite(size) ? size : GRID_MAX)));
}

/** The pinned controls' positions that exist in this run, each once, at most the window's size. */
function pinnedPositions(labels: string[], w: GridWindowSpec): number[] {
	const out: number[] = [];
	for (const name of w.pinned) {
		const i = labels.indexOf(name);
		if (i >= 0 && !out.includes(i)) out.push(i);
	}
	return out.slice(0, clampSize(w.size));
}

/** The positions of the controls not pinned, in the setup's order. */
function freePositions(labels: string[], pinned: number[]): number[] {
	return labels.map((_, i) => i).filter((i) => !pinned.includes(i));
}

/** How far the unpinned part can scroll. */
export function maxOffset(labels: string[], w: GridWindowSpec): number {
	const pinned = pinnedPositions(labels, w);
	const room = Math.min(clampSize(w.size), labels.length) - pinned.length;
	return Math.max(0, freePositions(labels, pinned).length - Math.max(0, room));
}

/**
 * The controls the window shows, by position: the pinned ones first in the order they were
 * pinned, then the unpinned ones from the scroll offset, in the setup's order.
 */
export function windowControls(labels: string[], w: GridWindowSpec): number[] {
	const pinned = pinnedPositions(labels, w);
	const room = Math.min(clampSize(w.size), labels.length) - pinned.length;
	const free = freePositions(labels, pinned);
	const off = Math.min(Math.max(0, Math.round(w.offset) || 0), maxOffset(labels, w));
	return [...pinned, ...free.slice(off, off + Math.max(0, room))];
}

/** The window scrolled by `by` controls, kept inside the run's controls. */
export function scrollWindow(labels: string[], w: GridWindowSpec, by: number): GridWindowSpec {
	return { ...w, offset: Math.min(maxOffset(labels, w), Math.max(0, w.offset + by)) };
}

/** Pin a control by name, or unpin it. A full window of pins refuses another (returned unchanged). */
export function togglePin(w: GridWindowSpec, name: string): GridWindowSpec {
	if (w.pinned.includes(name)) return { ...w, pinned: w.pinned.filter((p) => p !== name) };
	if (w.pinned.length >= clampSize(w.size)) return w;
	return { ...w, pinned: [...w.pinned, name] };
}

/** The window moved so that control `pos` is in it (unchanged when it already is). */
export function revealControl(labels: string[], w: GridWindowSpec, pos: number): GridWindowSpec {
	if (pos < 0 || pos >= labels.length || windowControls(labels, w).includes(pos)) return w;
	const free = freePositions(labels, pinnedPositions(labels, w));
	const k = free.indexOf(pos);
	return k < 0 ? w : { ...w, offset: Math.min(k, maxOffset(labels, w)) };
}

// ---- what each cell draws -----------------------------------------------------------------

export type OffDiagonal = 'coh_phase' | 'csd';
export type CellKind = 'psd' | 'coh' | 'phase' | 'csd';

/** Row r, column c of the window: the diagonal is a spectrum; above it coherence, below it phase. */
export function cellKind(r: number, c: number, off: OffDiagonal): CellKind {
	if (r === c) return 'psd';
	if (off === 'csd') return 'csd';
	return r < c ? 'coh' : 'phase';
}

export interface CellCurve {
	role: Role;
	name: string;
	x: NumericArray;
	y: NumericArray;
	/** Bin edges for a stepped density, one more than y. */
	edges?: NumericArray;
}

export interface CellData {
	r: number;
	c: number;
	kind: CellKind;
	curves: CellCurve[];
}

/** Bin edges for values on `x`, halfway between neighbours (geometrically on a log axis). */
export function edgesFromCenters(x: NumericArray, log: boolean): Float64Array {
	const n = x.length;
	const e = new Float64Array(n + 1);
	if (n === 0) return e;
	if (n === 1) {
		e[0] = log ? x[0] / 1.05 : x[0] - 0.5;
		e[1] = log ? x[0] * 1.05 : x[0] + 0.5;
		return e;
	}
	const mid = (a: number, b: number) => (log && a > 0 && b > 0 ? Math.sqrt(a * b) : (a + b) / 2);
	for (let i = 1; i < n; i++) e[i] = mid(x[i - 1], x[i]);
	e[0] = log && x[0] > 0 && e[1] > 0 ? (x[0] * x[0]) / e[1] : 2 * x[0] - e[1];
	e[n] = log && x[n - 1] > 0 && e[n - 1] > 0 ? (x[n - 1] * x[n - 1]) / e[n - 1] : 2 * x[n - 1] - e[n - 1];
	return e;
}

type Blockish = { re: NumericArray; im: NumericArray } | null;

/** One element's quantity for a cell, line by line, from a packed block. */
function elementCurve(b: Blockish, n: number, lines: number, r: number, c: number, kind: CellKind): Float64Array | null {
	if (!b) return null;
	const per = (n * (n + 1)) / 2;
	if (b.re.length < lines * per) return null;
	const out = new Float64Array(lines);
	for (let l = 0; l < lines; l++) {
		const [re, im] = packedElement(b.re, b.im, n, l, r, c);
		if (kind === 'psd') out[l] = re;
		else if (kind === 'csd') out[l] = Math.hypot(re, im);
		else if (kind === 'phase') out[l] = (Math.atan2(im, re) * 180) / Math.PI;
		else {
			const grr = packedElement(b.re, b.im, n, l, r, r)[0];
			const gcc = packedElement(b.re, b.im, n, l, c, c)[0];
			out[l] = grr > 0 && gcc > 0 ? (re * re + im * im) / (grr * gcc) : NaN;
		}
	}
	return out;
}

/**
 * What every cell of an `n` by `n` window draws. `sdm` is the window's block (its channels in the
 * window's order); `spectra[k]` is window control k's own spectrum view, or null until it comes.
 * A diagonal cell prefers the control's own spectrum (stepped, with its bands); without it, the
 * block's diagonal is drawn.
 */
export function gridCells(n: number, off: OffDiagonal, sdm: SdmView | null, spectra: (SpectrumView | null)[]): CellData[] {
	const cells: CellData[] = [];
	const lines = sdm ? sdm.x_values.length : 0;
	const nb = sdm ? sdm.channels.length : 0;
	const log = sdm ? sdm.x.scale === 'log' : true;
	const edges = sdm && lines ? edgesFromCenters(sdm.x_values, log) : undefined;
	for (let r = 0; r < n; r++) {
		for (let c = 0; c < n; c++) {
			const kind = cellKind(r, c, off);
			const curves: CellCurve[] = [];
			const own = kind === 'psd' ? spectra[r] : null;
			if (own) {
				for (const s of own.series) curves.push({ role: s.role, name: s.name, x: s.x, y: s.y, edges: s.edges });
			} else if (sdm && r < nb && c < nb) {
				const stepped = kind === 'psd' || kind === 'csd';
				const m = elementCurve(sdm.measured, nb, lines, r, c, kind);
				const ref = elementCurve(sdm.reference, nb, lines, r, c, kind);
				if (ref) curves.push({ role: 'reference', name: 'Reference', x: sdm.x_values, y: ref, edges: stepped ? edges : undefined });
				if (m) curves.push({ role: 'measured', name: 'Measured', x: sdm.x_values, y: m, edges: stepped ? edges : undefined });
			}
			cells.push({ r, c, kind, curves });
		}
	}
	return cells;
}

// ---- axes ---------------------------------------------------------------------------------

export interface Range {
	lo: number;
	hi: number;
	log: boolean;
}

/** The fixed vertical range of a coherence or phase cell. */
export function fixedRange(kind: CellKind): Range | null {
	if (kind === 'coh') return { lo: 0, hi: 1.05, log: false };
	if (kind === 'phase') return { lo: -180, hi: 180, log: false };
	return null;
}

/**
 * A log range over every positive value of the given curves, out to whole tenths of a decade
 * with a little room above and below; one range for every cell of a kind, so cells compare.
 */
export function logRangeOf(curves: CellCurve[]): Range {
	let lo = Infinity;
	let hi = -Infinity;
	for (const cv of curves) {
		for (let i = 0; i < cv.y.length; i++) {
			const v = cv.y[i];
			if (v > 0 && Number.isFinite(v)) {
				if (v < lo) lo = v;
				if (v > hi) hi = v;
			}
		}
	}
	if (!(hi >= lo)) return { lo: 1e-6, hi: 1, log: true };
	const a = Math.floor(Math.log10(lo) * 10 - 1) / 10;
	const b = Math.ceil(Math.log10(hi) * 10 + 1) / 10;
	return { lo: 10 ** a, hi: 10 ** Math.max(b, a + 0.5), log: true };
}

/** The frequency range every cell shares, from the data. */
export function frequencyRange(cells: CellData[]): Range {
	let lo = Infinity;
	let hi = -Infinity;
	for (const cell of cells) {
		for (const cv of cell.curves) {
			const xs = cv.edges ?? cv.x;
			for (let i = 0; i < xs.length; i++) {
				const v = xs[i];
				if (v > 0 && Number.isFinite(v)) {
					if (v < lo) lo = v;
					if (v > hi) hi = v;
				}
			}
		}
	}
	if (!(hi > lo)) return { lo: 10, hi: 2000, log: true };
	return { lo, hi, log: true };
}

/** 0 to 1 across a range. */
export function frac(v: number, r: Range): number {
	if (r.log) return (Math.log(v) - Math.log(r.lo)) / (Math.log(r.hi) - Math.log(r.lo));
	return (v - r.lo) / (r.hi - r.lo);
}

/** The value at 0 to 1 across a range. */
export function unfrac(t: number, r: Range): number {
	if (r.log) return Math.exp(Math.log(r.lo) + t * (Math.log(r.hi) - Math.log(r.lo)));
	return r.lo + t * (r.hi - r.lo);
}

// ---- layout -------------------------------------------------------------------------------

export interface GridGeometry {
	n: number;
	/** The top-left of cell (0, 0), CSS pixels. */
	x0: number;
	y0: number;
	/** A cell's width and height, and the gap between cells. */
	cell: number;
	cellH: number;
	gap: number;
}

export interface Rect {
	x: number;
	y: number;
	w: number;
	h: number;
}

/**
 * Cells filling `width` by `height` after the gutters for the row names (left), the
 * column names (top) and the frequency axis (bottom).
 */
export function gridGeometry(width: number, height: number, n: number, gutter: { left: number; top: number; bottom: number }): GridGeometry {
	const gap = n > 6 ? 4 : 6;
	const availW = Math.max(0, width - gutter.left - 4);
	const availH = Math.max(0, height - gutter.top - gutter.bottom);
	let w = n > 0 ? Math.floor((availW + gap) / n) - gap : 0;
	let h = n > 0 ? Math.floor((availH + gap) / n) - gap : 0;
	// Cells fill the space, but never so stretched that a curve's shape is lost.
	w = Math.min(w, Math.floor(h * 1.8));
	h = Math.min(h, Math.floor(w * 1.2));
	return { n, x0: gutter.left, y0: gutter.top, cell: Math.max(0, w), cellH: Math.max(0, h), gap };
}

export function cellRect(g: GridGeometry, r: number, c: number): Rect {
	return { x: g.x0 + c * (g.cell + g.gap), y: g.y0 + r * (g.cellH + g.gap), w: g.cell, h: g.cellH };
}

/** The cell under a point and how far across it the point is (0 to 1), or null in a gap or outside. */
export function cellAtPoint(g: GridGeometry, px: number, py: number): { r: number; c: number; fx: number } | null {
	const step = g.cell + g.gap;
	const stepH = g.cellH + g.gap;
	if (step <= 0 || stepH <= 0) return null;
	const c = Math.floor((px - g.x0) / step);
	const r = Math.floor((py - g.y0) / stepH);
	if (r < 0 || c < 0 || r >= g.n || c >= g.n) return null;
	const inX = px - g.x0 - c * step;
	const inY = py - g.y0 - r * stepH;
	if (inX > g.cell || inY > g.cellH) return null;
	return { r, c, fx: inX / g.cell };
}

// ---- polylines ----------------------------------------------------------------------------

/**
 * A curve as canvas points inside `rect`: x and y pairs, a NaN pair breaking the line where a
 * value is missing (or not positive on a log axis). A stepped curve gives two points per bin.
 */
export function curvePoints(cv: CellCurve, rect: Rect, xr: Range, yr: Range): Float32Array {
	const stepped = !!cv.edges && cv.edges.length === cv.y.length + 1;
	const n = cv.y.length;
	const out = new Float32Array((stepped ? 4 : 2) * n);
	const px = (f: number) => rect.x + frac(f, xr) * rect.w;
	const py = (v: number) => rect.y + rect.h - Math.min(1.02, Math.max(-0.02, frac(v, yr))) * rect.h;
	let k = 0;
	for (let i = 0; i < n; i++) {
		const v = cv.y[i];
		const ok = Number.isFinite(v) && (!yr.log || v > 0);
		if (stepped) {
			const a = cv.edges![i];
			const b = cv.edges![i + 1];
			const good = ok && a > 0 && b > 0;
			out[k++] = good ? px(a) : NaN;
			out[k++] = good ? py(v) : NaN;
			out[k++] = good ? px(b) : NaN;
			out[k++] = good ? py(v) : NaN;
		} else {
			const f = cv.x[i];
			const good = ok && f > 0;
			out[k++] = good ? px(f) : NaN;
			out[k++] = good ? py(v) : NaN;
		}
	}
	return out;
}

/** The value of a curve at frequency `f`: its bin's for a stepped curve, the nearest point's otherwise. */
export function curveValueAt(cv: CellCurve, f: number): number | null {
	if (cv.edges && cv.edges.length === cv.y.length + 1) return binValueAt(cv.edges, cv.y, f);
	const n = Math.min(cv.x.length, cv.y.length);
	if (!n || !(f > 0)) return null;
	let best = -1;
	let dist = Infinity;
	// x ascends: bisect, then compare the two neighbours on a log scale.
	let lo = 0;
	let hi = n - 1;
	while (hi - lo > 1) {
		const mid = (lo + hi) >> 1;
		if (cv.x[mid] <= f) lo = mid;
		else hi = mid;
	}
	for (const i of [lo, hi]) {
		const d = Math.abs(Math.log(cv.x[i] / f));
		if (d < dist) {
			dist = d;
			best = i;
		}
	}
	const v = cv.y[best];
	return Number.isFinite(v) ? v : null;
}

/** Every cell's ranges: one log range for all diagonal cells, one for all cross-spectrum cells. */
export function cellRanges(cells: CellData[]): Map<CellKind, Range> {
	const out = new Map<CellKind, Range>();
	out.set('psd', logRangeOf(cells.filter((c) => c.kind === 'psd').flatMap((c) => c.curves)));
	out.set('csd', logRangeOf(cells.filter((c) => c.kind === 'csd').flatMap((c) => c.curves)));
	out.set('coh', fixedRange('coh')!);
	out.set('phase', fixedRange('phase')!);
	return out;
}

export interface PreparedCell {
	cell: CellData;
	rect: Rect;
	yr: Range;
	paths: { role: Role; pts: Float32Array }[];
}

/** Everything a frame of the grid draws, in pixels: what the canvas strokes, and what is timed. */
export function prepareGrid(cells: CellData[], g: GridGeometry, xr: Range = frequencyRange(cells)): PreparedCell[] {
	const ranges = cellRanges(cells);
	return cells.map((cell) => {
		const rect = cellRect(g, cell.r, cell.c);
		const yr = ranges.get(cell.kind)!;
		return { cell, rect, yr, paths: cell.curves.map((cv) => ({ role: cv.role, pts: curvePoints(cv, rect, xr, yr) })) };
	});
}

export const CELL_WORDS: Record<CellKind, string> = {
	psd: 'Spectrum',
	coh: 'Coherence',
	phase: 'Phase',
	csd: 'Cross spectrum'
};
