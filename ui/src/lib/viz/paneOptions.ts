/**
 * What a line pane can be asked to draw beyond its view: a look per series, marks and notes over
 * the curves, the grid and the legend's place. Plain data and the arithmetic behind the pane's
 * zoom and grid; every value drawn is still the view's own.
 */

import type { MarkerShape } from './roles';

export type GridMode = 'major' | 'minor' | 'none';
export type LegendMode = 'outside' | 'inside' | 'off';
export type Corner = 'top-left' | 'top-right' | 'bottom-left' | 'bottom-right';
export type TickDensity = 'sparse' | 'normal' | 'dense';

/** One series' look in place of its role's; an absent field keeps the role's. */
export interface SeriesOverride {
	visible?: boolean;
	width?: number;
	/** Canvas dash pattern, CSS pixels; empty for solid. */
	dash?: readonly number[];
	/** The dash in words, for the legend. */
	dashName?: string;
	color?: string;
	/** Null draws none, even where the role's look would. */
	marker?: MarkerShape | null;
}

/** A vertical line over the curves at `x`, labelled at its top. */
export interface MarkLine {
	x: number;
	label: string;
	dash: readonly number[];
}

/** A marked point on a curve, labelled beside it. */
export interface MarkPoint {
	x: number;
	y: number;
	label: string;
}

/** What the cursors and marks put on a pane, and the lines of their readout. */
export interface PaneMarks {
	lines: MarkLine[];
	points: MarkPoint[];
	box: string[];
}

/** A note on the pane, at fractions of its drawing area. */
export interface PaneNote {
	id: string;
	text: string;
	fx: number;
	fy: number;
}

/** A zoom made on the pane: the new range of an axis, null to reset it, absent where unchanged. */
export interface ZoomChange {
	x?: [number, number] | null;
	y?: [number, number] | null;
}

/** How far apart ticks sit for each density, as a factor on the pane's own spacing. */
export const TICK_SPACE: Record<TickDensity, number> = { sparse: 1.8, normal: 1, dense: 0.6 };

/**
 * Minor grid lines between the major ticks: on a log axis every whole mantissa of each decade, on
 * a linear axis five steps to each major step. None where a major line already sits, none outside
 * [min, max], and none at all on a degenerate axis.
 */
export function minorSplits(min: number, max: number, log: boolean, majors: readonly number[]): number[] {
	if (!(Number.isFinite(min) && Number.isFinite(max)) || !(max > min)) return [];
	const near = (v: number) => majors.some((m) => Math.abs(m - v) <= Math.abs(v) * 1e-9 + 1e-300);
	const out: number[] = [];
	if (log) {
		if (!(min > 0)) return [];
		const e0 = Math.floor(Math.log10(min));
		const e1 = Math.ceil(Math.log10(max));
		if (e1 - e0 > 40) return [];
		for (let e = e0; e <= e1; e++) {
			for (let m = 1; m <= 9; m++) {
				const v = Number((m * 10 ** e).toPrecision(12));
				if (v >= min && v <= max && !near(v)) out.push(v);
			}
		}
		return out;
	}
	const sorted = [...majors].filter(Number.isFinite).sort((a, b) => a - b);
	if (sorted.length < 2) return [];
	const step = (sorted[1] - sorted[0]) / 5;
	if (!(step > 0) || (max - min) / step > 2000) return [];
	const start = Math.ceil((min - sorted[0]) / step);
	for (let k = start; ; k++) {
		const v = sorted[0] + k * step;
		if (v > max) break;
		if (!near(v)) out.push(Number(v.toPrecision(12)));
	}
	return out;
}

/**
 * The range after zooming [lo, hi] by `factor` about `at` (below 1 zooms in). On a log axis the
 * zoom is even in decades. A range that would leave the log axis's positive side is not taken.
 */
export function zoomAbout(lo: number, hi: number, at: number, factor: number, log: boolean): [number, number] {
	if (!(hi > lo) || !(factor > 0)) return [lo, hi];
	if (log) {
		if (!(lo > 0) || !(at > 0)) return [lo, hi];
		const [a, b, c] = [Math.log10(lo), Math.log10(hi), Math.log10(at)];
		const t = (c - a) / (b - a);
		const span = (b - a) * factor;
		const start = c - t * span;
		return [10 ** start, 10 ** (start + span)];
	}
	const t = (at - lo) / (hi - lo);
	const span = (hi - lo) * factor;
	const start = at - t * span;
	return [start, start + span];
}

/** The range moved by `fraction` of its own width (positive moves right), even in decades on a log axis. */
export function panBy(lo: number, hi: number, fraction: number, log: boolean): [number, number] {
	if (!(hi > lo) || !Number.isFinite(fraction)) return [lo, hi];
	if (log) {
		if (!(lo > 0)) return [lo, hi];
		const d = Math.log10(hi / lo) * fraction;
		return [lo * 10 ** d, hi * 10 ** d];
	}
	const d = (hi - lo) * fraction;
	return [lo + d, hi + d];
}

/**
 * The indices of a column to put markers on: one each time the line has moved `gap` pixels
 * along x since the last, so markers stay apart however dense the samples are. The first sits
 * `offset` pixels in from the line's start, so the markers of lines drawn together interleave.
 * `px` maps an index to its x pixel; missing values are skipped.
 */
export function markerIndices(values: ArrayLike<number | null>, px: (i: number) => number, gap: number, offset = 0): number[] {
	const out: number[] = [];
	let start: number | null = null;
	let last = -Infinity;
	for (let i = 0; i < values.length; i++) {
		const v = values[i];
		if (v === null || !Number.isFinite(v)) continue;
		const x = px(i);
		if (!Number.isFinite(x)) continue;
		start ??= x;
		if (out.length === 0 ? x - start >= offset : x - last >= gap) {
			out.push(i);
			last = x;
		}
	}
	return out;
}
