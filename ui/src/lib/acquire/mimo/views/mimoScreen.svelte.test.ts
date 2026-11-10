/**
 * MIMO Random on the test screen's frame: its quantities as plot types, a plot's own choice, the
 * first pages, what each page subscribes to, a layout kept by the MIMO run views taken over, and
 * one window against a mock control service (the page on screen is subscribed, a removed or hidden
 * page is not).
 */
import { describe, expect, it } from 'vitest';
import { ServiceClient } from '$lib/protocol/client.svelte';
import { mockSocketFactory, type MockServer } from '$lib/protocol/mock/socket';
import type { MimoStatus, MimoSubscribePlot } from '$lib/protocol/mimo';
import { scrollWindow, togglePin } from '$lib/viz/matrixGrid';
import { layoutKey } from '../../display/layout';
import { MIMO_QUANTITY_ORDER, mimoLayoutToSaved, SELECTOR_KINDS, type MimoLayout } from '../../display/mimoLayout';
import { fits } from '../../screen/grid';
import { fromSaved, screenKey, toSaved } from '../../screen/layout';
import type { Panel, ScreenPage } from '../../screen/types';
import {
	COH_ERROR,
	defaultMimoScreen,
	GEOMETRY,
	MIMO_CARDS,
	MIMO_LEGACY,
	MIMO_TYPES,
	mimoFromLegacy,
	mimoParams,
	mimoSpec,
	mimoTitle,
	mimoType,
	panelPlots,
	selectorOf,
	type MimoContext
} from './mimoScreen';
import { MimoScreenState } from './mimoScreenState.svelte';

const labels = Array.from({ length: 72 }, (_, i) => `A${i + 1}`);
const drives = Array.from({ length: 24 }, (_, i) => `D${i + 1}`);
const ctx = (over: Partial<MimoContext> = {}): MimoContext => ({ labels, drive_labels: drives, space: 'physical', quantities: MIMO_QUANTITY_ORDER, widths: {}, pixelRatio: 1, ...over });
const offer = { types: MIMO_TYPES, cards: MIMO_CARDS };
type PlotP = Extract<Panel, { kind: 'plot' }>;
const plotsOf = (page: ScreenPage) => page.panels.filter((p): p is PlotP => p.kind === 'plot');
const wire = (page: ScreenPage, c = ctx()) => {
	const p = mimoParams(plotsOf(page), 5, c);
	return (p.plots as MimoSubscribePlot[] | undefined) ?? [];
};
const quantity = (q: string, select?: Record<string, unknown>) => ({ id: 'pl-9', plot: mimoSpec(mimoType(q)!, select) });

describe('plot types', () => {
	it('offers every quantity MIMO Random draws, and the geometry, in plain words with a line of help', () => {
		const ids = MIMO_TYPES.map((t) => t.id);
		expect(new Set(ids).size).toBe(ids.length);
		// the coherence error is the heatmap's second mode, not a plot of its own
		expect([...ids].filter((id) => id !== GEOMETRY).sort()).toEqual(MIMO_QUANTITY_ORDER.filter((q) => q !== COH_ERROR).sort());
		expect(ids).toContain(GEOMETRY);
		for (const t of MIMO_TYPES) {
			expect(t.words.length).toBeGreaterThan(0);
			expect(t.help).toMatch(/^[A-Z].*\.$/);
			expect(t.help).not.toMatch(/\n/);
			expect(t.channels).toBe(false);
			expect(t.time).toBe(false);
		}
	});

	it('puts each in its group, with the line families it draws', () => {
		const group = (g: string) => MIMO_TYPES.filter((t) => t.group === g).map((t) => t.id);
		expect(group('frequency')).toEqual(['psd', 'trace', 'coh', 'phase', 'csd_mag', 'mcoh', 'drive_psd', 'drive_coh', 'frf', 'sv', 'cond', 'pred_error_db', 'law_diag']);
		expect(group('other')).toEqual(['error_db', 'sdm', 'pred_sdm', GEOMETRY]);
		expect(group('numbers')).toEqual(['readings', 'dof_rms', 'cap', 'pred_drive_rms', 'pred_drive_accel']);
		expect(group('time')).toEqual([]);
		expect(mimoType('psd')?.families).toEqual(['measured', 'reference', 'tolerance', 'abort']);
		expect(mimoType('cond')?.families).toEqual(['measured', 'limit']);
		expect(mimoType('drive_psd')?.families).toEqual(['drive']);
	});

	it('draws the grid, the heatmap, the achievable reference and the geometry with bodies of their own', () => {
		expect(MIMO_TYPES.filter((t) => t.own).map((t) => t.id)).toEqual(['error_db', 'sdm', 'pred_sdm', GEOMETRY]);
		expect(MIMO_TYPES.filter((t) => t.own).every((t) => !t.families)).toBe(true);
	});

	it('carries the selectors each quantity takes, the first the one a new plot starts with', () => {
		for (const t of MIMO_TYPES) {
			if (t.id === GEOMETRY) {
				expect(t.selectors).toBeUndefined();
				continue;
			}
			const kinds = SELECTOR_KINDS[t.id as keyof typeof SELECTOR_KINDS].filter((k) => k !== 'none');
			expect(t.selectors ?? []).toEqual(kinds);
		}
		expect(mimoType('coh')?.selectors).toEqual(['pair']);
		expect(mimoType('psd')?.selectors).toEqual(['index', 'diagonal']);
		expect(mimoType('sv')?.selectors).toBeUndefined();
	});
});

describe("a plot's own choice", () => {
	it('starts with its first selector, and coherence and phase with their own axis', () => {
		expect(mimoSpec(mimoType('psd')!).select).toEqual({ kind: 'index', index: 0 });
		expect(mimoSpec(mimoType('coh')!).select).toEqual({ kind: 'pair', pair: [0, 1] });
		expect(mimoSpec(mimoType('frf')!).select).toEqual({ kind: 'pair', pair: [0, 0] });
		expect(mimoSpec(mimoType('cap')!).select).toEqual({ kind: 'diagonal' });
		expect(mimoSpec(mimoType('law_diag')!).select).toEqual({ kind: 'key', key: '', index: null });
		expect(mimoSpec(mimoType('sv')!).select).toBeUndefined();
		expect(mimoSpec(mimoType('sdm')!).select).toBeUndefined();
		expect(mimoSpec(mimoType('coh')!).display.y).toMatchObject({ range: 'fixed', min: 0, max: 1.05 });
		expect(mimoSpec(mimoType('phase')!).display.y).toMatchObject({ range: 'fixed', min: -180, max: 180 });
		expect(mimoSpec(mimoType('psd')!).display.y.range).toBe('auto');
	});

	it('is read for its quantity: a choice the quantity does not take falls back to its first', () => {
		expect(selectorOf('psd', { kind: 'diagonal' })).toEqual({ kind: 'diagonal' });
		expect(selectorOf('psd', { kind: 'pair', pair: [1, 2] })).toEqual({ kind: 'index', index: 0 });
		// a plot changed from coherence to phase keeps its pair
		expect(selectorOf('phase', { kind: 'pair', pair: [3, 7] })).toEqual({ kind: 'pair', pair: [3, 7] });
		expect(selectorOf('coh', { kind: 'pair', pair: [-1, 2] })).toEqual({ kind: 'pair', pair: [0, 1] });
		expect(selectorOf('coh', undefined)).toEqual({ kind: 'pair', pair: [0, 1] });
		expect(selectorOf('law_diag', { kind: 'key', key: 'step', index: 2 })).toEqual({ kind: 'key', key: 'step', index: 2 });
	});

	it('titles name what it picks, by the run\'s names', () => {
		const names = { labels, drive_labels: drives, quantities: MIMO_QUANTITY_ORDER };
		expect(mimoTitle(mimoSpec(mimoType('coh')!, { kind: 'pair', pair: [2, 5] }), names)).toBe('Coherence: A3 and A6');
		expect(mimoTitle(mimoSpec(mimoType('frf')!, { kind: 'pair', pair: [1, 3] }), names)).toBe('Frequency Response: A2 from D4');
		expect(mimoTitle(mimoSpec(mimoType('drive_psd')!), names)).toBe('Drive Spectrum: D1');
		expect(mimoTitle(mimoSpec(mimoType('psd')!, { kind: 'diagonal' }), names)).toBe('Control Spectrum: all controls');
		expect(mimoTitle(mimoSpec(mimoType('psd')!), { ...names, labels: [] })).toBe('Control Spectrum: Control 1');
		expect(mimoTitle(mimoSpec(mimoType('error_db')!, { mode: 'coherence' }), names)).toBe('Coherence Error Heatmap');
		expect(mimoTitle(mimoSpec(mimoType('error_db')!, { mode: 'coherence' }), { ...names, quantities: ['error_db'] })).toBe('Error Heatmap');
		expect(mimoTitle({ ...mimoSpec(mimoType('sv')!), title: 'Plant' }, names)).toBe('Plant');
		expect(mimoTitle(mimoSpec(mimoType(GEOMETRY)!), names)).toBe('Geometry');
	});
});

describe('the first pages', () => {
	const l = defaultMimoScreen(1.4);

	it('are the error heatmap, the matrix grid, the drives and the plant, and the geometry', () => {
		expect(l.pages.map((p) => [p.id, p.name])).toEqual([
			['pg-1', 'Error Heatmap'],
			['pg-2', 'Matrix Grid'],
			['pg-3', 'Drive and Plant'],
			['pg-4', 'Geometry']
		]);
		expect(l.pages.map((p) => plotsOf(p).map((x) => x.plot.quantity))).toEqual([['error_db'], ['sdm'], ['drive_psd', 'drive_coh', 'frf', 'sv', 'cond', 'cap'], [GEOMETRY]]);
		for (const p of l.pages) expect(fits(p.panels, p.cols, p.rows)).toBe(true);
		// one plot fills its page
		expect(l.pages[0].panels[0].at).toEqual({ x: 0, y: 0, w: 12, h: 12 });
		const ids = l.pages.flatMap((p) => [p.id, ...p.panels.map((x) => x.id)]);
		expect(new Set(ids).size).toBe(ids.length);
		expect(l.serial).toBeGreaterThanOrEqual(9);
	});

	it('keep the cards in the dock: Run, Level, Drives, Prediction and Station', () => {
		expect(l.dock.cards.map((c) => [c.kind, c.shown])).toEqual(MIMO_CARDS.map((k) => [k, true]));
		expect(MIMO_CARDS).toEqual(['run', 'level', 'drives', 'prediction', 'station']);
	});

	it('save and read back as they were, the plots\' own choices with them', () => {
		const copy = defaultMimoScreen(1.4);
		const grid = plotsOf(copy.pages[1])[0];
		grid.plot.select = { window: { pinned: ['A9'], offset: 2, size: 8 }, off: 'csd', open: { row: 1, col: 4, kind: 'csd' } };
		const back = fromSaved(toSaved(copy), offer, 1.4)!;
		expect(back).toEqual(copy);
		expect(toSaved(back)).toBe(toSaved(copy));
		// what is not plain data is left out
		grid.plot.select = { off: 'csd', bad: Number.NaN, fn: (() => 1) as unknown as number };
		expect(plotsOf(fromSaved(toSaved(copy), offer)!.pages[1])[0].plot.select).toEqual({ off: 'csd' });
	});
});

describe('what each page subscribes to', () => {
	const l = defaultMimoScreen(1.4);

	it('the heatmap: every row at its drawn width', () => {
		expect(wire(l.pages[0], ctx({ widths: { 'pl-1': { main: 800 } } }))).toEqual([{ id: 'pl-1', quantity: 'error_db', rows: 'all', points: 800 }]);
	});

	it('the heatmap: the rows chosen by name; the coherence mode only when offered', () => {
		const heat = (select: Record<string, unknown>, c = ctx()) => panelPlots(quantity('error_db', select), c).map((p) => [p.quantity, p.selector]);
		expect(heat({ rows: ['A3', 'A70', 'nobody'] })).toEqual([['error_db', { kind: 'rows', rows: [2, 69] }]]);
		expect(heat({ mode: 'coherence' })[0][0]).toBe(COH_ERROR);
		expect(heat({ mode: 'coherence' }, ctx({ quantities: MIMO_QUANTITY_ORDER.filter((q) => q !== COH_ERROR) }))[0][0]).toBe('error_db');
	});

	it("the grid: its window's block and its controls' spectra at the cell width; nothing streams whole", () => {
		const sub = wire(l.pages[1], ctx({ widths: { 'pl-2': { cell: 110 } } }));
		expect(sub[0]).toEqual({ id: 'pl-2:sdm', quantity: 'sdm', block: [0, 1, 2, 3, 4, 5, 6, 7], points: 110 });
		expect(sub.slice(1).map((p) => [p.id, p.quantity, (p as { index: number }).index, p.points])).toEqual([0, 1, 2, 3, 4, 5, 6, 7].map((i) => [`pl-2:psd:${i}`, 'psd', i, 110]));
		for (const p of sub) if ('block' in p) expect(p.block.length).toBeLessThanOrEqual(8);
		expect(wire(l.pages[1], ctx({ labels: [] }))).toEqual([]);
	});

	it('the grid follows its window: pinned, scrolled, and a cell opened full size', () => {
		let w = togglePin({ pinned: [], offset: 0, size: 8 }, 'A50');
		w = scrollWindow(labels, w, 20);
		const sub = panelPlots(quantity('sdm', { window: w, open: { row: 49, col: 21 } }), ctx({ widths: { 'pl-9': { cell: 100, full: 900 } } }));
		expect((sub[0].selector as { block: number[] }).block).toEqual([49, 20, 21, 22, 23, 24, 25, 26]);
		expect(sub.at(-1)).toMatchObject({ id: 'pl-9:open', quantity: 'phase', selector: { kind: 'pair', pair: [49, 21] }, points: 900 });
		expect(panelPlots(quantity('sdm', { open: { row: 2, col: 5 }, off: 'csd' }), ctx()).at(-1)).toMatchObject({ quantity: 'csd_mag' });
		expect(panelPlots(quantity('sdm', { open: { row: 3, col: 3 } }), ctx()).at(-1)).toMatchObject({ quantity: 'psd', selector: { kind: 'index', index: 3 } });
		// the achievable reference: its block only
		expect(panelPlots(quantity('pred_sdm'), ctx()).map((p) => [p.id, p.quantity])).toEqual([['pl-9:sdm', 'pred_sdm']]);
	});

	it('the drives and the plant: the drive, a pair of drives, a response, singular values, condition and caps', () => {
		const sub = wire(l.pages[2], ctx({ widths: { 'pl-3': { main: 500 } } }));
		expect(sub.map((p) => [p.id, p.quantity])).toEqual([
			['pl-3', 'drive_psd'],
			['pl-4', 'drive_coh'],
			['pl-5', 'frf'],
			['pl-6', 'sv'],
			['pl-7', 'cond'],
			['pl-8', 'cap']
		]);
		expect(sub[0]).toEqual({ id: 'pl-3', quantity: 'drive_psd', index: 0, points: 500 });
		expect(sub[1]).toMatchObject({ pair: [0, 1] });
		expect(sub[2]).toMatchObject({ pair: [0, 0] });
		expect(sub[5]).toMatchObject({ diagonal: true, points: 64 });
	});

	it('the geometry: the degrees of freedom under virtual control, nothing under physical', () => {
		expect(wire(l.pages[3])).toEqual([]);
		expect(wire(l.pages[3], ctx({ space: 'virtual' }))).toEqual([{ id: 'pl-9', quantity: 'dof_rms', points: 64 }]);
	});

	it('a diagnostic is asked for once one is chosen; nothing shown asks for the default', () => {
		expect(panelPlots(quantity('law_diag'), ctx()).length).toBe(1);
		expect(mimoParams([quantity('law_diag')], 5, ctx())).toEqual({ default: true });
		expect(mimoParams([quantity('law_diag', { kind: 'key', key: 'step', index: 1 })], 2, ctx())).toEqual({ rate_hz: 2, plots: [{ id: 'pl-9', quantity: 'law_diag', key: 'step', index: 1, points: 1024 }] });
		expect(mimoParams([], 5, ctx())).toEqual({ default: true });
	});
});

describe('a layout the MIMO run views kept', () => {
	const v1: MimoLayout = {
		version: 1,
		rate: 2,
		serial: 7,
		pages: [
			{ id: 'mp-2', name: 'Grid A', kind: 'grid', settings: { window: { pinned: ['A2'], offset: 3, size: 8 }, off: 'csd', open: { row: 1, col: 2, kind: 'csd' } } },
			{ id: 'mp-7', name: 'Plant', kind: 'plant', settings: { drive: 3, drivePair: [2, 5], frf: [4, 1], diag: 'step', diagIndex: 2 } },
			{ id: 'mp-1', name: 'Overview', kind: 'heatmap', settings: { mode: 'coherence', order: 'worst' } },
			{ id: 'mp-4', name: 'Shape', kind: 'geometry', settings: { motion: false } }
		]
	};

	it('is taken over: its pages in order with their names, their choices and its rate', () => {
		const l = mimoFromLegacy(v1, 1.4);
		expect(l.version).toBe(2);
		expect(l.rate).toBe(2);
		expect(l.pages.map((p) => p.name)).toEqual(['Grid A', 'Plant', 'Overview', 'Shape']);
		const [grid, plant, heat, geo] = l.pages.map(plotsOf);
		expect(grid[0].plot).toMatchObject({ quantity: 'sdm', select: { window: { pinned: ['A2'], offset: 3, size: 8 }, off: 'csd', open: { row: 1, col: 2, kind: 'csd' } } });
		expect(plant.map((p) => [p.plot.quantity, p.plot.select])).toEqual([
			['drive_psd', { kind: 'index', index: 3 }],
			['drive_coh', { kind: 'pair', pair: [2, 5] }],
			['frf', { kind: 'pair', pair: [4, 1] }],
			['sv', undefined],
			['cond', undefined],
			['cap', { kind: 'diagonal' }],
			['law_diag', { kind: 'key', key: 'step', index: 2 }]
		]);
		expect(heat[0].plot).toMatchObject({ quantity: 'error_db', select: { mode: 'coherence', order: 'worst' } });
		expect(geo[0].plot).toMatchObject({ quantity: GEOMETRY, select: { motion: false } });
		for (const p of l.pages) expect(fits(p.panels, p.cols, p.rows)).toBe(true);
		// what it asks for is what the old pages asked for
		expect(wire(l.pages[1]).map((p) => p.quantity)).toEqual(['drive_psd', 'drive_coh', 'frf', 'sv', 'cond', 'cap', 'law_diag']);
		expect(wire(l.pages[2])[0]).toMatchObject({ quantity: COH_ERROR });
	});

	it('is read from where it was kept; anything else is not one', () => {
		expect(MIMO_LEGACY.key('Bench')).toBe(layoutKey('mimo_random', 'Bench'));
		expect(MIMO_LEGACY.read(mimoLayoutToSaved(v1))?.pages.map((p) => p.name)).toEqual(['Grid A', 'Plant', 'Overview', 'Shape']);
		expect(MIMO_LEGACY.read('not json')).toBeNull();
		expect(MIMO_LEGACY.read(JSON.stringify({ version: 1, pages: [{ id: 'x-1', name: 'Bad', kind: 'nonsense' }] }))).toBeNull();
		expect(MIMO_LEGACY.read(null)).toBeNull();
	});
});

// ---- one window against a mock control service --------------------------------------------------

const until = async (cond: () => boolean, ms = 3000) => {
	const t0 = Date.now();
	while (!cond()) {
		if (Date.now() - t0 > ms) throw new Error('timed out waiting');
		await new Promise((r) => setTimeout(r, 5));
	}
};

function status(n = 12, nd = 4, space: 'physical' | 'virtual' = 'physical'): MimoStatus {
	return {
		run_id: 'm-1',
		state: 'running',
		phase: 'full_level',
		title: 'T',
		source: 'SIM',
		station: null,
		grid: null,
		clock: 'shared',
		shape: { controls: n, drives: nd, space, dofs: null, shape: 'more_controls_than_drives', labels: labels.slice(0, n), drive_labels: drives.slice(0, nd) },
		mode: 'auto',
		levels_db: [0],
		level_db: 0,
		elapsed_s: 10,
		at_level_s: 5,
		remaining_at_level_s: 25,
		loop: 7,
		alarm: false,
		control_error_db: [],
		gates: null,
		caps: [],
		drive_rms_v: [],
		law: null,
		faults: [],
		override: null,
		prediction: null,
		end: null
	};
}

function server(st: () => MimoStatus = () => status()) {
	const calls: Record<string, unknown>[] = [];
	const s: MockServer = {
		hello: { type: 'hello', protocol: 'sds/1', service: 'control', version: '0.1.0' },
		methods: {
			'mimo_random.status': () => st() as unknown as Record<string, unknown>,
			'mimo_random.subscribe': (params) => {
				calls.push(params);
				if (params.default === true) return { default: true, rate_hz: 5, plots: null, quantities: MIMO_QUANTITY_ORDER };
				return { default: false, rate_hz: params.rate_hz, plots: params.plots, quantities: MIMO_QUANTITY_ORDER };
			}
		}
	};
	return { s, calls };
}

function memory(): Storage & { map: Map<string, string> } {
	const map = new Map<string, string>();
	return {
		map,
		get length() {
			return map.size;
		},
		clear: () => map.clear(),
		getItem: (k) => map.get(k) ?? null,
		key: (i) => [...map.keys()][i] ?? null,
		removeItem: (k) => void map.delete(k),
		setItem: (k, v) => void map.set(k, v)
	};
}

const quantities = (c: Record<string, unknown> | undefined) => ((c?.plots as MimoSubscribePlot[] | undefined) ?? []).map((p) => p.quantity);

describe('one window of the MIMO run screen', () => {
	it('subscribes for the page on screen only, follows the page and its plots, and asks for nothing hidden', async () => {
		const { s, calls } = server();
		const control = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(s, 1) });
		let showing = $state(true);
		const screen = new MimoScreenState(control, { setupTitle: () => 'T', showing: () => showing, storage: null, session: null, events: null, subscribeDelayMs: 5, pixelRatio: 1 });
		const stop = screen.watch();
		control.connect();
		await until(() => screen.status !== null);
		// the heatmap first
		await until(() => quantities(calls.at(-1)).includes('error_db'));
		expect(quantities(calls.at(-1))).toEqual(['error_db']);

		// the grid: its block and one spectrum for each of its 8 controls, nothing of the heatmap
		screen.selectPage('pg-2');
		await until(() => quantities(calls.at(-1))[0] === 'sdm');
		expect(quantities(calls.at(-1))).toEqual(['sdm', ...Array(8).fill('psd')]);

		// a choice made in the grid changes what it asks for
		screen.choose('pl-2', { window: { pinned: ['A12'], offset: 0, size: 8 } });
		await until(() => ((calls.at(-1)?.plots as { block?: number[] }[])?.[0]?.block ?? [])[0] === 11);
		expect((calls.at(-1)!.plots as { block: number[] }[])[0].block).toEqual([11, 0, 1, 2, 3, 4, 5, 6]);

		// a plot added to the page is asked for with MIMO's defaults; removed, it is let go
		const added = screen.addPlot('coh')!;
		expect(screen.plot(added)?.select).toEqual({ kind: 'pair', pair: [0, 1] });
		expect(screen.titleOf(screen.find(added)!.panel)).toBe('Coherence: A1 and A2');
		await until(() => quantities(calls.at(-1)).includes('coh'));
		screen.removePanel(added);
		await until(() => !quantities(calls.at(-1)).includes('coh'));

		// the page on screen removed: the window shows its neighbour and asks for that instead
		screen.removePage('pg-2');
		await until(() => quantities(calls.at(-1))[0] === 'error_db');
		expect(screen.pageId).toBe('pg-1');
		expect(JSON.stringify(calls.at(-1))).not.toContain('pl-2:');

		// hidden: nothing is asked for
		showing = false;
		await until(() => calls.at(-1)?.default === true);
		stop();
		control.close();
	});

	it('a heatmap row opens its control in the grid, adding a grid if there is none', async () => {
		const { s } = server(() => status(72, 24));
		const control = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(s, 1) });
		const screen = new MimoScreenState(control, { setupTitle: () => 'T', showing: () => true, storage: null, session: null, events: null, subscribeDelayMs: 5, pixelRatio: 1 });
		const stop = screen.watch();
		control.connect();
		await until(() => screen.labels.length === 72);
		screen.openInGrid(40);
		expect(screen.pageId).toBe('pg-2');
		expect((panelPlots(plotsOf(screen.page)[0], screen.context)[0].selector as { block: number[] }).block).toContain(40);
		screen.removePage('pg-2');
		screen.selectPage('pg-1');
		screen.openInGrid(5);
		expect(screen.page.name).toBe('Matrix Grid');
		expect(plotsOf(screen.page).map((p) => p.plot.quantity)).toEqual(['sdm']);
		expect((panelPlots(plotsOf(screen.page)[0], screen.context)[0].selector as { block: number[] }).block).toContain(5);
		stop();
		control.close();
	});

	it("keeps a run's finish: an older status of the same run does not take it back", async () => {
		const conns = new Set<{ emit(topic: string, data: Record<string, unknown>): void }>();
		const { s } = server();
		s.attach = (c) => void conns.add(c);
		const control = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(s, 1) });
		const screen = new MimoScreenState(control, { setupTitle: () => 'T', showing: () => true, storage: null, session: null, events: null });
		control.connect();
		await until(() => conns.size > 0);
		const emit = (st: MimoStatus) => conns.forEach((c) => c.emit('mimo_random.status', st as unknown as Record<string, unknown>));
		emit({ ...status(), state: 'finished', phase: 'stopped' });
		await until(() => screen.status?.state === 'finished');
		emit({ ...status(), state: 'stopping', phase: 'ramping_down' });
		await new Promise((r) => setTimeout(r, 30));
		expect(screen.status?.state).toBe('finished');
		// a new run is a new id
		emit({ ...status(), run_id: 'm-2', state: 'running' });
		await until(() => screen.status?.run_id === 'm-2');
		expect(screen.status?.state).toBe('running');
		control.close();
	});

	it("a drive picked shows on the page's drive spectrum", () => {
		const { s } = server();
		const control = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(s, 1) });
		const screen = new MimoScreenState(control, { setupTitle: () => 'T', showing: () => true, storage: null, session: null, events: null });
		expect(screen.showDrive(2)).toBe(false);
		screen.selectPage('pg-3');
		expect(screen.showDrive(2)).toBe(true);
		expect(screen.plot('pl-3')?.select).toEqual({ kind: 'index', index: 2 });
	});

	it('takes over the layout the MIMO run views kept for the setup, once, and keeps it in the new form', async () => {
		const storage = memory();
		const v1: MimoLayout = { version: 1, rate: 10, serial: 3, pages: [{ id: 'mp-3', name: 'Plant', kind: 'plant', settings: { drive: 1 } }] };
		storage.setItem(layoutKey('mimo_random', 'Bench'), mimoLayoutToSaved(v1));
		const { s } = server();
		const control = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(s, 1) });
		const screen = new MimoScreenState(control, { setupTitle: () => 'Bench', showing: () => true, storage, session: null, events: null });
		const stop = screen.watch();
		await until(() => screen.key !== '');
		expect(screen.pages.map((p) => p.name)).toEqual(['Plant']);
		expect(screen.layout.rate).toBe(10);
		expect(screen.plot('pl-1')?.select).toEqual({ kind: 'index', index: 1 });
		await until(() => storage.map.has(screenKey('mimo_random', 'Bench')));
		expect(fromSaved(storage.map.get(screenKey('mimo_random', 'Bench'))!, offer)?.pages.map((p) => p.name)).toEqual(['Plant']);
		stop();
	});
});

describe('the level card in words', async () => {
	const { mimoLevelStatus } = await import('./words');
	it('says what the run is doing, with a mark that is not a color', () => {
		expect(mimoLevelStatus(null)).toEqual({ mark: 'idle', text: 'Waiting for the run', tone: 'info' });
		expect(mimoLevelStatus({ ...status(), phase: 'full_level', level_db: -6 })).toMatchObject({ mark: 'control', text: 'Controlling at -6.0 dB', tone: 'pass' });
		expect(mimoLevelStatus({ ...status(), phase: 'full_level', alarm: true })).toMatchObject({ tone: 'warn' });
		expect(mimoLevelStatus({ ...status(), phase: 'sysid' })).toMatchObject({ mark: 'identify' });
		expect(mimoLevelStatus({ ...status(), phase: 'gate_failed' })).toMatchObject({ mark: 'hold', tone: 'warn' });
		expect(mimoLevelStatus({ ...status(), phase: 'aborted', end: null })).toMatchObject({ mark: 'ended', tone: 'fail', text: 'Aborted' });
	});
});
