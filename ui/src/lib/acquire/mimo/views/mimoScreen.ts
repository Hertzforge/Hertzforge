/**
 * MIMO Random on the test screen's frame: its quantities as plot types, its cards, the pages it
 * opens with, what a window asks the service for, and how a layout kept by the MIMO run views'
 * pages is taken over.
 *
 * A plot keeps its own choice of what it shows in its `select`: one control or every one, a pair,
 * a window of controls for the grid, the heatmap's mode and order, a diagnostic by its key. A line
 * plot is one subscription under its panel's id; a plot drawn by its own body may be several,
 * under the panel's id and a part (`pl-2:sdm`, `pl-2:psd:4`, `pl-2:open`). A window asks for the
 * plots on the page it shows only, each at its drawn width, and nothing for a page it does not show.
 */

import { MIMO_TOPICS, type MimoQuantity, type MimoSubscribePlot } from '$lib/protocol/mimo';
import type { HeatMode } from '$lib/viz/heatmap';
import { cellKind, defaultWindow, GRID_MAX, windowControls, type CellKind, type GridWindowSpec, type OffDiagonal } from '$lib/viz/matrixGrid';
import { lastLayoutKey, layoutKey } from '../../display/layout';
import {
	countsDrives,
	defaultSelector,
	MIMO_QUANTITY_WORDS,
	mimoLayoutFromSaved,
	mimoPlotTitle,
	mimoSubscriptionFor,
	newMimoPlot,
	pointsForWidth,
	readSelectorSpec,
	SELECTOR_KINDS,
	type MimoLayout,
	type MimoPageKind,
	type MimoPlotSpec,
	type SelectorKind,
	type SelectorSpec
} from '../../display/mimoLayout';
import { newPlotSpec, type LineFamily, type PlotPanelSpec, type PlotSelect, type PlotType } from '../../plots/types';
import { DEFAULT_ASPECT, DEFAULT_RATE, defaultDock, plotPage, RATES } from '../../screen/layout';
import type { ScreenHooks } from '../../screen/screen.svelte';
import { signalsHooks } from '../../signals/signals';
import type { CardKind, Panel, ScreenLayout } from '../../screen/types';

export const MIMO_TEST_TYPE = 'mimo_random';

/** The geometry is a plot type of its own: the sensors and drives in three dimensions. */
export const GEOMETRY = 'geometry';

/**
 * The coherence error per control, when the service offers it: the heatmap's second mode. The
 * mode is offered when the service lists the quantity.
 */
export const COH_ERROR = 'coh_error';

/** The most plots one subscription carries (the service's own bound). */
export const MAX_SUBSCRIBED = 64;

type PlotId = MimoQuantity | typeof GEOMETRY;

interface Offer {
	help: string;
	group: PlotType['group'];
	families?: LineFamily[];
	own?: boolean;
}

/** What MIMO Random offers to plot, in the order `+ Plot` lists it within each group. */
const OFFER: [PlotId, Offer][] = [
	['psd', { help: "One control's spectrum against its reference and its alarm and abort lines, or every control's.", group: 'frequency', families: ['measured', 'reference', 'tolerance', 'abort'] }],
	['trace', { help: "The sum of the control spectra against the reference's.", group: 'frequency', families: ['measured', 'reference'] }],
	['coh', { help: "The coherence of two controls against the reference's, 0 to 1.", group: 'frequency', families: ['measured', 'reference'] }],
	['phase', { help: "The phase between two controls against the reference's, in degrees.", group: 'frequency', families: ['measured', 'reference'] }],
	['csd_mag', { help: "The cross-spectrum magnitude of two controls against the reference's.", group: 'frequency', families: ['measured', 'reference'] }],
	['mcoh', { help: "A control's multiple coherence with the drives, or every control's.", group: 'frequency', families: ['measured'] }],
	['drive_psd', { help: "One drive's spectrum, or every drive's.", group: 'frequency', families: ['drive'] }],
	['drive_coh', { help: 'The coherence of two drives.', group: 'frequency', families: ['measured'] }],
	['frf', { help: 'The response at a control to one drive: magnitude and phase.', group: 'frequency', families: ['measured'] }],
	['sv', { help: "The plant's singular values at each frequency.", group: 'frequency', families: ['measured'] }],
	['cond', { help: "The plant's condition number at each frequency, against the identification gate.", group: 'frequency', families: ['measured', 'limit'] }],
	['pred_error_db', { help: "A control's predicted error at each frequency, in dB.", group: 'frequency', families: ['measured'] }],
	['law_diag', { help: 'A diagnostic the control law declares, chosen by name.', group: 'frequency', families: ['measured'] }],
	['error_db', { help: "Every control's error against its reference across frequency; a row opens its control in the grid.", group: 'other', own: true }],
	['sdm', { help: 'Up to eight controls at once: their spectra on the diagonal, coherence and phase or the cross spectrum off it.', group: 'other', own: true }],
	['pred_sdm', { help: 'What the controls can reach when they outnumber the drives, as a grid.', group: 'other', own: true }],
	[GEOMETRY, { help: 'The sensors and drives in three dimensions, each sensor shaded by its error.', group: 'other', own: true }],
	['readings', { help: 'The run in numbers: loop, level, largest error, gates and time.', group: 'numbers' }],
	['dof_rms', { help: "Each controlled quantity's rms over the band.", group: 'numbers' }],
	['cap', { help: "Each drive's cap factor this loop.", group: 'numbers' }],
	['pred_drive_rms', { help: "Each drive's predicted rms against its ceiling.", group: 'numbers' }],
	['pred_drive_accel', { help: "Each drive's predicted acceleration at its shaker.", group: 'numbers' }]
];

/** MIMO Random's plot types: every quantity it draws, and the geometry. */
export const MIMO_TYPES: PlotType[] = OFFER.map(([id, o]) => {
	const t: PlotType = { id, words: id === GEOMETRY ? 'Geometry' : MIMO_QUANTITY_WORDS[id], help: o.help, group: o.group, channels: false, time: false };
	const kinds = id === GEOMETRY ? [] : SELECTOR_KINDS[id].filter((k): k is Exclude<SelectorKind, 'none'> => k !== 'none');
	if (kinds.length) t.selectors = kinds;
	if (o.families) t.families = [...o.families];
	if (o.own) t.own = true;
	return t;
});

/** The cards MIMO Random offers, in the dock's first order. */
export const MIMO_CARDS: CardKind[] = ['run', 'level', 'drives', 'prediction', 'station'];

const QUANTITIES = new Set<string>(OFFER.map(([id]) => id).filter((id) => id !== GEOMETRY));
const isQuantity = (q: string): q is MimoQuantity => QUANTITIES.has(q);

export function mimoType(id: string): PlotType | null {
	return MIMO_TYPES.find((t) => t.id === id) ?? null;
}

// ---- a plot's choice ---------------------------------------------------------------------------

const isPos = (v: unknown): v is number => Number.isInteger(v) && (v as number) >= 0;
const obj = (v: unknown): Record<string, unknown> => (typeof v === 'object' && v !== null && !Array.isArray(v) ? (v as Record<string, unknown>) : {});

/** The selector a new plot of `quantity` starts with. */
export function defaultMimoSelector(quantity: MimoQuantity): SelectorSpec {
	// the response at the first control to the first drive
	if (quantity === 'frf') return { kind: 'pair', pair: [0, 0] };
	// no diagnostic until one is chosen: a law declares its own
	if (quantity === 'law_diag') return { kind: 'key', key: '', index: null };
	return defaultSelector(SELECTOR_KINDS[quantity][0]);
}

/** What a plot of `quantity` selects: its own choice when the quantity takes it, else its first. */
export function selectorOf(quantity: MimoQuantity, select?: PlotSelect): SelectorSpec {
	if (quantity === 'law_diag') {
		const d = diagChoice(select);
		return { kind: 'key', key: d.key ?? '', index: d.index };
	}
	const read = select ? readSelectorSpec(select) : null;
	return read && SELECTOR_KINDS[quantity].includes(read.kind) ? read : defaultMimoSelector(quantity);
}

export interface GridChoice {
	window: GridWindowSpec;
	off: OffDiagonal;
	/**
	 * The cell open full size, by the controls' positions (row, column) and what it showed (above or
	 * below the diagonal as drawn, which pinning can reorder), or none.
	 */
	open: { row: number; col: number; kind: CellKind } | null;
}

const CELL_KINDS: CellKind[] = ['psd', 'coh', 'phase', 'csd'];

export function gridChoice(select?: PlotSelect): GridChoice {
	const s = obj(select);
	const w = obj(s.window);
	const window: GridWindowSpec = {
		pinned: Array.isArray(w.pinned) ? [...new Set(w.pinned.filter((x): x is string => typeof x === 'string'))].slice(0, GRID_MAX) : [],
		offset: isPos(w.offset) ? w.offset : 0,
		size: isPos(w.size) && w.size >= 1 && w.size <= GRID_MAX ? w.size : GRID_MAX
	};
	const off: OffDiagonal = s.off === 'csd' ? 'csd' : 'coh_phase';
	const o = obj(s.open);
	return {
		window: { ...defaultWindow(), ...window },
		off,
		open: isPos(o.row) && isPos(o.col) ? { row: o.row, col: o.col, kind: CELL_KINDS.includes(o.kind as CellKind) ? (o.kind as CellKind) : cellKind(o.row, o.col, off) } : null
	};
}

export interface HeatmapChoice {
	mode: HeatMode;
	order: 'setup' | 'worst';
	/** The controls shown, by name, or all of them. */
	rows: 'all' | string[];
}

export function heatmapChoice(select?: PlotSelect): HeatmapChoice {
	const s = obj(select);
	return {
		mode: s.mode === 'coherence' ? 'coherence' : 'error',
		order: s.order === 'worst' ? 'worst' : 'setup',
		rows: Array.isArray(s.rows) && s.rows.some((x) => typeof x === 'string') ? s.rows.filter((x): x is string => typeof x === 'string') : 'all'
	};
}

export function geometryChoice(select?: PlotSelect): { motion: boolean } {
	return { motion: obj(select).motion !== false };
}

/** The law diagnostic a plot shows, by key, with its position for a per-control or per-drive one. */
export function diagChoice(select?: PlotSelect): { key: string | null; index: number | null } {
	const s = obj(select);
	return { key: typeof s.key === 'string' && s.key ? s.key : null, index: isPos(s.index) ? s.index : null };
}

/** The controls a heatmap shows, by position. */
export function heatmapRows(h: HeatmapChoice, labels: string[]): 'all' | number[] {
	if (h.rows === 'all') return 'all';
	const pos = h.rows.map((n) => labels.indexOf(n)).filter((i) => i >= 0);
	return pos.length ? pos : 'all';
}

/** Whether the heatmap's coherence mode can be offered: the service lists the quantity. */
export function coherenceModeOffered(quantities: readonly string[] | null): boolean {
	return !!quantities && quantities.includes(COH_ERROR);
}

/** Plots whose y axis has fixed ends of its own: coherence from 0 to 1, phase around the circle. */
const Y_RANGE: Partial<Record<string, [number, number]>> = {
	coh: [0, 1.05],
	drive_coh: [0, 1.05],
	mcoh: [0, 1.05],
	phase: [-180, 180]
};

/** A new plot of `type` with MIMO's own defaults: its first selector and its axis. */
export function mimoSpec(type: PlotType, select?: PlotSelect): PlotPanelSpec {
	const spec = newPlotSpec(type);
	if (select) spec.select = { ...select };
	else if (isQuantity(type.id) && !type.own && SELECTOR_KINDS[type.id][0] !== 'none') spec.select = { ...defaultMimoSelector(type.id) };
	const y = Y_RANGE[type.id];
	if (y) spec.display.y = { ...spec.display.y, range: 'fixed', min: y[0], max: y[1] };
	return spec;
}

// ---- titles ------------------------------------------------------------------------------------

/** What the titles read: the run's names for its controls and drives, and what the service offers. */
export interface MimoNames {
	labels: string[];
	drive_labels: string[];
	quantities: readonly string[] | null;
}

/** A plot's title: its own, else what it shows with the names of what it selects. */
export function mimoTitle(spec: PlotPanelSpec, names: MimoNames): string {
	if (spec.title) return spec.title;
	const q = spec.quantity;
	if (q === GEOMETRY) return 'Geometry';
	if (!isQuantity(q)) return q;
	if (q === 'error_db') return heatmapChoice(spec.select).mode === 'coherence' && coherenceModeOffered(names.quantities) ? MIMO_QUANTITY_WORDS.coh_error : MIMO_QUANTITY_WORDS.error_db;
	if (q === 'sdm' || q === 'pred_sdm') return MIMO_QUANTITY_WORDS[q];
	if (q === 'law_diag') {
		const d = diagChoice(spec.select);
		return d.key ? `${MIMO_QUANTITY_WORDS.law_diag}: ${d.key}` : MIMO_QUANTITY_WORDS.law_diag;
	}
	return mimoPlotTitle(newMimoPlot('', q, selectorOf(q, spec.select)), names.labels, names.drive_labels);
}

/** The name of a control or drive by its position, as the run names it. */
export function positionName(quantity: string, i: number, names: Pick<MimoNames, 'labels' | 'drive_labels'>): string {
	const drives = isQuantity(quantity) && countsDrives(quantity);
	return (drives ? names.drive_labels : names.labels)[i] ?? `${drives ? 'Drive' : 'Control'} ${i + 1}`;
}

// ---- what a window asks for ---------------------------------------------------------------------

/** What a window's plots depend on besides their choices. */
export interface MimoContext extends MimoNames {
	space: 'physical' | 'virtual' | null;
	/** Each panel's drawn widths in CSS pixels, by part (`main`, `cell`, `full`). */
	widths: Record<string, Record<string, number>>;
	pixelRatio?: number;
}

/** The id of one part of a plot drawn in several (`pl-2:sdm`). */
export const partId = (panelId: string, part: string) => `${panelId}:${part}`;

/** The quantity a grid cell opens full size as, with its selector. */
export function openCellPlot(id: string, open: { row: number; col: number; kind: CellKind }, points: number): MimoPlotSpec {
	const { row, col, kind } = open;
	const p = (q: MimoQuantity, s: SelectorSpec) => ({ ...newMimoPlot(id, q, s), points });
	if (kind === 'psd') return p('psd', { kind: 'index', index: row });
	if (kind === 'coh') return p('coh', { kind: 'pair', pair: [row, col] });
	if (kind === 'phase') return p('phase', { kind: 'pair', pair: [row, col] });
	return p('csd_mag', { kind: 'pair', pair: [row, col] });
}

/** Everything one plot panel asks for now. */
export function panelPlots(panel: { id: string; plot: PlotPanelSpec }, ctx: MimoContext): MimoPlotSpec[] {
	const { id, plot } = panel;
	const q = plot.quantity;
	const pts = (part: string, fallback: number) => pointsForWidth(ctx.widths[id]?.[part] ?? fallback, ctx.pixelRatio ?? 1);
	if (q === GEOMETRY) return ctx.space === 'virtual' ? [{ ...newMimoPlot(id, 'dof_rms'), points: 64 }] : [];
	if (!isQuantity(q)) return [];
	switch (q) {
		case 'sdm':
		case 'pred_sdm': {
			if (!ctx.labels.length) return [];
			const g = gridChoice(plot.select);
			const window = windowControls(ctx.labels, g.window);
			const cell = pts('cell', 120);
			const out: MimoPlotSpec[] = [{ ...newMimoPlot(partId(id, 'sdm'), q, { kind: 'block', block: window }), points: cell }];
			if (q === 'pred_sdm') return out;
			for (const i of window) out.push({ ...newMimoPlot(partId(id, `psd:${i}`), 'psd', { kind: 'index', index: i }), points: cell });
			if (g.open && g.open.row < ctx.labels.length && g.open.col < ctx.labels.length) out.push(openCellPlot(partId(id, 'open'), g.open, pts('full', 1024)));
			return out;
		}
		case 'error_db': {
			const h = heatmapChoice(plot.select);
			const quantity = (h.mode === 'coherence' && coherenceModeOffered(ctx.quantities) ? COH_ERROR : 'error_db') as MimoQuantity;
			return [{ ...newMimoPlot(id, quantity, { kind: 'rows', rows: heatmapRows(h, ctx.labels) }), points: pts('main', 1024) }];
		}
		default: {
			const numbers = mimoType(q)?.group === 'numbers';
			return [{ ...newMimoPlot(id, q, selectorOf(q, plot.select)), points: numbers ? 64 : pts('main', 1024) }];
		}
	}
}

/**
 * What a window sends for the plots it shows, or the default subscription when it shows none. A
 * plot whose choice cannot go with its quantity (a diagnostic not yet chosen) is left out, so one
 * plot never costs the others their data.
 */
export function mimoParams(panels: { id: string; plot: PlotPanelSpec }[], rate: number, ctx: MimoContext): Record<string, unknown> {
	const plots: MimoSubscribePlot[] = mimoSubscriptionFor(panels.flatMap((p) => panelPlots(p, ctx))).slice(0, MAX_SUBSCRIBED);
	return plots.length ? { rate_hz: rate, plots } : { default: true };
}

// ---- the first pages ---------------------------------------------------------------------------

type Planned = { quantity: PlotId; select?: PlotSelect };

function planned(kind: MimoPageKind, settings: Record<string, unknown> = {}): Planned[] {
	const s = settings;
	const pair = (v: unknown, d: [number, number]): [number, number] => (Array.isArray(v) && v.length === 2 && v.every(isPos) ? [v[0], v[1]] : d);
	const keep = (keys: string[]) => Object.fromEntries(keys.filter((k) => s[k] !== undefined).map((k) => [k, s[k]]));
	switch (kind) {
		case 'heatmap':
			return [{ quantity: 'error_db', select: keep(['mode', 'order', 'rows']) }];
		case 'grid':
			return [{ quantity: 'sdm', select: keep(['window', 'off', 'open']) }];
		case 'geometry':
			return [{ quantity: GEOMETRY, select: keep(['motion']) }];
		case 'plant': {
			const out: Planned[] = [
				{ quantity: 'drive_psd', select: { kind: 'index', index: isPos(s.drive) ? s.drive : 0 } },
				{ quantity: 'drive_coh', select: { kind: 'pair', pair: pair(s.drivePair, [0, 1]) } },
				{ quantity: 'frf', select: { kind: 'pair', pair: pair(s.frf, [0, 0]) } },
				{ quantity: 'sv' },
				{ quantity: 'cond' },
				{ quantity: 'cap', select: { kind: 'diagonal' } }
			];
			if (typeof s.diag === 'string' && s.diag) out.push({ quantity: 'law_diag', select: { kind: 'key', key: s.diag, index: isPos(s.diagIndex) ? s.diagIndex : null } });
			return out;
		}
	}
}

const KIND_WORDS: Record<MimoPageKind, string> = { heatmap: 'Error Heatmap', grid: 'Matrix Grid', plant: 'Drive and Plant', geometry: 'Geometry' };

/** A layout of these pages, each of its kind's plots tiled evenly, with the dock's first cards. */
export function mimoScreenLayout(pages: readonly { name: string; kind: MimoPageKind; settings?: Record<string, unknown> }[], rate = DEFAULT_RATE, aspect = DEFAULT_ASPECT): ScreenLayout {
	let n = 0;
	const out = pages.map((p, i) =>
		plotPage(
			`pg-${i + 1}`,
			p.name || KIND_WORDS[p.kind],
			planned(p.kind, p.settings).map((x) => ({ id: `pl-${++n}`, plot: mimoSpec(mimoType(x.quantity) as PlotType, x.select && Object.keys(x.select).length ? x.select : undefined) })),
			aspect
		)
	);
	return { version: 2, pages: out, dock: defaultDock(MIMO_CARDS), rate: RATES.includes(rate) ? rate : DEFAULT_RATE, serial: Math.max(n, out.length) };
}

/**
 * MIMO Random's pages until the operator changes them: the error heatmap first (every control at
 * once), the matrix grid, the drives and the plant, and the geometry.
 */
export function defaultMimoScreen(aspect = DEFAULT_ASPECT): ScreenLayout {
	return mimoScreenLayout(
		(['heatmap', 'grid', 'plant', 'geometry'] as MimoPageKind[]).map((kind) => ({ name: KIND_WORDS[kind], kind })),
		DEFAULT_RATE,
		aspect
	);
}

/** A layout the MIMO run views kept (pages of fixed kinds), taken over: the same pages, names, choices and rate. */
export function mimoFromLegacy(v1: MimoLayout, aspect = DEFAULT_ASPECT): ScreenLayout {
	return mimoScreenLayout(v1.pages, v1.rate, aspect);
}

/** Where the MIMO run views kept their layouts, and how one is taken over. */
export const MIMO_LEGACY = {
	key: (setupTitle: string | null) => layoutKey(MIMO_TEST_TYPE, setupTitle),
	last: lastLayoutKey(MIMO_TEST_TYPE),
	read: (text: string | null): ScreenLayout | null => {
		const v1 = mimoLayoutFromSaved(text);
		return v1 ? mimoFromLegacy(v1) : null;
	}
};

/** Everything MIMO Random gives its screen; `ctx` is read whenever the subscription is made. */
export function mimoHooks(ctx: () => MimoContext): ScreenHooks {
	// MIMO also offers Signals: any channel against time, on its own feed.
	return signalsHooks({
		testType: MIMO_TEST_TYPE,
		types: MIMO_TYPES,
		cards: MIMO_CARDS,
		defaultLayout: defaultMimoScreen,
		legacy: MIMO_LEGACY,
		feed: { method: 'mimo_random.subscribe', topic: MIMO_TOPICS.data, params: (plots, rate) => mimoParams(plots, rate, ctx()) }
	});
}

/** The first plot of `quantity` on any page, with its page, or null. */
export function findPlot(layout: ScreenLayout, quantity: string): { pageId: string; panel: Extract<Panel, { kind: 'plot' }> } | null {
	for (const page of layout.pages) {
		const panel = page.panels.find((p): p is Extract<Panel, { kind: 'plot' }> => p.kind === 'plot' && p.plot.quantity === quantity);
		if (panel) return { pageId: page.id, panel };
	}
	return null;
}
