/**
 * The live display's layout: named pages, each holding its own plots, and what each plot shows.
 *
 * A page is what the operator switches between from the drop-down ("Control", "Limits",
 * "Responses"); a plot is one quantity from the service (PROTOCOL.md, "Live Data Subscriptions")
 * with the channels and axes the operator chose. The layout is saved per test type and per test
 * setup, so a setup comes back with the pages it was run with. It is plain data: nothing here
 * draws or computes a measurement.
 *
 * Today the layout lives in the browser's storage. When test setups are saved as `.hforge` files,
 * the layout travels inside the setup (`toSaved` / `fromSaved` are the shape it will be written
 * in).
 */

import type { ChannelSelector, LiveQuantity, SubscribePlot } from '$lib/protocol/types';

export type ScaleChoice = 'auto' | 'log' | 'linear';
/** How a plot of several channels draws them: one small pane each, or all on one pane. */
export type Arrange = 'split' | 'overlay';

export interface PlotSpec {
	/** Stable within the layout; the subscription's plot id. */
	id: string;
	quantity: LiveQuantity;
	/** For `response` and `time`; ignored otherwise. */
	channels: ChannelSelector;
	arrange: Arrange;
	/** `auto` keeps the service's scale for the quantity. */
	x: ScaleChoice;
	y: ScaleChoice;
	/** A fixed y range when both are set; otherwise the plot ranges itself. */
	yMin: number | null;
	yMax: number | null;
	/** For `time`: the most points per channel. */
	points: number;
}

export interface Page {
	id: string;
	name: string;
	plots: PlotSpec[];
}

export interface Layout {
	version: 1;
	pages: Page[];
	/** Display updates per second this window asks for. */
	rate: number;
	/**
	 * The last number given to a page or a plot. Numbers are never given twice, so a frame still
	 * on its way for a removed plot can never land on a new one.
	 */
	serial: number;
}

export const QUANTITY_WORDS: Record<LiveQuantity, string> = {
	control: 'Control and Reference',
	drive: 'Drive',
	response: 'Response Spectra',
	time: 'Time History',
	coherence: 'Coherence',
	readings: 'Run Numbers'
};

/** One plain line on what each quantity shows, for the plot editor. */
export const QUANTITY_HELP: Record<LiveQuantity, string> = {
	control: 'The control spectrum against the reference, with the alarm and abort lines.',
	drive: 'The drive spectrum sent to the amplifier.',
	response: "Each chosen channel's spectrum, with its limit line when the setup limits it.",
	time: "Each chosen channel's samples from the last control loop.",
	coherence: "The system identification's coherence between drive and control.",
	readings: 'The run numbers: control, reference and drive rms, level and lines in alarm.'
};

export const QUANTITY_ORDER: LiveQuantity[] = ['control', 'drive', 'response', 'time', 'coherence', 'readings'];

export const RATES = [1, 2, 5, 10];
export const DEFAULT_RATE = 5;
export const MAX_PAGES = 12;
export const MAX_PLOTS = 12;
const MAX_NAME = 40;

export function hasChannels(q: LiveQuantity): boolean {
	return q === 'response' || q === 'time';
}

export function hasAxes(q: LiveQuantity): boolean {
	return q !== 'readings';
}

/** A new plot of `quantity`, with the quantity's own defaults. */
export function newPlot(id: string, quantity: LiveQuantity, channels: ChannelSelector = 'all'): PlotSpec {
	return { id, quantity, channels, arrange: 'split', x: 'auto', y: 'auto', yMin: null, yMax: null, points: 1024 };
}

/** The Random test's pages until the operator changes them. */
export function defaultRandomLayout(): Layout {
	return {
		version: 1,
		rate: DEFAULT_RATE,
		serial: 4,
		pages: [
			{ id: 'pg-1', name: 'Control', plots: [newPlot('pl-1', 'control'), newPlot('pl-2', 'drive')] },
			{ id: 'pg-2', name: 'Limits', plots: [newPlot('pl-3', 'response', 'limited')] },
			{ id: 'pg-3', name: 'Responses', plots: [newPlot('pl-4', 'response', 'all')] }
		]
	};
}

/** The highest number in `ids` of the form `<prefix>-<n>`, or 0. */
export function highest(ids: Iterable<string>): number {
	let n = 0;
	for (const id of ids) {
		const m = Number(id.replace(/^[a-z]+-/, ''));
		if (Number.isInteger(m) && m > n) n = m;
	}
	return n;
}

/** A new id for `layout`, never one used before in it; the layout keeps the count. */
export function nextId(layout: Layout, prefix: 'pg' | 'pl'): string {
	const used = [...layout.pages.map((p) => p.id), ...plotIds(layout)];
	layout.serial = Math.max(layout.serial, highest(used)) + 1;
	return `${prefix}-${layout.serial}`;
}

export function plotIds(layout: Layout): string[] {
	return layout.pages.flatMap((p) => p.plots.map((x) => x.id));
}

/** A page name: trimmed, at most 40 characters; empty names are refused (null). */
export function cleanName(name: string): string | null {
	const t = name.replace(/\s+/g, ' ').trim().slice(0, MAX_NAME);
	return t.length ? t : null;
}

/** A name not yet used by another page: "Page 4", or "Responses 2". */
export function freeName(layout: Layout, base = 'Page'): string {
	const used = new Set(layout.pages.map((p) => p.name.toLowerCase()));
	if (base !== 'Page' && !used.has(base.toLowerCase())) return base;
	for (let n = base === 'Page' ? layout.pages.length + 1 : 2; ; n++) {
		const name = `${base} ${n}`;
		if (!used.has(name.toLowerCase())) return name;
	}
}

/** What a plot shows, in words: its title. */
export function plotTitle(p: PlotSpec): string {
	const q = QUANTITY_WORDS[p.quantity];
	if (!hasChannels(p.quantity)) return q;
	return `${q}: ${channelWords(p.channels)}`;
}

export function channelWords(c: ChannelSelector): string {
	if (c === 'all') return 'all inputs';
	if (c === 'control') return 'control channels';
	if (c === 'limited') return 'limited channels';
	if (c.length === 0) return 'no channels';
	return `${c.length === 1 ? 'channel' : 'channels'} ${c.join(', ')}`;
}

/** The subscription a window sends for the plots it shows. */
export function subscriptionFor(plots: PlotSpec[]): SubscribePlot[] {
	return plots.map((p) => {
		const out: SubscribePlot = { id: p.id, quantity: p.quantity };
		if (hasChannels(p.quantity)) out.channels = Array.isArray(p.channels) ? [...p.channels] : p.channels;
		if (p.quantity === 'time') out.points = p.points;
		return out;
	});
}

// ---- reading a saved layout back --------------------------------------------------------------

const QUANTITIES = new Set<string>(QUANTITY_ORDER);
const SCALES = new Set<string>(['auto', 'log', 'linear']);

function finiteOrNull(v: unknown): number | null {
	return typeof v === 'number' && Number.isFinite(v) ? v : null;
}

function readSelector(v: unknown): ChannelSelector {
	if (v === 'all' || v === 'control' || v === 'limited') return v;
	if (Array.isArray(v)) {
		const ids = [...new Set(v.filter((x): x is number => Number.isInteger(x) && x >= 0))];
		return ids;
	}
	return 'all';
}

function readPlot(v: unknown, ids: Set<string>): PlotSpec | null {
	if (typeof v !== 'object' || v === null) return null;
	const o = v as Record<string, unknown>;
	if (typeof o.id !== 'string' || !o.id || o.id.length > 64 || ids.has(o.id)) return null;
	if (typeof o.quantity !== 'string' || !QUANTITIES.has(o.quantity)) return null;
	ids.add(o.id);
	const points = Number.isInteger(o.points) && (o.points as number) >= 64 && (o.points as number) <= 8192 ? (o.points as number) : 1024;
	return {
		id: o.id,
		quantity: o.quantity as LiveQuantity,
		channels: readSelector(o.channels),
		arrange: o.arrange === 'overlay' ? 'overlay' : 'split',
		x: typeof o.x === 'string' && SCALES.has(o.x) ? (o.x as ScaleChoice) : 'auto',
		y: typeof o.y === 'string' && SCALES.has(o.y) ? (o.y as ScaleChoice) : 'auto',
		yMin: finiteOrNull(o.yMin),
		yMax: finiteOrNull(o.yMax),
		points
	};
}

/** A layout read from storage, checked field by field; null when it is not one. */
export function readLayout(raw: unknown): Layout | null {
	if (typeof raw !== 'object' || raw === null) return null;
	const o = raw as Record<string, unknown>;
	if (o.version !== 1 || !Array.isArray(o.pages)) return null;
	const pageIds = new Set<string>();
	const plotIdsSeen = new Set<string>();
	const pages: Page[] = [];
	for (const pv of o.pages.slice(0, MAX_PAGES)) {
		if (typeof pv !== 'object' || pv === null) continue;
		const p = pv as Record<string, unknown>;
		if (typeof p.id !== 'string' || !p.id || pageIds.has(p.id)) continue;
		const name = typeof p.name === 'string' ? cleanName(p.name) : null;
		if (!name) continue;
		pageIds.add(p.id);
		const plots = (Array.isArray(p.plots) ? p.plots : [])
			.slice(0, MAX_PLOTS)
			.map((x) => readPlot(x, plotIdsSeen))
			.filter((x): x is PlotSpec => x !== null);
		pages.push({ id: p.id, name, plots });
	}
	if (!pages.length) return null;
	const rate = typeof o.rate === 'number' && RATES.includes(o.rate) ? o.rate : DEFAULT_RATE;
	const used = highest([...pages.map((p) => p.id), ...pages.flatMap((p) => p.plots.map((x) => x.id))]);
	const serial = Math.max(used, Number.isInteger(o.serial) ? (o.serial as number) : 0);
	return { version: 1, pages, rate, serial };
}

/**
 * The shape a layout is saved in (storage now, the `.hforge` setup later). The fields are written
 * in one order, whatever order they were set in, so two equal layouts always save the same text.
 */
export function toSaved(layout: Layout): string {
	const plot = (p: PlotSpec) => ({ id: p.id, quantity: p.quantity, channels: p.channels, arrange: p.arrange, x: p.x, y: p.y, yMin: p.yMin, yMax: p.yMax, points: p.points });
	return JSON.stringify({
		version: layout.version,
		pages: layout.pages.map((p) => ({ id: p.id, name: p.name, plots: p.plots.map(plot) })),
		rate: layout.rate,
		serial: layout.serial
	});
}

export function fromSaved(text: string | null): Layout | null {
	if (!text) return null;
	try {
		return readLayout(JSON.parse(text));
	} catch {
		return null;
	}
}

/** Where a layout is kept: per test type and per setup (by its title). */
export function layoutKey(testType: string, setupTitle: string | null | undefined): string {
	const t = (setupTitle ?? '').replace(/\s+/g, ' ').trim().toLowerCase();
	return `hertzforge.display.v1:${testType}:${t || 'untitled'}`;
}

/** The test type's most recent layout, the starting point for a setup that has none of its own. */
export function lastLayoutKey(testType: string): string {
	return `hertzforge.display.v1:${testType}:*last`;
}
