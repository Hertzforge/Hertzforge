/**
 * How a plot panel presents the views the service sent for it: which lines are shown, on what
 * scales and ranges, in what looks, and how several channels share the panel (one pane each, all
 * on one pane, or one lane each on a time plot). Presentation only: every value drawn is the
 * service's. Cropping to a time window drops samples outside it; an envelope keeps each
 * interval's smallest and largest sample, both the service's.
 */

import type { ReadingView, Series, SpectrumView, View } from '$lib/protocol/types';
import type { SeriesOverride } from '$lib/viz/paneOptions';
import { DASH_STYLES, MARKER_ORDER, type MarkerShape } from '$lib/viz/roles';
import { plotTitle } from './display';
import { HELD_FAMILIES } from './hold';
import type { AxisDisplay, LineFamily, PlotDisplay, PlotPanelSpec, PlotType, TraceStyle } from './types';

/** One pane of a panel: the view it draws and how. */
export interface Pane {
	key: string;
	view: SpectrumView;
	/** One per series of `view`, by index. */
	styles: (SeriesOverride | undefined)[];
	xFixed: [number, number] | null;
	yFixed: [number, number] | null;
}

export interface Presented {
	panes: Pane[];
	/** Several panes: tiled in a grid, or stacked as lanes on one time axis. */
	layout: 'grid' | 'lanes';
	/** Several channels drawn on one pane. */
	overlaid: boolean;
	readings: ReadingView[];
}

const isData = (s: Pick<Series, 'role'>) => (HELD_FAMILIES as string[]).includes(s.role);

/** The view with the panel's axis scales in place of the service's (`auto` keeps the service's). */
export function withScales(view: SpectrumView, display: Pick<PlotDisplay, 'x' | 'y'>): SpectrumView {
	const x = display.x.scale === 'auto' ? view.x : { ...view.x, scale: display.x.scale };
	const y = display.y.scale === 'auto' ? view.y : { ...view.y, scale: display.y.scale };
	return x === view.x && y === view.y ? view : { ...view, x, y };
}

/** The view without the families of lines switched off. */
export function withoutFamilies(view: SpectrumView, hidden: readonly LineFamily[]): SpectrumView {
	if (!hidden.length) return view;
	const series = view.series.filter((s) => !hidden.includes(s.role));
	return series.length === view.series.length ? view : { ...view, series };
}

/** A series cut to the samples with x in [lo, hi]. */
function cut(s: Series, lo: number, hi: number): Series {
	const n = Math.min(s.x.length, s.y.length);
	let a = 0;
	while (a < n && !(s.x[a] >= lo)) a++;
	let b = n;
	while (b > a && !(s.x[b - 1] <= hi)) b--;
	if (a === 0 && b === n) return s;
	const out: Series = { ...s, x: s.x.slice(a, b), y: s.y.slice(a, b) };
	if (s.y_min) out.y_min = s.y_min.slice(a, b);
	if (s.edges) delete out.edges;
	if (s.shape === 'steps') delete out.shape;
	return out;
}

/** The last `seconds` of a time view: samples older than its newest less `seconds` are left out. */
export function cropWindow(view: SpectrumView, seconds: number | null): SpectrumView {
	if (!(seconds !== null && seconds > 0)) return view;
	let newest = -Infinity;
	for (const s of view.series) {
		for (let i = s.x.length - 1; i >= 0; i--) {
			if (Number.isFinite(s.x[i])) {
				newest = Math.max(newest, s.x[i]);
				break;
			}
		}
	}
	if (newest === -Infinity) return view;
	const from = newest - seconds;
	return { ...view, series: view.series.map((s) => cut(s, from, newest)) };
}

/**
 * A series as an envelope over `buckets` equal intervals of x: each interval's largest sample as
 * `y` and its smallest as `y_min`, at the interval's centre. A series already that short keeps its
 * samples, each its own minimum.
 */
export function envelopeOf(s: Series, buckets: number): Series {
	const n = Math.min(s.x.length, s.y.length);
	const lows = s.y_min ?? s.y;
	if (n <= buckets) return { ...s, y_min: s.y_min ?? s.y.slice(0, n) };
	let x0 = Infinity;
	let x1 = -Infinity;
	for (let i = 0; i < n; i++) {
		if (Number.isFinite(s.x[i])) {
			x0 = Math.min(x0, s.x[i]);
			x1 = Math.max(x1, s.x[i]);
		}
	}
	if (!(x1 > x0)) return { ...s, y_min: s.y_min ?? s.y.slice(0, n) };
	const width = (x1 - x0) / buckets;
	const hi = new Float64Array(buckets).fill(-Infinity);
	const lo = new Float64Array(buckets).fill(Infinity);
	for (let i = 0; i < n; i++) {
		const x = s.x[i];
		if (!Number.isFinite(x)) continue;
		const b = Math.min(buckets - 1, Math.floor((x - x0) / width));
		if (Number.isFinite(s.y[i])) hi[b] = Math.max(hi[b], s.y[i]);
		if (Number.isFinite(lows[i])) lo[b] = Math.min(lo[b], lows[i]);
	}
	const xs: number[] = [];
	const ys: number[] = [];
	const mins: number[] = [];
	for (let b = 0; b < buckets; b++) {
		if (hi[b] === -Infinity || lo[b] === Infinity) continue;
		xs.push(x0 + (b + 0.5) * width);
		ys.push(hi[b]);
		mins.push(lo[b]);
	}
	const out: Series = { name: s.name, role: s.role, x: Float64Array.from(xs), y: Float64Array.from(ys), y_min: Float64Array.from(mins) };
	return out;
}

/**
 * Several views on one pane: every series of every view, under the first view's axes (a panel
 * shows one quantity, so they agree). A name that more than one view uses is told apart by its
 * view's title, so the legend, the readout and a trace's look each name one line.
 */
export function overlay(views: SpectrumView[], title: string): SpectrumView {
	const uses = new Map<string, number>();
	for (const v of views) for (const s of v.series) uses.set(s.name, (uses.get(s.name) ?? 0) + 1);
	const taken = new Set<string>();
	const series = views.flatMap((v, vi) =>
		v.series.map((s) => {
			let name = (uses.get(s.name) ?? 0) > 1 ? `${s.name}, ${v.title}` : s.name;
			if (taken.has(name)) name = `${name} (${vi + 1})`;
			taken.add(name);
			// a follower's index points into its own view's series
			const offset = views.slice(0, vi).reduce((k, w) => k + w.series.length, 0);
			const out: Series = name === s.name ? { ...s } : { ...s, name };
			if (s.follows !== undefined) out.follows = s.follows + offset;
			return out;
		})
	);
	return { ...views[0], title, series, regions: undefined, handles: undefined, link: views[0].link };
}

/** The x extent of the test's band: the reference's, or the specification lines' when there is none. */
export function bandOf(views: View[]): [number, number] | null {
	const spans = (roles: string[]) => {
		let lo = Infinity;
		let hi = -Infinity;
		for (const v of views) {
			if (v.view !== 'spectrum') continue;
			for (const s of v.series) {
				if (!roles.includes(s.role)) continue;
				for (let i = 0; i < s.x.length; i++) {
					if (!Number.isFinite(s.x[i])) continue;
					lo = Math.min(lo, s.edges ? s.edges[0] : s.x[i]);
					hi = Math.max(hi, s.edges ? s.edges[s.edges.length - 1] : s.x[i]);
				}
			}
		}
		return hi > lo ? ([lo, hi] as [number, number]) : null;
	};
	return spans(['reference']) ?? spans(['tolerance', 'abort', 'limit']);
}

/** The range an axis is held to, or null when the pane ranges itself. */
export function axisRange(axis: AxisDisplay, log: boolean, band: [number, number] | null = null): [number, number] | null {
	const valid = (r: [number, number] | null): [number, number] | null => (r && Number.isFinite(r[0]) && Number.isFinite(r[1]) && r[1] > r[0] && (!log || r[0] > 0) ? r : null);
	switch (axis.range) {
		case 'auto':
			return null;
		case 'band':
			return valid(band);
		case 'fixed':
		case 'held':
			return axis.min === null || axis.max === null ? null : valid([axis.min, axis.max]);
	}
}

function override(t: TraceStyle | undefined, auto: MarkerShape | null): SeriesOverride | undefined {
	const marker = t?.marker === 'none' ? null : (t?.marker ?? auto);
	if (!t) return marker ? { marker } : undefined;
	const o: SeriesOverride = { visible: t.visible, marker };
	if (t.width !== null) o.width = t.width;
	if (t.dash) {
		o.dash = DASH_STYLES[t.dash].dash;
		o.dashName = DASH_STYLES[t.dash].words;
	}
	if (t.color) o.color = t.color;
	return o;
}

/**
 * Each series' look on a pane: the person's choices by name, and a marker for the second and
 * later measurements drawn together, so lines of one role differ by more than their colour.
 */
export function traceOverrides(view: SpectrumView, traces: Record<string, TraceStyle>): (SeriesOverride | undefined)[] {
	const data = view.series.filter((s) => isData(s) && s.follows === undefined).length;
	let k = 0;
	return view.series.map((s) => {
		let auto: MarkerShape | null = null;
		if (data > 1 && isData(s) && s.follows === undefined) {
			auto = k === 0 ? null : MARKER_ORDER[(k - 1) % MARKER_ORDER.length];
			k++;
		}
		return override(traces[s.name], auto);
	});
}

/** Whether a series is drawn: shown unless its look hides it. */
export const isShown = (styles: (SeriesOverride | undefined)[], i: number) => styles[i]?.visible !== false;

/**
 * The panes a panel draws. `band` is the test's band, for an x range held to it; without one the
 * band is read from the reference line. `buckets` is the envelope's resolution.
 */
export function present(views: View[], spec: PlotPanelSpec, type: PlotType | null, opts: { band?: [number, number] | null; buckets?: number } = {}): Presented {
	const d = spec.display;
	const time = type?.time ?? false;
	const spectra = views.filter((v): v is SpectrumView => v.view === 'spectrum');
	const readings = views.filter((v): v is ReadingView => v.view === 'reading');
	let shown = spectra.map((v) => withoutFamilies(v, d.hidden));
	if (time && d.window_s !== null) shown = shown.map((v) => cropWindow(v, d.window_s));
	if (time && d.timeRender === 'envelope') {
		const buckets = opts.buckets ?? 600;
		shown = shown.map((v) => ({ ...v, series: v.series.map((s) => (isData(s) ? envelopeOf(s, buckets) : s)) }));
	}
	const overlaid = shown.length > 1 && (time ? d.lanes === 'overlay' : spec.arrange === 'overlay');
	if (overlaid) shown = [overlay(shown, plotTitle(spec, type))];
	const band = opts.band ?? bandOf(spectra);
	const panes = shown.map((v, i): Pane => {
		const view = withScales(v, d);
		return {
			key: overlaid ? 'overlay' : `${i}:${v.title}`,
			view,
			styles: traceOverrides(view, d.traces),
			xFixed: axisRange(d.x, view.x.scale === 'log', time ? null : band),
			yFixed: axisRange(d.y, view.y.scale === 'log')
		};
	});
	return { panes, layout: time ? 'lanes' : 'grid', overlaid, readings };
}

/** The names and roles of the lines a panel draws, for its trace list. */
export function traceList(presented: Presented): { name: string; role: Series['role'] }[] {
	const seen = new Set<string>();
	const out: { name: string; role: Series['role'] }[] = [];
	for (const p of presented.panes) {
		for (const s of p.view.series) {
			if (seen.has(s.name)) continue;
			seen.add(s.name);
			out.push({ name: s.name, role: s.role });
		}
	}
	return out;
}
