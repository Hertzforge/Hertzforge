/**
 * What each MIMO run-view page shows and asks the service for. A page's choices live in its
 * settings (plain data, saved with the layout and shared by this browser's windows); its plots
 * follow from those choices, the run's names, and the page's size on screen. Nothing here draws.
 *
 * Every plot a page subscribes to is one it draws now: the grid asks for its window's block of
 * the spectral matrix and its window controls' spectra (and the cell open full size); the heatmap
 * asks for its rows; the drive and plant page for its chosen drive, pair and diagnostic; the
 * geometry for the degree-of-freedom rms under virtual control. A page that is not on screen asks
 * for nothing.
 */

import type { MimoQuantity } from '$lib/protocol/mimo';
import { newMimoPlot, pointsForWidth, type MimoPageSpec, type MimoPlotSpec } from '$lib/acquire/display/mimoLayout';
import type { HeatMode } from '$lib/viz/heatmap';
import { cellKind, defaultWindow, GRID_MAX, windowControls, type CellKind, type GridWindowSpec, type OffDiagonal } from '$lib/viz/matrixGrid';

/**
 * The coherence error per control, when the service offers it: the heatmap's second mode. Not a
 * quantity on the wire yet; the mode appears when the service lists it among its quantities.
 */
export const COH_ERROR = 'coh_error';

const CELL_KINDS: CellKind[] = ['psd', 'coh', 'phase', 'csd'];

export interface GridSettings {
	window: GridWindowSpec;
	off: OffDiagonal;
	/**
	 * The cell open full size, by the controls' positions (row, column) and what the cell showed
	 * (above or below the diagonal of the window as drawn, which pinning can reorder), or none.
	 */
	open: { row: number; col: number; kind: CellKind } | null;
}

export interface HeatmapSettings {
	mode: HeatMode;
	order: 'setup' | 'worst';
	/** The controls shown, by name, or all of them. */
	rows: 'all' | string[];
}

export interface PlantSettings {
	/** The drive whose spectrum is shown. */
	drive: number;
	/** The two drives whose coherence is shown. */
	drivePair: [number, number];
	/** The control and the drive whose frequency response is shown. */
	frf: [number, number];
	/** The law diagnostic shown, by key, with its position for a per-control or per-drive one. */
	diag: string | null;
	diagIndex: number | null;
}

export interface GeometrySettings {
	/** Animate the rigid-body motion. */
	motion: boolean;
}

const isPos = (v: unknown): v is number => Number.isInteger(v) && (v as number) >= 0;
const obj = (v: unknown): Record<string, unknown> => (typeof v === 'object' && v !== null && !Array.isArray(v) ? (v as Record<string, unknown>) : {});

export function gridSettings(page: MimoPageSpec): GridSettings {
	const s = page.settings;
	const w = obj(s.window);
	const window: GridWindowSpec = {
		pinned: Array.isArray(w.pinned) ? [...new Set(w.pinned.filter((x): x is string => typeof x === 'string'))].slice(0, GRID_MAX) : [],
		offset: isPos(w.offset) ? w.offset : 0,
		size: isPos(w.size) && w.size >= 1 && w.size <= GRID_MAX ? w.size : GRID_MAX
	};
	const o = obj(s.open);
	return {
		window: { ...defaultWindow(), ...window },
		off: s.off === 'csd' ? 'csd' : 'coh_phase',
		open: isPos(o.row) && isPos(o.col) ? { row: o.row, col: o.col, kind: CELL_KINDS.includes(o.kind as CellKind) ? (o.kind as CellKind) : cellKind(o.row, o.col, s.off === 'csd' ? 'csd' : 'coh_phase') } : null
	};
}

export function heatmapSettings(page: MimoPageSpec): HeatmapSettings {
	const s = page.settings;
	return {
		mode: s.mode === 'coherence' ? 'coherence' : 'error',
		order: s.order === 'worst' ? 'worst' : 'setup',
		rows: Array.isArray(s.rows) && s.rows.length ? s.rows.filter((x): x is string => typeof x === 'string') : 'all'
	};
}

export function plantSettings(page: MimoPageSpec): PlantSettings {
	const s = page.settings;
	const pair = (v: unknown, d: [number, number]): [number, number] => (Array.isArray(v) && v.length === 2 && v.every(isPos) ? [v[0], v[1]] : d);
	return {
		drive: isPos(s.drive) ? s.drive : 0,
		drivePair: pair(s.drivePair, [0, 1]),
		frf: pair(s.frf, [0, 0]),
		diag: typeof s.diag === 'string' && s.diag ? s.diag : null,
		diagIndex: isPos(s.diagIndex) ? s.diagIndex : null
	};
}

export function geometrySettings(page: MimoPageSpec): GeometrySettings {
	return { motion: page.settings.motion !== false };
}

/** What a page's plots depend on besides its settings. */
export interface PageContext {
	/** The controlled quantities' names, in order (the status's `shape.labels`). */
	labels: string[];
	drive_labels: string[];
	space: 'physical' | 'virtual' | null;
	/** The quantities the service offers, or null before it has said. */
	quantities: MimoQuantity[] | null;
	/** The page's drawn widths in CSS pixels, by part (`cell`, `full`, `main`, `half`). */
	widths: Record<string, number>;
	pixelRatio?: number;
}

const pts = (ctx: PageContext, part: string, fallback: number) => pointsForWidth(ctx.widths[part] ?? fallback, ctx.pixelRatio ?? 1);

/** The plot id a page uses for one of its parts: the page's id, then the part. */
export const plotId = (page: MimoPageSpec, part: string) => `${page.id}:${part}`;

/** The quantity a grid cell opens full size as, with its selector. */
export function openCellPlot(page: MimoPageSpec, open: { row: number; col: number; kind: CellKind }, points: number): MimoPlotSpec {
	const { row, col, kind } = open;
	const id = plotId(page, 'open');
	const p = (q: MimoQuantity, s: MimoPlotSpec['selector']) => ({ ...newMimoPlot(id, q, s), points });
	if (kind === 'psd') return p('psd', { kind: 'index', index: row });
	if (kind === 'coh') return p('coh', { kind: 'pair', pair: [row, col] });
	if (kind === 'phase') return p('phase', { kind: 'pair', pair: [row, col] });
	return p('csd_mag', { kind: 'pair', pair: [row, col] });
}

/** The controls a heatmap page shows, by position. */
export function heatmapRows(s: HeatmapSettings, labels: string[]): 'all' | number[] {
	if (s.rows === 'all') return 'all';
	const pos = s.rows.map((n) => labels.indexOf(n)).filter((i) => i >= 0);
	return pos.length ? pos : 'all';
}

/** Whether the heatmap's coherence mode can be offered: the service lists the quantity. */
export function coherenceModeOffered(quantities: MimoQuantity[] | null): boolean {
	return !!quantities && (quantities as string[]).includes(COH_ERROR);
}

/** Everything a page subscribes to now. */
export function pagePlots(page: MimoPageSpec, ctx: PageContext): MimoPlotSpec[] {
	switch (page.kind) {
		case 'grid': {
			if (!ctx.labels.length) return [];
			const s = gridSettings(page);
			const window = windowControls(ctx.labels, s.window);
			const cell = pts(ctx, 'cell', 120);
			const out: MimoPlotSpec[] = [{ ...newMimoPlot(plotId(page, 'sdm'), 'sdm', { kind: 'block', block: window }), points: cell }];
			for (const i of window) out.push({ ...newMimoPlot(plotId(page, `psd:${i}`), 'psd', { kind: 'index', index: i }), points: cell });
			if (s.open && s.open.row < ctx.labels.length && s.open.col < ctx.labels.length) out.push(openCellPlot(page, s.open, pts(ctx, 'full', 1024)));
			return out;
		}
		case 'heatmap': {
			const s = heatmapSettings(page);
			const rows = heatmapRows(s, ctx.labels);
			const quantity = (s.mode === 'coherence' && coherenceModeOffered(ctx.quantities) ? COH_ERROR : 'error_db') as MimoQuantity;
			return [{ ...newMimoPlot(plotId(page, 'heat'), quantity, { kind: 'rows', rows }), points: pts(ctx, 'main', 1024) }];
		}
		case 'plant': {
			const s = plantSettings(page);
			const nd = ctx.drive_labels.length;
			const nc = ctx.labels.length;
			if (!nd || !nc) return [];
			const half = pts(ctx, 'half', 600);
			const drive = Math.min(s.drive, nd - 1);
			const out: MimoPlotSpec[] = [
				{ ...newMimoPlot(plotId(page, 'drive'), 'drive_psd', { kind: 'index', index: drive }), points: half },
				{ ...newMimoPlot(plotId(page, 'frf'), 'frf', { kind: 'pair', pair: [Math.min(s.frf[0], nc - 1), Math.min(s.frf[1], nd - 1)] }), points: half },
				{ ...newMimoPlot(plotId(page, 'sv'), 'sv'), points: half },
				{ ...newMimoPlot(plotId(page, 'cond'), 'cond'), points: half },
				{ ...newMimoPlot(plotId(page, 'cap'), 'cap', { kind: 'diagonal' }), points: 64 }
			];
			if (nd >= 2) {
				const a = Math.min(s.drivePair[0], nd - 1);
				let b = Math.min(s.drivePair[1], nd - 1);
				if (a === b) b = a === 0 ? 1 : 0;
				out.splice(1, 0, { ...newMimoPlot(plotId(page, 'dcoh'), 'drive_coh', { kind: 'pair', pair: [a, b] }), points: half });
			}
			if (s.diag) out.push({ ...newMimoPlot(plotId(page, 'diag'), 'law_diag', { kind: 'key', key: s.diag, index: s.diagIndex }), points: half });
			return out;
		}
		case 'geometry':
			return ctx.space === 'virtual' ? [{ ...newMimoPlot(plotId(page, 'dof'), 'dof_rms'), points: 64 }] : [];
	}
}
