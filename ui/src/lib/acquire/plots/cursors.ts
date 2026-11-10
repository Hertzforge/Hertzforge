/**
 * Cursors and marks on a plot: the peak, the top peaks, harmonics of a frequency, and two cursors'
 * difference. Each reads the series the service sent; nothing is fitted or interpolated beyond the
 * straight segment the plot draws between samples.
 */

import type { Series, SpectrumView } from '$lib/protocol/types';
import { valueAt } from '$lib/viz/align';
import { formatFixed } from '$lib/viz/format';
import type { PaneMarks } from '$lib/viz/paneOptions';
import { DASH_STYLES } from '$lib/viz/roles';
import type { CursorMode, PlotDisplay } from './types';

export interface Mark {
	x: number;
	y: number;
	index: number;
}

type XY = Pick<Series, 'x' | 'y'>;

const inside = (x: number, lo: number | null | undefined, hi: number | null | undefined) => (lo == null || x >= lo) && (hi == null || x <= hi);

/** The largest finite value of a series within [lo, hi] (all of it when not given); null when it has none. */
export function peak(s: XY, lo?: number | null, hi?: number | null): Mark | null {
	let best: Mark | null = null;
	const n = Math.min(s.x.length, s.y.length);
	for (let i = 0; i < n; i++) {
		const y = s.y[i];
		if (!Number.isFinite(y) || !inside(s.x[i], lo, hi)) continue;
		if (best === null || y > best.y) best = { x: s.x[i], y, index: i };
	}
	return best;
}

/**
 * The `n` highest local maxima of a series, highest first, no two closer on x than
 * `separation` (measured in decades when `logX`). A point is a local maximum when it is no lower
 * than the point before and higher than the point after; an end must stand above its one
 * neighbour. A higher peak always wins over a lower one too close to it.
 */
export function topPeaks(s: XY, n: number, separation: number, logX: boolean, lo?: number | null, hi?: number | null): Mark[] {
	const len = Math.min(s.x.length, s.y.length);
	const finite = (i: number) => i >= 0 && i < len && Number.isFinite(s.y[i]);
	const candidates: Mark[] = [];
	for (let i = 0; i < len; i++) {
		if (!finite(i) || !inside(s.x[i], lo, hi) || (logX && !(s.x[i] > 0))) continue;
		const y = s.y[i];
		const before = finite(i - 1) ? s.y[i - 1] : -Infinity;
		const after = finite(i + 1) ? s.y[i + 1] : -Infinity;
		// the last point has no neighbour after it, so it must stand above the one before
		const rises = after === -Infinity ? y > before : y >= before;
		if (rises && y > after) candidates.push({ x: s.x[i], y, index: i });
	}
	candidates.sort((a, b) => b.y - a.y || a.x - b.x);
	const where = (x: number) => (logX ? Math.log10(x) : x);
	const out: Mark[] = [];
	for (const c of candidates) {
		if (out.length >= n) break;
		if (out.every((m) => Math.abs(where(m.x) - where(c.x)) >= separation)) out.push(c);
	}
	return out;
}

/** A sensible least separation of peaks: `fraction` of the span [lo, hi], in decades on a log axis. */
export function separationFor(lo: number, hi: number, logX: boolean, fraction = 0.02): number {
	if (logX) return lo > 0 && hi > lo ? Math.log10(hi / lo) * fraction : 0;
	return hi > lo ? (hi - lo) * fraction : 0;
}

/** The fundamental `f` and its harmonics, `n` in all (f, 2f, ... nf), up to `xmax`. */
export function harmonics(f: number, n: number, xmax = Infinity): number[] {
	if (!(f > 0) || !Number.isFinite(f)) return [];
	const out: number[] = [];
	for (let k = 1; k <= n && k * f <= xmax; k++) out.push(k * f);
	return out;
}

/**
 * Two cursors' difference on a series: B less A on x, and on y the change (a linear axis) or the
 * ratio (a log axis, where a ratio is what the eye reads). Null where a cursor reads nothing.
 */
export function difference(xa: number, xb: number, ya: number | null, yb: number | null, logY: boolean): { dx: number; dy: number | null; ratio: number | null } {
	const both = ya !== null && yb !== null && Number.isFinite(ya) && Number.isFinite(yb);
	return {
		dx: xb - xa,
		dy: both && !logY ? (yb as number) - (ya as number) : null,
		ratio: both && logY && (ya as number) > 0 ? (yb as number) / (ya as number) : null
	};
}

/**
 * Where a click at `x` leaves the cursors. One cursor (and harmonics, which hang from it) moves to
 * the click. With two, the click places A, then B, then moves whichever is nearer (in decades on
 * a log axis).
 */
export function placeCursor(at: [number | null, number | null], x: number, mode: CursorMode, logX: boolean): [number | null, number | null] {
	if (!Number.isFinite(x) || (logX && !(x > 0))) return at;
	const [a, b] = at;
	if (mode !== 'two') return [x, b];
	if (a === null) return [x, b];
	if (b === null) return [a, x];
	const d = (u: number) => (logX && u > 0 ? Math.abs(Math.log10(u / x)) : Math.abs(u - x));
	return d(a) <= d(b) ? [x, b] : [a, x];
}

const READ_ROLES: string[] = ['measured', 'drive', 'other', 'reference'];

const num = (v: number | null) => (v === null || !Number.isFinite(v) ? '-' : formatFixed(v, 4));
const unitOf = (u: string | null) => (u ? ` ${u}` : '');

/** The series a mark reads: the first shown measured one, or else the first shown. */
export function primarySeries(view: SpectrumView, shown: (i: number) => boolean): number {
	const measured = view.series.findIndex((s, i) => shown(i) && s.role === 'measured');
	if (measured >= 0) return measured;
	return view.series.findIndex((_, i) => shown(i));
}

/**
 * What the plot's cursors and marks put on one pane: lines, labelled points, and the readout's
 * lines. `shown` says which series are drawn; only those are read.
 */
export function paneMarks(view: SpectrumView, display: Pick<PlotDisplay, 'cursor' | 'cursorAt' | 'count'>, shown: (i: number) => boolean = () => true): PaneMarks {
	const marks: PaneMarks = { lines: [], points: [], box: [] };
	const mode = display.cursor;
	if (mode === 'none') return marks;
	const logX = view.x.scale === 'log';
	const logY = view.y.scale === 'log';
	const xu = unitOf(view.x.unit);
	const yu = unitOf(view.y.unit);
	const p = primarySeries(view, shown);
	const s = p >= 0 ? view.series[p] : null;
	const [a, b] = display.cursorAt ?? [null, null];
	const read = (series: Series, x: number) => valueAt(series, x, logX, logY);
	// the cursors read the measurements and the reference; the alarm, abort and limit lines are
	// read from the legend's readout, so the box stays short
	const reads = (i: number) => shown(i) && READ_ROLES.includes(view.series[i].role);

	if (mode === 'one' || mode === 'two') {
		const placed: [string, number][] = [];
		if (a !== null) placed.push(['A', a]);
		if (mode === 'two' && b !== null) placed.push(['B', b]);
		for (const [label, x] of placed) {
			marks.lines.push({ x, label, dash: label === 'A' ? DASH_STYLES.solid.dash : DASH_STYLES.dashed.dash });
			marks.box.push(`${label}  ${num(x)}${xu}`);
			view.series.forEach((series, i) => {
				if (reads(i)) marks.box.push(`   ${series.name}  ${num(read(series, x))}${yu}`);
			});
		}
		if (mode === 'two' && a !== null && b !== null) {
			marks.box.push(`B-A  ${num(b - a)}${xu}`);
			view.series.forEach((series, i) => {
				if (!reads(i)) return;
				const d = difference(a, b, read(series, a), read(series, b), logY);
				marks.box.push(logY ? `   ${series.name}  B/A ${num(d.ratio)}` : `   ${series.name}  ${num(d.dy)}${yu}`);
			});
		}
		return marks;
	}

	if (mode === 'peak') {
		view.series.forEach((series, i) => {
			if (!shown(i) || series.role !== 'measured') return;
			const m = peak(series);
			if (!m) return;
			marks.points.push({ x: m.x, y: m.y, label: 'Peak' });
			marks.box.push(`Peak ${series.name}  ${num(m.x)}${xu}  ${num(m.y)}${yu}`);
		});
		return marks;
	}

	if (!s) return marks;
	if (mode === 'peaks') {
		const n = Math.min(s.x.length, s.y.length);
		const sep = n ? separationFor(s.x[0], s.x[n - 1], logX) : 0;
		topPeaks(s, display.count, sep, logX).forEach((m, k) => {
			marks.points.push({ x: m.x, y: m.y, label: String(k + 1) });
			marks.box.push(`${k + 1}  ${num(m.x)}${xu}  ${num(m.y)}${yu}`);
		});
		return marks;
	}

	// harmonics of cursor A
	if (a === null) return marks;
	const xmax = s.x.length ? s.x[s.x.length - 1] : Infinity;
	harmonics(a, display.count, xmax).forEach((x, k) => {
		const label = k === 0 ? 'f' : `${k + 1}f`;
		marks.lines.push({ x, label, dash: k === 0 ? DASH_STYLES.solid.dash : DASH_STYLES.dotted.dash });
		marks.box.push(`${label}  ${num(x)}${xu}  ${num(read(s, x))}${yu}`);
	});
	return marks;
}
