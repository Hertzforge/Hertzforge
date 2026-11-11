/**
 * A plot panel's saved choice, read back field by field. Stored layouts come from this browser's
 * storage and, later, from setup files: nothing in them is trusted. A field that is missing, of
 * the wrong kind or out of bounds takes its default, so a damaged or older layout still opens.
 *
 * Saving writes every field in a fixed order (traces by name), so the same choice always saves to
 * the same text.
 */

import type { ChannelSelector } from '$lib/protocol/types';
import { traceColor } from '$lib/viz/roles';
import {
	defaultAxis,
	defaultDisplay,
	type AxisDisplay,
	type AxisScale,
	type Corner,
	type CursorMode,
	type Dash,
	type GridLines,
	type HoldMode,
	type LegendPlace,
	type LineFamily,
	type Marker,
	type PlotDisplay,
	type PlotNote,
	type PlotPanelSpec,
	type PlotType,
	type RangeMode,
	type TraceStyle
} from './types';

export const FAMILIES: LineFamily[] = ['measured', 'reference', 'tolerance', 'abort', 'limit', 'drive', 'other'];
export const SCALES: AxisScale[] = ['auto', 'log', 'linear'];
export const RANGES: RangeMode[] = ['auto', 'band', 'fixed', 'held'];
export const GRIDS: GridLines[] = ['major', 'minor', 'none'];
export const LEGENDS: LegendPlace[] = ['inside', 'outside', 'off'];
export const CORNERS: Corner[] = ['top-left', 'top-right', 'bottom-left', 'bottom-right'];
export const CURSORS: CursorMode[] = ['none', 'one', 'two', 'peak', 'peaks', 'harmonics'];
export const HOLDS: HoldMode[] = ['none', 'peak', 'min', 'average', 'exponential'];
export const DASHES: Dash[] = ['solid', 'dashed', 'dotted', 'dashdot'];
export const MARKERS: Marker[] = ['none', 'circle', 'square', 'triangle', 'diamond', 'cross'];
export const TICKS: AxisDisplay['ticks'][] = ['sparse', 'normal', 'dense'];

/** Bounds on what a stored layout may hold. */
export const LIMITS = {
	count: [1, 20],
	average: [2, 256],
	points: [64, 65536],
	width: [0.5, 6],
	window_s: [0.001, 3600],
	notes: 50,
	traces: 200,
	channels: 512,
	text: 200,
	label: 120
} as const;

const isRecord = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);

function oneOf<T extends string>(v: unknown, options: readonly T[], fallback: T): T {
	return typeof v === 'string' && (options as readonly string[]).includes(v) ? (v as T) : fallback;
}

function finiteOrNull(v: unknown): number | null {
	return typeof v === 'number' && Number.isFinite(v) ? v : null;
}

function intIn(v: unknown, [lo, hi]: readonly [number, number], fallback: number): number {
	return typeof v === 'number' && Number.isInteger(v) && v >= lo && v <= hi ? v : fallback;
}

function textOrNull(v: unknown, max: number): string | null {
	if (typeof v !== 'string') return null;
	const t = v.trim().slice(0, max);
	return t.length ? t : null;
}

const clamp01 = (v: unknown, fallback: number) => (typeof v === 'number' && Number.isFinite(v) ? Math.min(1, Math.max(0, v)) : fallback);

export function readAxis(raw: unknown): AxisDisplay {
	const d = defaultAxis();
	if (!isRecord(raw)) return d;
	let min = finiteOrNull(raw.min);
	let max = finiteOrNull(raw.max);
	if (min !== null && max !== null && !(max > min)) [min, max] = [null, null];
	return {
		scale: oneOf(raw.scale, SCALES, d.scale),
		range: oneOf(raw.range, RANGES, d.range),
		min,
		max,
		label: textOrNull(raw.label, LIMITS.label),
		unit: textOrNull(raw.unit, LIMITS.label),
		ticks: oneOf(raw.ticks, TICKS, d.ticks)
	};
}

function readTrace(raw: unknown): TraceStyle | null {
	if (!isRecord(raw)) return null;
	const width = typeof raw.width === 'number' && raw.width >= LIMITS.width[0] && raw.width <= LIMITS.width[1] ? raw.width : null;
	const color = typeof raw.color === 'string' ? (traceColor(raw.color)?.hex ?? null) : null;
	const marker = raw.marker === null || raw.marker === undefined ? null : oneOf<Marker | ''>(raw.marker, MARKERS, '') || null;
	return {
		visible: raw.visible !== false,
		width,
		dash: raw.dash === null || raw.dash === undefined ? null : oneOf<Dash | ''>(raw.dash, DASHES, '') || null,
		color,
		marker
	};
}

function readNote(raw: unknown, k: number): PlotNote | null {
	if (!isRecord(raw)) return null;
	const text = textOrNull(raw.text, LIMITS.text);
	if (text === null) return null;
	const id = textOrNull(raw.id, 40) ?? `n${k + 1}`;
	return { id, text, fx: clamp01(raw.fx, 0.5), fy: clamp01(raw.fy, 0.2) };
}

/** A stored display, every field checked; defaults for what is missing or wrong. */
export function readDisplay(raw: unknown): PlotDisplay {
	const d = defaultDisplay();
	if (!isRecord(raw)) return { ...d, cursorAt: [null, null], holdEpoch: 0 };
	const hidden = Array.isArray(raw.hidden) ? FAMILIES.filter((f) => (raw.hidden as unknown[]).includes(f)) : [];
	const traces: Record<string, TraceStyle> = {};
	if (isRecord(raw.traces)) {
		for (const [name, t] of Object.entries(raw.traces).slice(0, LIMITS.traces)) {
			const style = readTrace(t);
			if (style && name.length > 0 && name.length <= LIMITS.text) traces[name] = style;
		}
	}
	const notes: PlotNote[] = [];
	const ids = new Set<string>();
	if (Array.isArray(raw.notes)) {
		raw.notes.slice(0, LIMITS.notes).forEach((n, k) => {
			const note = readNote(n, k);
			if (note && !ids.has(note.id)) {
				ids.add(note.id);
				notes.push(note);
			}
		});
	}
	const at = Array.isArray(raw.cursorAt) ? raw.cursorAt : [];
	const window = typeof raw.window_s === 'number' && raw.window_s >= LIMITS.window_s[0] && raw.window_s <= LIMITS.window_s[1] ? raw.window_s : null;
	return {
		x: readAxis(raw.x),
		y: readAxis(raw.y),
		grid: oneOf(raw.grid, GRIDS, d.grid),
		legend: oneOf(raw.legend, LEGENDS, d.legend),
		legendCorner: oneOf(raw.legendCorner, CORNERS, d.legendCorner),
		readout: typeof raw.readout === 'boolean' ? raw.readout : d.readout,
		hidden,
		traces,
		cursor: oneOf(raw.cursor, CURSORS, d.cursor),
		cursorAt: [finiteOrNull(at[0]), finiteOrNull(at[1])],
		count: intIn(raw.count, LIMITS.count, d.count),
		hold: oneOf(raw.hold, HOLDS, d.hold),
		average: intIn(raw.average, LIMITS.average, d.average),
		holdEpoch: typeof raw.holdEpoch === 'number' && Number.isInteger(raw.holdEpoch) && raw.holdEpoch >= 0 ? raw.holdEpoch : 0,
		timeRender: oneOf(raw.timeRender, ['waveform', 'envelope'] as const, d.timeRender),
		lanes: oneOf(raw.lanes, ['overlay', 'split'] as const, d.lanes),
		window_s: window,
		notes
	};
}

/** A stored channel choice: a preset, or channel numbers (whole, not negative, each once, in order). */
export function readChannels(raw: unknown): ChannelSelector {
	if (raw === 'all' || raw === 'control' || raw === 'limited') return raw;
	if (!Array.isArray(raw)) return 'all';
	const picked = [...new Set(raw.filter((c): c is number => typeof c === 'number' && Number.isInteger(c) && c >= 0))].sort((a, b) => a - b);
	return picked.slice(0, LIMITS.channels);
}

/**
 * A stored panel spec, or null when it names no quantity. With `types`, a quantity the test does
 * not offer is null too, so the screen drops the panel rather than draw something unknown.
 */
export function readPanelSpec(raw: unknown, types?: readonly PlotType[]): PlotPanelSpec | null {
	if (!isRecord(raw)) return null;
	const quantity = textOrNull(raw.quantity, 60);
	if (quantity === null) return null;
	const type = types?.find((t) => t.id === quantity);
	if (types && !type) return null;
	return {
		quantity,
		channels: readChannels(raw.channels),
		arrange: oneOf(raw.arrange, ['split', 'overlay'] as const, type?.time ? 'overlay' : 'split'),
		points: intIn(raw.points, LIMITS.points, 1024),
		title: textOrNull(raw.title, LIMITS.label),
		display: readDisplay(raw.display)
	};
}

function saveAxis(a: AxisDisplay) {
	return { scale: a.scale, range: a.range, min: a.min, max: a.max, label: a.label, unit: a.unit, ticks: a.ticks };
}

/** The display as plain data in a fixed order: the same choice always saves to the same text. */
export function saveDisplay(d: PlotDisplay) {
	const traces: Record<string, { visible: boolean; width: number | null; dash: Dash | null; color: string | null; marker: Marker | null }> = {};
	for (const name of Object.keys(d.traces).sort()) {
		const t = d.traces[name];
		traces[name] = { visible: t.visible, width: t.width, dash: t.dash, color: t.color, marker: t.marker ?? null };
	}
	return {
		x: saveAxis(d.x),
		y: saveAxis(d.y),
		grid: d.grid,
		legend: d.legend,
		legendCorner: d.legendCorner,
		readout: d.readout,
		hidden: FAMILIES.filter((f) => d.hidden.includes(f)),
		traces,
		cursor: d.cursor,
		cursorAt: [d.cursorAt?.[0] ?? null, d.cursorAt?.[1] ?? null],
		count: d.count,
		hold: d.hold,
		average: d.average,
		holdEpoch: d.holdEpoch ?? 0,
		timeRender: d.timeRender,
		lanes: d.lanes,
		window_s: d.window_s,
		notes: d.notes.map((n) => ({ id: n.id, text: n.text, fx: n.fx, fy: n.fy }))
	};
}

export function savePanelSpec(s: PlotPanelSpec) {
	return {
		quantity: s.quantity,
		channels: Array.isArray(s.channels) ? [...s.channels] : s.channels,
		arrange: s.arrange,
		points: s.points,
		title: s.title,
		display: saveDisplay(s.display)
	};
}

/** The plot's title: the one given, or its type's name. */
export function plotTitle(spec: Pick<PlotPanelSpec, 'title' | 'quantity'>, type: Pick<PlotType, 'words'> | null | undefined): string {
	return spec.title ?? type?.words ?? spec.quantity;
}

/** The spec with its plot type changed: the type's own channels, arrangement, axes and lines. */
export function withType(spec: PlotPanelSpec, type: PlotType): PlotPanelSpec {
	const d = spec.display;
	return {
		...spec,
		quantity: type.id,
		channels: type.channels ? spec.channels : 'all',
		parts: type.parts ? (spec.parts ?? (type.parts === 'both' ? 'all' : type.parts)) : undefined,
		arrange: type.time ? 'overlay' : 'split',
		display: { ...d, x: defaultAxis(), y: defaultAxis(), hidden: [], traces: {}, cursorAt: [null, null], window_s: null, lanes: 'overlay', hold: type.time ? 'none' : d.hold }
	};
}
