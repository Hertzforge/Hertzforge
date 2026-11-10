/**
 * A test screen's layout as data: the Random test's own pages, how a layout is kept (per test type
 * and per setup, in this browser's storage now, inside the `.hforge` setup later), how a kept one
 * is read back, and how Random's layouts from before the page grid are taken over.
 *
 * Nothing kept is trusted: a layout is read field by field, anything that is not what it should
 * be is dropped or replaced by its default, and panels that do not fit their page are tiled
 * evenly again.
 */

import type { ChannelSelector } from '$lib/protocol/types';
import {
	cleanName,
	DEFAULT_RATE,
	fromSaved as fromSavedV1,
	highest,
	lastLayoutKey as lastLayoutKeyV1,
	layoutKey as layoutKeyV1,
	QUANTITY_HELP,
	QUANTITY_ORDER,
	QUANTITY_WORDS,
	RATES,
	type Layout as LayoutV1,
	type PlotSpec as PlotSpecV1
} from '../display/layout';
import {
	defaultAxis,
	defaultDisplay,
	newPlotSpec,
	type AxisDisplay,
	type LineFamily,
	type PlotDisplay,
	type PlotNote,
	type PlotPanelSpec,
	type PlotType,
	type TraceStyle
} from '../plots/types';
import { arrangeEvenly, clampCell, fits, tiling } from './grid';
import { GRID_COLS, GRID_ROWS, type CardKind, type Cell, type Dock, type DockCard, type Panel, type ScreenLayout, type ScreenPage } from './types';

export { cleanName, DEFAULT_RATE, RATES };

/** The grids a page offers: coarser, the default, finer. */
export const GRID_SIZES = [6, 12, 24] as const;
export const MAX_PAGES = 12;
/** Plots and cards on one page. */
export const MAX_PANELS = 16;
export const MAX_PLOTS = 12;
/** The shape of the page area a layout is made for before one is measured, width over height. */
export const DEFAULT_ASPECT = 1.4;

/** The dock's size, width on the right or height at the bottom, in CSS pixels. */
export const DOCK_SIZE = { right: { min: 240, max: 640, start: 320 }, bottom: { min: 140, max: 520, start: 230 } } as const;

/** The cards a screen may offer, by name. */
export const CARD_WORDS: Record<string, string> = {
	run: 'Run',
	level: 'Level',
	readings: 'Readings',
	inputs: 'Inputs',
	log: 'Log',
	marks: 'Marks'
};

export function cardWords(kind: CardKind): string {
	return CARD_WORDS[kind] ?? kind.charAt(0).toUpperCase() + kind.slice(1);
}

// ---- Random -----------------------------------------------------------------------------------

const RANDOM_GROUP: Record<string, PlotType['group']> = {
	control: 'frequency',
	drive: 'frequency',
	response: 'frequency',
	coherence: 'frequency',
	time: 'time',
	readings: 'numbers'
};

/** The lines each of Random's quantities draws, by the role the service gives each. */
const RANDOM_FAMILIES: Record<string, LineFamily[] | undefined> = {
	control: ['measured', 'reference', 'tolerance', 'abort'],
	drive: ['drive'],
	response: ['measured', 'limit'],
	time: ['measured'],
	coherence: ['measured'],
	readings: undefined
};

/** What a Random test offers to plot, in the order `+ Plot` lists it. */
export const RANDOM_TYPES: PlotType[] = QUANTITY_ORDER.map((q) => {
	const t: PlotType = { id: q, words: QUANTITY_WORDS[q], help: QUANTITY_HELP[q], group: RANDOM_GROUP[q], channels: q === 'response' || q === 'time', time: q === 'time' };
	const families = RANDOM_FAMILIES[q];
	if (families) t.families = families;
	return t;
});

/** The cards a Random test offers, in the dock's first order. */
export const RANDOM_CARDS: CardKind[] = ['run', 'level', 'readings', 'inputs', 'log'];

export function defaultDock(cards: CardKind[]): Dock {
	return { side: 'right', size: DOCK_SIZE.right.start, cards: cards.map((kind) => ({ kind, open: true, shown: true })) };
}

/** A page of plots tiled evenly. */
export function plotPage(id: string, name: string, plots: { id: string; plot: PlotPanelSpec }[], aspect = DEFAULT_ASPECT): ScreenPage {
	const cells = tiling(plots.length, GRID_COLS, GRID_ROWS, aspect);
	return { id, name, cols: GRID_COLS, rows: GRID_ROWS, linkTime: false, panels: plots.map((p, i) => ({ id: p.id, kind: 'plot' as const, at: cells[i], plot: p.plot })) };
}

function randomType(id: string): PlotType {
	return RANDOM_TYPES.find((t) => t.id === id) as PlotType;
}

/** The Random test's pages until the operator changes them: Control, Limits and Responses. */
export function defaultRandomScreen(aspect = DEFAULT_ASPECT): ScreenLayout {
	const plot = (id: string, q: string, channels: ChannelSelector = 'all') => ({ id, plot: newPlotSpec(randomType(q), channels) });
	return {
		version: 2,
		pages: [
			plotPage('pg-1', 'Control', [plot('pl-1', 'control'), plot('pl-2', 'drive')], aspect),
			plotPage('pg-2', 'Limits', [plot('pl-3', 'response', 'limited')], aspect),
			plotPage('pg-3', 'Responses', [plot('pl-4', 'response', 'all')], aspect)
		],
		dock: defaultDock(RANDOM_CARDS),
		rate: DEFAULT_RATE,
		serial: 4
	};
}

// ---- ids and names ----------------------------------------------------------------------------

export function layoutIds(layout: ScreenLayout): string[] {
	return [...layout.pages.map((p) => p.id), ...layout.pages.flatMap((p) => p.panels.map((x) => x.id))];
}

/** A new id for `layout`, never one used before in it; the layout keeps the count. */
export function nextId(layout: ScreenLayout, prefix: 'pg' | 'pl' | 'cd'): string {
	layout.serial = Math.max(layout.serial, highest(layoutIds(layout))) + 1;
	return `${prefix}-${layout.serial}`;
}

/** A name not yet used by another page: "Page 4". */
export function freePageName(layout: ScreenLayout): string {
	const used = new Set(layout.pages.map((p) => p.name.toLowerCase()));
	for (let n = layout.pages.length + 1; ; n++) if (!used.has(`page ${n}`)) return `Page ${n}`;
}

/** A plot panel's title: its own, else its plot type's name with the channels it shows. */
export function plotTitle(spec: PlotPanelSpec, type: PlotType | null): string {
	if (spec.title) return spec.title;
	const words = type?.words ?? spec.quantity;
	return type?.channels ? `${words}: ${channelWords(spec.channels)}` : words;
}

export function channelWords(c: ChannelSelector): string {
	if (c === 'all') return 'all inputs';
	if (c === 'control') return 'control channels';
	if (c === 'limited') return 'limited channels';
	if (c.length === 0) return 'no channels';
	return `${c.length === 1 ? 'channel' : 'channels'} ${c.join(', ')}`;
}

// ---- reading a kept layout back ---------------------------------------------------------------

type Raw = Record<string, unknown>;

const isObj = (v: unknown): v is Raw => typeof v === 'object' && v !== null && !Array.isArray(v);
const oneOf = <T extends string>(v: unknown, set: readonly T[], fallback: T): T => (typeof v === 'string' && (set as readonly string[]).includes(v) ? (v as T) : fallback);
const finiteOrNull = (v: unknown): number | null => (typeof v === 'number' && Number.isFinite(v) ? v : null);
const wholeIn = (v: unknown, lo: number, hi: number, fallback: number): number => (Number.isInteger(v) && (v as number) >= lo && (v as number) <= hi ? (v as number) : fallback);
const textOrNull = (v: unknown, max: number): string | null => {
	if (typeof v !== 'string') return null;
	const t = v.replace(/\s+/g, ' ').trim().slice(0, max);
	return t.length ? t : null;
};
const idOf = (v: unknown): string | null => (typeof v === 'string' && v.length > 0 && v.length <= 64 ? v : null);

const FAMILIES: LineFamily[] = ['measured', 'reference', 'tolerance', 'abort', 'limit', 'drive', 'other'];
const DASHES = ['solid', 'dashed', 'dotted', 'dashdot'] as const;

export function readSelector(v: unknown): ChannelSelector {
	if (v === 'all' || v === 'control' || v === 'limited') return v;
	if (Array.isArray(v)) return [...new Set(v.filter((x): x is number => Number.isInteger(x) && x >= 0 && x < 4096))];
	return 'all';
}

function readAxis(v: unknown): AxisDisplay {
	const d = defaultAxis();
	if (!isObj(v)) return d;
	return {
		scale: oneOf(v.scale, ['auto', 'log', 'linear'], d.scale),
		range: oneOf(v.range, ['auto', 'band', 'fixed', 'held'], d.range),
		min: finiteOrNull(v.min),
		max: finiteOrNull(v.max),
		label: textOrNull(v.label, 80),
		unit: textOrNull(v.unit, 32),
		ticks: oneOf(v.ticks, ['sparse', 'normal', 'dense'], d.ticks)
	};
}

function readTrace(v: unknown): TraceStyle | null {
	if (!isObj(v)) return null;
	const width = finiteOrNull(v.width);
	const color = typeof v.color === 'string' && /^#[0-9a-f]{3,8}$/i.test(v.color) ? v.color : null;
	return {
		visible: v.visible !== false,
		width: width !== null && width >= 0.5 && width <= 8 ? width : null,
		dash: typeof v.dash === 'string' && (DASHES as readonly string[]).includes(v.dash) ? (v.dash as TraceStyle['dash']) : null,
		color
	};
}

function readNotes(v: unknown): PlotNote[] {
	if (!Array.isArray(v)) return [];
	const seen = new Set<string>();
	const out: PlotNote[] = [];
	for (const n of v.slice(0, 20)) {
		if (!isObj(n)) continue;
		const id = idOf(n.id);
		const text = textOrNull(n.text, 200);
		const fx = finiteOrNull(n.fx);
		const fy = finiteOrNull(n.fy);
		if (!id || seen.has(id) || !text || fx === null || fy === null) continue;
		seen.add(id);
		out.push({ id, text, fx: Math.min(1, Math.max(0, fx)), fy: Math.min(1, Math.max(0, fy)) });
	}
	return out;
}

/** A plot's display options, field by field; what is not one keeps its default. */
export function readDisplay(v: unknown): PlotDisplay {
	const d = defaultDisplay();
	if (!isObj(v)) return d;
	const traces: Record<string, TraceStyle> = {};
	if (isObj(v.traces)) {
		for (const [name, t] of Object.entries(v.traces).slice(0, 64)) {
			const style = name.length <= 120 ? readTrace(t) : null;
			if (style) traces[name] = style;
		}
	}
	return {
		x: readAxis(v.x),
		y: readAxis(v.y),
		grid: oneOf(v.grid, ['major', 'minor', 'none'], d.grid),
		legend: oneOf(v.legend, ['inside', 'outside', 'off'], d.legend),
		legendCorner: oneOf(v.legendCorner, ['top-left', 'top-right', 'bottom-left', 'bottom-right'], d.legendCorner),
		readout: typeof v.readout === 'boolean' ? v.readout : d.readout,
		hidden: Array.isArray(v.hidden) ? [...new Set(v.hidden.filter((f): f is LineFamily => typeof f === 'string' && (FAMILIES as string[]).includes(f)))] : [],
		traces,
		cursor: oneOf(v.cursor, ['none', 'one', 'two', 'peak', 'peaks', 'harmonics'], d.cursor),
		count: wholeIn(v.count, 1, 20, d.count),
		hold: oneOf(v.hold, ['none', 'peak', 'min', 'average', 'exponential'], d.hold),
		average: wholeIn(v.average, 1, 1000, d.average),
		timeRender: oneOf(v.timeRender, ['waveform', 'envelope'], d.timeRender),
		lanes: oneOf(v.lanes, ['overlay', 'split'], d.lanes),
		window_s: (() => {
			const w = finiteOrNull(v.window_s);
			return w !== null && w > 0 && w <= 3600 ? w : null;
		})(),
		notes: readNotes(v.notes)
	};
}

/** A plot panel's choice, field by field; null when its plot type is not one the test offers. */
export function readSpec(v: unknown, types: PlotType[]): PlotPanelSpec | null {
	if (!isObj(v)) return null;
	const type = types.find((t) => t.id === v.quantity);
	if (!type) return null;
	const base = newPlotSpec(type);
	return {
		quantity: type.id,
		channels: type.channels ? readSelector(v.channels) : 'all',
		arrange: oneOf(v.arrange, ['split', 'overlay'], base.arrange),
		points: wholeIn(v.points, 64, 8192, base.points),
		title: textOrNull(v.title, 80),
		display: readDisplay(v.display)
	};
}

function readCell(v: unknown): Cell | null {
	if (!isObj(v)) return null;
	const n = (k: string) => (typeof v[k] === 'number' && Number.isFinite(v[k]) ? (v[k] as number) : null);
	const [x, y, w, h] = [n('x'), n('y'), n('w'), n('h')];
	return x === null || y === null || w === null || h === null ? null : { x, y, w, h };
}

/** What a test offers: the plot types it draws and the cards it has. */
export interface Offer {
	types: PlotType[];
	cards: CardKind[];
}

function readDock(v: unknown, offer: Offer, placed: Set<string>): Dock {
	const d = defaultDock(offer.cards);
	if (!isObj(v)) return { ...d, cards: d.cards.map((c) => ({ ...c, shown: !placed.has(c.kind) })) };
	const side = oneOf(v.side, ['right', 'bottom', 'collapsed'], d.side);
	const bounds = side === 'bottom' ? DOCK_SIZE.bottom : DOCK_SIZE.right;
	const size = finiteOrNull(v.size);
	const cards: DockCard[] = [];
	for (const c of Array.isArray(v.cards) ? v.cards : []) {
		if (!isObj(c) || typeof c.kind !== 'string' || !offer.cards.includes(c.kind) || cards.some((x) => x.kind === c.kind)) continue;
		cards.push({ kind: c.kind, open: c.open !== false, shown: c.shown !== false });
	}
	for (const kind of offer.cards) if (!cards.some((c) => c.kind === kind)) cards.push({ kind, open: true, shown: true });
	// A card on a page is not in the dock as well.
	for (const c of cards) if (placed.has(c.kind)) c.shown = false;
	return { side, size: size === null ? bounds.start : Math.round(Math.min(bounds.max, Math.max(bounds.min, size))), cards };
}

/** A layout read from storage, checked field by field; null when it is not one. */
export function readScreen(raw: unknown, offer: Offer, aspect = DEFAULT_ASPECT): ScreenLayout | null {
	if (!isObj(raw) || raw.version !== 2 || !Array.isArray(raw.pages)) return null;
	const ids = new Set<string>();
	const placed = new Set<string>();
	const pages: ScreenPage[] = [];
	for (const pv of raw.pages.slice(0, MAX_PAGES)) {
		if (!isObj(pv)) continue;
		const id = idOf(pv.id);
		const name = typeof pv.name === 'string' ? cleanName(pv.name) : null;
		if (!id || ids.has(id) || !name) continue;
		ids.add(id);
		const cols = (GRID_SIZES as readonly number[]).includes(pv.cols as number) ? (pv.cols as number) : GRID_COLS;
		const rows = (GRID_SIZES as readonly number[]).includes(pv.rows as number) ? (pv.rows as number) : GRID_ROWS;
		let panels: Panel[] = [];
		let plots = 0;
		for (const xv of Array.isArray(pv.panels) ? pv.panels.slice(0, MAX_PANELS) : []) {
			if (!isObj(xv)) continue;
			const pid = idOf(xv.id);
			if (!pid || ids.has(pid)) continue;
			const cell = readCell(xv.at);
			const at = clampCell(cell ?? { x: 0, y: 0, w: cols, h: rows }, cols, rows);
			if (xv.kind === 'plot') {
				const plot = plots < MAX_PLOTS ? readSpec(xv.plot, offer.types) : null;
				if (!plot) continue;
				plots += 1;
				panels.push({ id: pid, kind: 'plot', at, plot });
			} else if (xv.kind === 'card') {
				if (typeof xv.card !== 'string' || !offer.cards.includes(xv.card) || placed.has(xv.card)) continue;
				placed.add(xv.card);
				panels.push({ id: pid, kind: 'card', at, card: xv.card });
			} else continue;
			ids.add(pid);
		}
		if (!fits(panels, cols, rows)) panels = arrangeEvenly(panels, cols, rows, aspect);
		pages.push({ id, name, cols, rows, panels, linkTime: pv.linkTime === true });
	}
	if (!pages.length) return null;
	const rate = typeof raw.rate === 'number' && RATES.includes(raw.rate) ? raw.rate : DEFAULT_RATE;
	const serial = Math.max(highest(ids), Number.isInteger(raw.serial) && (raw.serial as number) >= 0 ? (raw.serial as number) : 0);
	return { version: 2, pages, dock: readDock(raw.dock, offer, placed), rate, serial };
}

export function fromSaved(text: string | null, offer: Offer, aspect = DEFAULT_ASPECT): ScreenLayout | null {
	if (!text) return null;
	try {
		return readScreen(JSON.parse(text), offer, aspect);
	} catch {
		return null;
	}
}

// ---- writing one ------------------------------------------------------------------------------

function savedAxis(a: AxisDisplay) {
	return { scale: a.scale, range: a.range, min: a.min, max: a.max, label: a.label, unit: a.unit, ticks: a.ticks };
}

function savedDisplay(d: PlotDisplay) {
	return {
		x: savedAxis(d.x),
		y: savedAxis(d.y),
		grid: d.grid,
		legend: d.legend,
		legendCorner: d.legendCorner,
		readout: d.readout,
		hidden: [...d.hidden],
		traces: Object.fromEntries(
			Object.keys(d.traces)
				.sort()
				.map((k) => {
					const t = d.traces[k];
					return [k, { visible: t.visible, width: t.width, dash: t.dash, color: t.color }];
				})
		),
		cursor: d.cursor,
		count: d.count,
		hold: d.hold,
		average: d.average,
		timeRender: d.timeRender,
		lanes: d.lanes,
		window_s: d.window_s,
		notes: d.notes.map((n) => ({ id: n.id, text: n.text, fx: n.fx, fy: n.fy }))
	};
}

function savedSpec(s: PlotPanelSpec) {
	return { quantity: s.quantity, channels: Array.isArray(s.channels) ? [...s.channels] : s.channels, arrange: s.arrange, points: s.points, title: s.title, display: savedDisplay(s.display) };
}

function savedPanel(p: Panel) {
	const at = { x: p.at.x, y: p.at.y, w: p.at.w, h: p.at.h };
	return p.kind === 'plot' ? { id: p.id, kind: p.kind, at, plot: savedSpec(p.plot) } : { id: p.id, kind: p.kind, at, card: p.card };
}

/**
 * The shape a layout is kept in (storage now, the `.hforge` setup later). The fields are written
 * in one order, whatever order they were set in, so two equal layouts always save the same text.
 */
export function toSaved(layout: ScreenLayout): string {
	return JSON.stringify({
		version: 2,
		pages: layout.pages.map((p) => ({ id: p.id, name: p.name, cols: p.cols, rows: p.rows, linkTime: p.linkTime, panels: p.panels.map(savedPanel) })),
		dock: { side: layout.dock.side, size: layout.dock.size, cards: layout.dock.cards.map((c) => ({ kind: c.kind, open: c.open, shown: c.shown })) },
		rate: layout.rate,
		serial: layout.serial
	});
}

// ---- where it is kept -------------------------------------------------------------------------

function titleKey(setupTitle: string | null | undefined): string {
	const t = (setupTitle ?? '').replace(/\s+/g, ' ').trim().toLowerCase();
	return t || 'untitled';
}

/** Where a layout is kept: per test type and per setup (by its title). */
export function screenKey(testType: string, setupTitle: string | null | undefined): string {
	return `hertzforge.screen.v2:${testType}:${titleKey(setupTitle)}`;
}

/** The test type's most recent layout, the starting point for a setup that has none of its own. */
export function lastScreenKey(testType: string): string {
	return `hertzforge.screen.v2:${testType}:*last`;
}

// ---- Random's layouts from before the page grid -----------------------------------------------

/** A Random plot as it was kept before, as a plot panel's choice. */
export function specFromV1(p: PlotSpecV1, types: PlotType[] = RANDOM_TYPES): PlotPanelSpec | null {
	const type = types.find((t) => t.id === p.quantity);
	if (!type) return null;
	const spec = newPlotSpec(type, p.channels);
	spec.arrange = p.arrange;
	spec.points = p.points;
	spec.display.x.scale = p.x;
	spec.display.y.scale = p.y;
	if (p.yMin !== null && p.yMax !== null && p.yMax > p.yMin) spec.display.y = { ...spec.display.y, range: 'fixed', min: p.yMin, max: p.yMax };
	return spec;
}

/** A Random layout as it was kept before: the same pages, their plots tiled evenly. */
export function fromV1(v1: LayoutV1, offer: Offer = { types: RANDOM_TYPES, cards: RANDOM_CARDS }, aspect = DEFAULT_ASPECT): ScreenLayout {
	const pages = v1.pages.map((p) =>
		plotPage(
			p.id,
			p.name,
			p.plots.flatMap((x) => {
				const plot = specFromV1(x, offer.types);
				return plot ? [{ id: x.id, plot }] : [];
			}),
			aspect
		)
	);
	return { version: 2, pages, dock: defaultDock(offer.cards), rate: v1.rate, serial: v1.serial };
}

/** Where Random's layouts were kept before, and how one is taken over. */
export const RANDOM_LEGACY = {
	key: (setupTitle: string | null | undefined) => layoutKeyV1('random', setupTitle),
	last: lastLayoutKeyV1('random'),
	read: (text: string | null): ScreenLayout | null => {
		const v1 = fromSavedV1(text);
		return v1 ? fromV1(v1) : null;
	}
};
