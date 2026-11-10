/**
 * What a MIMO Random plot shows: a quantity and its selector (PROTOCOL.md, "MIMO Random
 * Display"), the subscription a window sends for its plots, and the plain-data form a MIMO layout
 * is saved in. Nothing here draws or computes a measurement.
 *
 * A selector picks out part of a quantity: one control or drive (`index`), two of them (`pair`),
 * the heatmap's rows (`rows`), every one (`diagonal`), a block of the spectral matrix for the
 * grid (`block`), or a law diagnostic (`key`). Positions count the setup's controlled quantities
 * and drives from 0, in order; the run's status names them.
 */

import type { MimoQuantity, MimoSelector, MimoSubscribePlot } from '$lib/protocol/mimo';
import type { ScaleChoice } from './layout';

export type SelectorKind = 'none' | 'index' | 'pair' | 'rows' | 'diagonal' | 'block' | 'key';

/** A selector as a layout keeps it. */
export type SelectorSpec =
	| { kind: 'none' }
	| { kind: 'index'; index: number }
	| { kind: 'pair'; pair: [number, number] }
	| { kind: 'rows'; rows: 'all' | number[] }
	| { kind: 'diagonal' }
	| { kind: 'block'; block: number[] }
	| { kind: 'key'; key: string; index: number | null };

export interface MimoPlotSpec {
	/** Stable within the layout; the subscription's plot id. */
	id: string;
	quantity: MimoQuantity;
	selector: SelectorSpec;
	x: ScaleChoice;
	y: ScaleChoice;
	yMin: number | null;
	yMax: number | null;
	/** The plot's width in points: the service decimates to it. */
	points: number;
}

/** The largest block of the spectral matrix one plot shows (the grid's 8 by 8 window). */
export const MAX_BLOCK = 8;
export const MIN_POINTS = 64;
export const MAX_POINTS = 8192;
export const DEFAULT_POINTS = 1024;

/** Every MIMO quantity, in the order a chooser offers them. */
export const MIMO_QUANTITY_ORDER: MimoQuantity[] = [
	'psd',
	'coh',
	'phase',
	'csd_mag',
	'error_db',
	'coh_error',
	'sdm',
	'trace',
	'drive_psd',
	'drive_coh',
	'cap',
	'frf',
	'sv',
	'cond',
	'mcoh',
	'dof_rms',
	'pred_drive_rms',
	'pred_error_db',
	'pred_sdm',
	'pred_drive_accel',
	'law_diag',
	'readings'
];

export const MIMO_QUANTITY_WORDS: Record<MimoQuantity, string> = {
	psd: 'Control Spectrum',
	coh: 'Coherence',
	phase: 'Phase',
	csd_mag: 'Cross Spectrum',
	error_db: 'Error Heatmap',
	coh_error: 'Coherence Error Heatmap',
	sdm: 'Matrix Grid',
	trace: 'Overall Level',
	drive_psd: 'Drive Spectrum',
	drive_coh: 'Drive Coherence',
	cap: 'Drive Caps',
	frf: 'Frequency Response',
	sv: 'Singular Values',
	cond: 'Condition Number',
	mcoh: 'Multiple Coherence',
	dof_rms: 'Degree-of-Freedom RMS',
	pred_drive_rms: 'Predicted Drive',
	pred_error_db: 'Predicted Error',
	pred_sdm: 'Achievable Reference',
	pred_drive_accel: 'Predicted Shaker Acceleration',
	law_diag: 'Law Diagnostic',
	readings: 'Run Numbers'
};

/**
 * The selectors each quantity takes; the first is the one a new plot starts with. The service
 * refuses any other.
 */
export const SELECTOR_KINDS: Record<MimoQuantity, SelectorKind[]> = {
	psd: ['index', 'diagonal'],
	coh: ['pair'],
	phase: ['pair'],
	csd_mag: ['pair'],
	error_db: ['rows'],
	coh_error: ['rows'],
	sdm: ['block'],
	trace: ['none'],
	drive_psd: ['index', 'diagonal'],
	drive_coh: ['pair'],
	cap: ['diagonal', 'index'],
	frf: ['pair'],
	sv: ['none'],
	cond: ['none'],
	mcoh: ['index', 'diagonal'],
	dof_rms: ['none'],
	pred_drive_rms: ['diagonal', 'index'],
	pred_error_db: ['index'],
	pred_sdm: ['block'],
	pred_drive_accel: ['diagonal', 'index'],
	law_diag: ['key'],
	readings: ['none']
};

/** Whether a quantity's positions count drives (otherwise controls). */
export function countsDrives(q: MimoQuantity): boolean {
	return q === 'drive_psd' || q === 'drive_coh' || q === 'cap' || q === 'pred_drive_rms' || q === 'pred_drive_accel';
}

/** A pair of two different channels, for the quantities that are between two. */
export function needsDistinctPair(q: MimoQuantity): boolean {
	return q === 'coh' || q === 'phase' || q === 'drive_coh';
}

/** The selector a new plot of `kind` starts with. */
export function defaultSelector(kind: SelectorKind): SelectorSpec {
	switch (kind) {
		case 'none':
			return { kind: 'none' };
		case 'index':
			return { kind: 'index', index: 0 };
		case 'pair':
			return { kind: 'pair', pair: [0, 1] };
		case 'rows':
			return { kind: 'rows', rows: 'all' };
		case 'diagonal':
			return { kind: 'diagonal' };
		case 'block':
			return { kind: 'block', block: [0, 1] };
		case 'key':
			return { kind: 'key', key: 'condition', index: null };
	}
}

/** A new plot of `quantity` with its first selector. */
export function newMimoPlot(id: string, quantity: MimoQuantity, selector?: SelectorSpec): MimoPlotSpec {
	return {
		id,
		quantity,
		selector: selector ?? defaultSelector(SELECTOR_KINDS[quantity][0]),
		x: 'auto',
		y: 'auto',
		yMin: null,
		yMax: null,
		points: DEFAULT_POINTS
	};
}

const isPos = (v: unknown): v is number => Number.isInteger(v) && (v as number) >= 0;

/**
 * Why `selector` cannot go with `quantity`, in words, or null when it can. The service checks the
 * same; asking here lets an editor say so before anything is sent.
 */
export function selectorProblem(quantity: MimoQuantity, selector: SelectorSpec): string | null {
	if (!SELECTOR_KINDS[quantity].includes(selector.kind)) {
		const kinds = SELECTOR_KINDS[quantity];
		return kinds[0] === 'none' ? `${MIMO_QUANTITY_WORDS[quantity]} shows the whole run; it takes no channel choice.` : `${MIMO_QUANTITY_WORDS[quantity]} takes ${kinds.join(' or ')}.`;
	}
	switch (selector.kind) {
		case 'index':
			return isPos(selector.index) ? null : 'Choose a channel.';
		case 'pair':
			if (!isPos(selector.pair[0]) || !isPos(selector.pair[1])) return 'Choose two channels.';
			if (needsDistinctPair(quantity) && selector.pair[0] === selector.pair[1]) return 'Choose two different channels.';
			return null;
		case 'rows':
			if (selector.rows === 'all') return null;
			return selector.rows.length > 0 && selector.rows.every(isPos) ? null : 'Choose at least one control, or all of them.';
		case 'block':
			if (selector.block.length < 1 || selector.block.length > MAX_BLOCK) return `Choose 1 to ${MAX_BLOCK} controls for the grid.`;
			if (!selector.block.every(isPos)) return 'Choose controls for the grid.';
			return new Set(selector.block).size === selector.block.length ? null : 'Choose each control once.';
		case 'key':
			return selector.key ? null : 'Choose a diagnostic.';
		default:
			return null;
	}
}

/** The selector as the service reads it. */
export function selectorWire(s: SelectorSpec): MimoSelector {
	switch (s.kind) {
		case 'none':
			return {};
		case 'diagonal':
			return { diagonal: true };
		case 'index':
			return { index: s.index };
		case 'pair':
			return { pair: [s.pair[0], s.pair[1]] };
		case 'rows':
			return { rows: s.rows === 'all' ? 'all' : [...s.rows] };
		case 'block':
			return { block: [...s.block] };
		case 'key':
			return s.index === null ? { key: s.key } : { key: s.key, index: s.index };
	}
}

/**
 * The subscription a window sends for the MIMO plots it shows. A plot whose selector cannot go
 * with its quantity is left out (its editor says why), so one bad plot never costs the others
 * their data.
 */
export function mimoSubscriptionFor(plots: MimoPlotSpec[]): MimoSubscribePlot[] {
	return plots
		.filter((p) => selectorProblem(p.quantity, p.selector) === null)
		.map((p) => ({ id: p.id, quantity: p.quantity, ...selectorWire(p.selector), points: clampPoints(p.points) }) as MimoSubscribePlot);
}

function clampPoints(n: number): number {
	if (!Number.isFinite(n)) return DEFAULT_POINTS;
	return Math.min(MAX_POINTS, Math.max(MIN_POINTS, Math.round(n)));
}

/** A plot's width in points from its width on screen in CSS pixels and the screen's pixel ratio. */
export function pointsForWidth(cssPixels: number, pixelRatio = 1): number {
	return clampPoints(cssPixels * Math.max(1, pixelRatio));
}

/** The plots of a matrix grid window: one block of up to 8 controls, from `first`. */
export function gridWindow(first: number, controls: number, size = MAX_BLOCK): number[] {
	const n = Math.max(0, Math.min(size, MAX_BLOCK, controls - first));
	return Array.from({ length: n }, (_, k) => first + k);
}

/**
 * The pairs of a window of controls above the diagonal, in row order: what a grid of coherence or
 * phase cells subscribes to when it shows cells rather than the packed block.
 */
export function pairsOf(window: number[]): [number, number][] {
	const out: [number, number][] = [];
	for (let r = 0; r < window.length; r++) for (let c = r + 1; c < window.length; c++) out.push([window[r], window[c]]);
	return out;
}

/** What a plot shows, in words: its title, with the names of what it selects. */
export function mimoPlotTitle(p: MimoPlotSpec, labels: string[] = [], driveLabels: string[] = []): string {
	const q = MIMO_QUANTITY_WORDS[p.quantity];
	const names = countsDrives(p.quantity) ? driveLabels : labels;
	const name = (i: number) => names[i] ?? `${countsDrives(p.quantity) ? 'Drive' : 'Control'} ${i + 1}`;
	const s = p.selector;
	switch (s.kind) {
		case 'index':
			return `${q}: ${name(s.index)}`;
		case 'pair':
			return p.quantity === 'frf' ? `${q}: ${labels[s.pair[0]] ?? `Control ${s.pair[0] + 1}`} from ${driveLabels[s.pair[1]] ?? `Drive ${s.pair[1] + 1}`}` : `${q}: ${name(s.pair[0])} and ${name(s.pair[1])}`;
		case 'rows':
			return s.rows === 'all' ? q : `${q}: ${s.rows.length} ${s.rows.length === 1 ? 'control' : 'controls'}`;
		case 'diagonal':
			return `${q}: all ${countsDrives(p.quantity) ? 'drives' : 'controls'}`;
		case 'block':
			return `${q}: ${s.block.map(name).join(', ')}`;
		case 'key':
			return `${q}: ${s.key}`;
		default:
			return q;
	}
}

// ---- reading a saved plot back ----------------------------------------------------------------

const QUANTITIES = new Set<string>(MIMO_QUANTITY_ORDER);
const SCALES = new Set<string>(['auto', 'log', 'linear']);

/** A selector as kept, checked field by field; null when it is not one. */
export function readSelectorSpec(v: unknown): SelectorSpec | null {
	if (typeof v !== 'object' || v === null) return null;
	const o = v as Record<string, unknown>;
	switch (o.kind) {
		case 'none':
			return { kind: 'none' };
		case 'diagonal':
			return { kind: 'diagonal' };
		case 'index':
			return isPos(o.index) ? { kind: 'index', index: o.index } : null;
		case 'pair':
			return Array.isArray(o.pair) && o.pair.length === 2 && o.pair.every(isPos) ? { kind: 'pair', pair: [o.pair[0], o.pair[1]] } : null;
		case 'rows':
			if (o.rows === 'all') return { kind: 'rows', rows: 'all' };
			return Array.isArray(o.rows) && o.rows.length > 0 && o.rows.every(isPos) ? { kind: 'rows', rows: [...new Set(o.rows as number[])] } : null;
		case 'block':
			return Array.isArray(o.block) && o.block.every(isPos) ? { kind: 'block', block: [...new Set(o.block as number[])].slice(0, MAX_BLOCK) } : null;
		case 'key':
			return typeof o.key === 'string' && o.key ? { kind: 'key', key: o.key, index: isPos(o.index) ? o.index : null } : null;
		default:
			return null;
	}
}

/** A MIMO plot read from storage (or a saved setup), checked field by field; null when it is not one. */
export function readMimoPlot(v: unknown): MimoPlotSpec | null {
	if (typeof v !== 'object' || v === null) return null;
	const o = v as Record<string, unknown>;
	if (typeof o.id !== 'string' || !o.id || o.id.length > 64) return null;
	if (typeof o.quantity !== 'string' || !QUANTITIES.has(o.quantity)) return null;
	const quantity = o.quantity as MimoQuantity;
	const read = readSelectorSpec(o.selector);
	const selector = read && SELECTOR_KINDS[quantity].includes(read.kind) ? read : defaultSelector(SELECTOR_KINDS[quantity][0]);
	return {
		id: o.id,
		quantity,
		selector,
		x: typeof o.x === 'string' && SCALES.has(o.x) ? (o.x as ScaleChoice) : 'auto',
		y: typeof o.y === 'string' && SCALES.has(o.y) ? (o.y as ScaleChoice) : 'auto',
		yMin: typeof o.yMin === 'number' && Number.isFinite(o.yMin) ? o.yMin : null,
		yMax: typeof o.yMax === 'number' && Number.isFinite(o.yMax) ? o.yMax : null,
		points: typeof o.points === 'number' ? clampPoints(o.points) : DEFAULT_POINTS
	};
}

// ---- the MIMO run views' pages ----------------------------------------------------------------

/**
 * The four kinds of page the MIMO run view offers. A page of each kind draws its own view and
 * holds its own choices (the grid's window, the heatmap's mode, the pair a plant plot shows); the
 * plots it subscribes to follow from those choices and its size on screen.
 */
export type MimoPageKind = 'grid' | 'heatmap' | 'plant' | 'geometry';

export const MIMO_PAGE_KINDS: MimoPageKind[] = ['grid', 'heatmap', 'plant', 'geometry'];

export const MIMO_PAGE_WORDS: Record<MimoPageKind, string> = {
	grid: 'Matrix Grid',
	heatmap: 'Error Heatmap',
	plant: 'Drive and Plant',
	geometry: 'Geometry'
};

export interface MimoPageSpec {
	id: string;
	name: string;
	kind: MimoPageKind;
	/** The page's own choices, plain data; read through the views' settings functions. */
	settings: Record<string, unknown>;
}

export interface MimoLayout {
	version: 1;
	pages: MimoPageSpec[];
	/** Display updates per second this window asks for, 0.5 to 10. */
	rate: number;
	/** The last number given to a page; never given twice. */
	serial: number;
}

/** A default layout and the page each window opens on, first the main window. */
export interface MimoWindows {
	layout: MimoLayout;
	windows: string[];
}

export const MIMO_RATES = [1, 2, 5, 10];
export const MIMO_DEFAULT_RATE = 5;
export const MAX_MIMO_PAGES = 12;

/**
 * The MIMO Random pages until the operator changes them: the four views, one page each. On one
 * monitor the window opens on the error heatmap, the overview of every control at once. On two,
 * the main window opens on the matrix grid and the second on the heatmap, so the overview stays
 * up while the grid is worked; a third and fourth monitor take the drive and plant page and the
 * geometry.
 */
export function defaultMimoLayout(monitors = 1): MimoWindows {
	const layout: MimoLayout = {
		version: 1,
		rate: MIMO_DEFAULT_RATE,
		serial: 4,
		pages: [
			{ id: 'mp-1', name: MIMO_PAGE_WORDS.heatmap, kind: 'heatmap', settings: {} },
			{ id: 'mp-2', name: MIMO_PAGE_WORDS.grid, kind: 'grid', settings: {} },
			{ id: 'mp-3', name: MIMO_PAGE_WORDS.plant, kind: 'plant', settings: {} },
			{ id: 'mp-4', name: MIMO_PAGE_WORDS.geometry, kind: 'geometry', settings: {} }
		]
	};
	const m = Math.min(4, Math.max(1, Math.round(monitors) || 1));
	const windows = m === 1 ? ['mp-1'] : ['mp-2', 'mp-1', 'mp-3', 'mp-4'].slice(0, m);
	return { layout, windows };
}

/** A MIMO layout read from storage, checked field by field; null when it is not one. */
export function readMimoLayout(raw: unknown): MimoLayout | null {
	if (typeof raw !== 'object' || raw === null) return null;
	const o = raw as Record<string, unknown>;
	if (o.version !== 1 || !Array.isArray(o.pages)) return null;
	const ids = new Set<string>();
	const pages: MimoPageSpec[] = [];
	for (const v of o.pages.slice(0, MAX_MIMO_PAGES)) {
		if (typeof v !== 'object' || v === null) continue;
		const p = v as Record<string, unknown>;
		if (typeof p.id !== 'string' || !p.id || p.id.length > 64 || ids.has(p.id)) continue;
		if (typeof p.kind !== 'string' || !MIMO_PAGE_KINDS.includes(p.kind as MimoPageKind)) continue;
		const name = typeof p.name === 'string' ? p.name.replace(/\s+/g, ' ').trim().slice(0, 40) : '';
		if (!name) continue;
		ids.add(p.id);
		const settings = typeof p.settings === 'object' && p.settings !== null && !Array.isArray(p.settings) ? (p.settings as Record<string, unknown>) : {};
		pages.push({ id: p.id, name, kind: p.kind as MimoPageKind, settings });
	}
	if (!pages.length) return null;
	const rate = typeof o.rate === 'number' && MIMO_RATES.includes(o.rate) ? o.rate : MIMO_DEFAULT_RATE;
	let used = 0;
	for (const p of pages) {
		const m = Number(p.id.replace(/^[a-z]+-/, ''));
		if (Number.isInteger(m) && m > used) used = m;
	}
	return { version: 1, pages, rate, serial: Math.max(used, Number.isInteger(o.serial) ? (o.serial as number) : 0) };
}

/** The text a MIMO layout is saved as, fields in one order so equal layouts save equal text. */
export function mimoLayoutToSaved(l: MimoLayout): string {
	return JSON.stringify({ version: 1, pages: l.pages.map((p) => ({ id: p.id, name: p.name, kind: p.kind, settings: p.settings })), rate: l.rate, serial: l.serial });
}

export function mimoLayoutFromSaved(text: string | null): MimoLayout | null {
	if (!text) return null;
	try {
		return readMimoLayout(JSON.parse(text));
	} catch {
		return null;
	}
}
