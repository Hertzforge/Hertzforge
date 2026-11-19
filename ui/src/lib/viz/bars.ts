/**
 * The layout of a `bars` view: the value axis's range and ticks, and where each bar runs. The
 * service has judged every bar (`states`); this only places them.
 */
import type { BarsView } from '$lib/protocol/types';
import { niceTicks } from './axes';

/** The value axis: its ends, after padding, and its ticks. */
export interface BarsAxis {
	lo: number;
	hi: number;
	ticks: number[];
	decimals: number;
}

/**
 * The value axis of `view`: it spans the bars, the baseline and both thresholds, padded by an
 * eighth either side, and never reaches below `floor`. Rows with no number have no say.
 */
export function barsAxis(view: BarsView): BarsAxis {
	const base = view.baseline ?? 0;
	const known = view.values.filter((v): v is number => v !== null && Number.isFinite(v));
	const marks = [base, ...known];
	if (view.under !== null && view.under !== undefined) marks.push(view.under);
	if (view.over !== null && view.over !== undefined) marks.push(view.over);
	let lo = Math.min(...marks);
	let hi = Math.max(...marks);
	const pad = 0.12 * (hi - lo || 1);
	lo -= pad;
	hi += pad;
	if (view.floor !== null && view.floor !== undefined) lo = Math.max(lo, Math.min(view.floor, hi - 1e-9));
	const { values, decimals } = niceTicks(lo, hi, 5);
	return { lo, hi, ticks: values.filter((t) => t >= lo - 1e-9 && t <= hi + 1e-9), decimals };
}

/** Where a value lies across a plot `width` pixels wide starting at `left`. */
export function place(axis: BarsAxis, left: number, width: number, value: number): number {
	return left + ((value - axis.lo) / (axis.hi - axis.lo)) * width;
}

/** A bar's span across the plot, from the baseline to its value: [start, end] in pixels, start the smaller; null with no number. */
export function barSpan(axis: BarsAxis, left: number, width: number, base: number, value: number | null): [number, number] | null {
	if (value === null || !Number.isFinite(value)) return null;
	const a = place(axis, left, width, base);
	const b = place(axis, left, width, value);
	return [Math.min(a, b), Math.max(a, b)];
}

/** How a row is said: its label, its value with the unit, and its state in words. */
export function barText(view: BarsView, row: number, decimals = 2): string {
	const value = view.values[row];
	const unit = view.value.unit ? ` ${view.value.unit}` : '';
	const said = value === null ? 'no reading' : `${value.toFixed(decimals)}${unit}`;
	const state = view.states[row];
	const note = view.notes?.[row];
	return [`${view.labels[row]}: ${said}`, state ? `${state} its limit` : null, note].filter(Boolean).join(', ');
}
