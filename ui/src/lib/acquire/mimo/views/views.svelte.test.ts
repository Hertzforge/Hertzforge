/**
 * The MIMO run views' pages: what each page subscribes to for a given selection, the default
 * layouts on one and two monitors, and one window against a mock control service (the page on
 * screen is subscribed, a removed or hidden page is not).
 */
import { describe, expect, it } from 'vitest';
import { ServiceClient } from '$lib/protocol/client.svelte';
import { mockSocketFactory, type MockServer } from '$lib/protocol/mock/socket';
import type { MimoQuantity, MimoStatus, MimoSubscribePlot } from '$lib/protocol/mimo';
import { defaultMimoLayout, MIMO_QUANTITY_ORDER, mimoLayoutFromSaved, mimoLayoutToSaved, mimoSubscriptionFor, type MimoPageSpec } from '$lib/acquire/display/mimoLayout';
import { scrollWindow, togglePin } from '$lib/viz/matrixGrid';
import { COH_ERROR, gridSettings, pagePlots, type PageContext } from './pages';
import { MimoViewsState } from './mimoViews.svelte';

const labels = Array.from({ length: 72 }, (_, i) => `A${i + 1}`);
const drives = Array.from({ length: 24 }, (_, i) => `D${i + 1}`);
const ctx = (over: Partial<PageContext> = {}): PageContext => ({ labels, drive_labels: drives, space: 'physical', quantities: MIMO_QUANTITY_ORDER, widths: {}, pixelRatio: 1, ...over });
const page = (kind: MimoPageSpec['kind'], settings: Record<string, unknown> = {}): MimoPageSpec => ({ id: 'mp-9', name: 'P', kind, settings });
const wire = (p: MimoPageSpec, c = ctx()) => mimoSubscriptionFor(pagePlots(p, c));

describe('what each page subscribes to', () => {
	it('the grid: its window block and its window controls\' spectra, at the cell width', () => {
		const sub = wire(page('grid'), ctx({ widths: { cell: 110 } }));
		expect(sub[0]).toEqual({ id: 'mp-9:sdm', quantity: 'sdm', block: [0, 1, 2, 3, 4, 5, 6, 7], points: 110 });
		expect(sub.slice(1).map((p) => [p.quantity, (p as { index: number }).index, p.points])).toEqual([0, 1, 2, 3, 4, 5, 6, 7].map((i) => ['psd', i, 110]));
		expect(sub).toHaveLength(9);
		// nothing streams whole: no plot asks for more than the 8 by 8 block
		for (const p of sub) if ('block' in p) expect(p.block.length).toBeLessThanOrEqual(8);
	});

	it('the grid follows its window: scrolled, pinned, and a cell opened full size', () => {
		let w = gridSettings(page('grid')).window;
		w = togglePin(w, 'A50');
		w = scrollWindow(labels, w, 20);
		const p = page('grid', { window: w, open: { row: 49, col: 21 }, off: 'coh_phase' });
		const sub = wire(p, ctx({ widths: { cell: 100, full: 900 } }));
		expect((sub[0] as { block: number[] }).block).toEqual([49, 20, 21, 22, 23, 24, 25, 26]);
		expect(sub.at(-1)).toEqual({ id: 'mp-9:open', quantity: 'phase', pair: [49, 21], points: 900 });
		// A pinned control drawn first opens above the diagonal as coherence, whatever its position.
		const pinnedAbove = wire(page('grid', { window: w, open: { row: 49, col: 21, kind: 'coh' } }));
		expect(pinnedAbove.at(-1)).toMatchObject({ quantity: 'coh', pair: [49, 21] });
		const csd = wire(page('grid', { open: { row: 2, col: 5 }, off: 'csd' }));
		expect(csd.at(-1)).toMatchObject({ quantity: 'csd_mag', pair: [2, 5] });
		const diag = wire(page('grid', { open: { row: 3, col: 3 } }));
		expect(diag.at(-1)).toMatchObject({ quantity: 'psd', index: 3 });
		const coh = wire(page('grid', { open: { row: 1, col: 4 } }));
		expect(coh.at(-1)).toMatchObject({ quantity: 'coh', pair: [1, 4] });
	});

	it('the grid asks for nothing before the run has named its controls', () => {
		expect(wire(page('grid'), ctx({ labels: [] }))).toEqual([]);
	});

	it('the heatmap: every row, or the rows chosen by name; the coherence mode only when offered', () => {
		expect(wire(page('heatmap'), ctx({ widths: { main: 800 } }))).toEqual([{ id: 'mp-9:heat', quantity: 'error_db', rows: 'all', points: 800 }]);
		expect(wire(page('heatmap', { rows: ['A3', 'A70', 'nobody'] }))[0]).toMatchObject({ rows: [2, 69] });
		const without = MIMO_QUANTITY_ORDER.filter((q) => q !== COH_ERROR);
		expect(wire(page('heatmap', { mode: 'coherence' }), ctx({ quantities: without }))[0]).toMatchObject({ quantity: 'error_db' });
		// When the service offers the coherence error, the mode asks for it by that name, and the
		// request carries it (it was dropped before the quantity joined the selector table).
		const offered = pagePlots(page('heatmap', { mode: 'coherence' }), ctx({ quantities: [...MIMO_QUANTITY_ORDER, COH_ERROR as MimoQuantity] }));
		expect(offered[0].quantity).toBe(COH_ERROR);
		expect(mimoSubscriptionFor(offered)).toEqual([{ id: 'mp-9:heat', quantity: 'coh_error', rows: 'all', points: expect.any(Number) }]);
	});

	it('the drive and plant page: its drive, drive pair, response, singular values, condition, caps, and a chosen diagnostic', () => {
		const sub = wire(page('plant', { drive: 3, drivePair: [4, 5], frf: [10, 2], diag: 'regularization' }));
		expect(sub.map((p) => p.quantity)).toEqual(['drive_psd', 'drive_coh', 'frf', 'sv', 'cond', 'cap', 'law_diag']);
		expect(sub[0]).toMatchObject({ index: 3 });
		expect(sub[1]).toMatchObject({ pair: [4, 5] });
		expect(sub[2]).toMatchObject({ pair: [10, 2] });
		expect(sub[5]).toMatchObject({ diagonal: true });
		expect(sub[6]).toMatchObject({ key: 'regularization' });
		// one drive: no drive coherence; positions past the run are pulled in
		const one = wire(page('plant', { drive: 9, frf: [99, 9] }), ctx({ drive_labels: ['D1'] }));
		expect(one.map((p) => p.quantity)).toEqual(['drive_psd', 'frf', 'sv', 'cond', 'cap']);
		expect(one[0]).toMatchObject({ index: 0 });
		expect(one[1]).toMatchObject({ pair: [71, 0] });
	});

	it('the geometry: the degree-of-freedom rms under virtual control, nothing under physical', () => {
		expect(wire(page('geometry'), ctx({ space: 'virtual' }))).toEqual([{ id: 'mp-9:dof', quantity: 'dof_rms', points: 64 }]);
		expect(wire(page('geometry'))).toEqual([]);
	});
});

describe('the default layouts', () => {
	it('one monitor: the four views, opening on the error heatmap', () => {
		const d = defaultMimoLayout(1);
		expect(d.layout.pages.map((p) => [p.kind, p.name])).toEqual([
			['heatmap', 'Error Heatmap'],
			['grid', 'Matrix Grid'],
			['plant', 'Drive and Plant'],
			['geometry', 'Geometry']
		]);
		expect(d.windows).toEqual(['mp-1']);
	});

	it('two monitors: the grid in the main window, the heatmap in the second; up to four', () => {
		expect(defaultMimoLayout(2).windows).toEqual(['mp-2', 'mp-1']);
		expect(defaultMimoLayout(4).windows).toEqual(['mp-2', 'mp-1', 'mp-3', 'mp-4']);
		expect(defaultMimoLayout(9).windows).toHaveLength(4);
	});

	it('a layout saves and reads back checked', () => {
		const d = defaultMimoLayout(1).layout;
		d.pages[1].settings = { window: { pinned: ['A2'], offset: 3, size: 8 } };
		const back = mimoLayoutFromSaved(mimoLayoutToSaved(d))!;
		expect(back).toEqual(d);
		expect(mimoLayoutFromSaved(JSON.stringify({ version: 1, pages: [{ id: 'x-1', name: 'Bad', kind: 'nonsense' }] }))).toBeNull();
		expect(mimoLayoutFromSaved('not json')).toBeNull();
	});
});

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

function server() {
	const calls: Record<string, unknown>[] = [];
	const s: MockServer = {
		hello: { type: 'hello', protocol: 'sds/1', service: 'control', version: '0.1.0' },
		methods: {
			'mimo_random.status': () => status() as unknown as Record<string, unknown>,
			'mimo_random.subscribe': (params) => {
				calls.push(params);
				if (params.default === true) return { default: true, rate_hz: 5, plots: null, quantities: MIMO_QUANTITY_ORDER };
				return { default: false, rate_hz: params.rate_hz, plots: params.plots, quantities: MIMO_QUANTITY_ORDER };
			}
		}
	};
	return { s, calls };
}

const quantities = (c: Record<string, unknown> | undefined) => ((c?.plots as MimoSubscribePlot[] | undefined) ?? []).map((p) => p.quantity);

describe('one window of MIMO run views', () => {
	it('subscribes for the page on screen only, follows the page, and drops a removed page at once', async () => {
		const { s, calls } = server();
		const control = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(s, 1) });
		let showing = $state(true);
		const views = new MimoViewsState(control, { setupTitle: () => 'T', showing: () => showing, storage: null, session: null, events: null, subscribeDelayMs: 5, pixelRatio: 1 });
		const stop = views.watch();
		control.connect();
		await until(() => views.status !== null);
		// the heatmap first, on one monitor
		await until(() => quantities(calls.at(-1)).includes('error_db'));
		expect(quantities(calls.at(-1))).toEqual(['error_db']);

		// the grid: its block and its 8 spectra, nothing of the heatmap
		views.selectPage('mp-2');
		await until(() => quantities(calls.at(-1))[0] === 'sdm');
		expect(quantities(calls.at(-1))).toEqual(['sdm', ...Array(8).fill('psd')]);

		// a selection change in the grid changes the subscription
		views.patch('mp-2', { window: { pinned: ['A12'], offset: 0, size: 8 } });
		await until(() => ((calls.at(-1)?.plots as { block?: number[] }[])?.[0]?.block ?? [])[0] === 11);
		expect((calls.at(-1)!.plots as { block: number[] }[])[0].block).toEqual([11, 0, 1, 2, 3, 4, 5, 6]);

		// the page on screen removed: the window shows its neighbour and asks for that instead
		views.removePage('mp-2');
		await until(() => quantities(calls.at(-1))[0] === 'error_db');
		expect(views.pageId).toBe('mp-1');
		expect(JSON.stringify(calls.at(-1))).not.toContain('mp-2:');

		// a page added during the run is subscribed when shown
		const id = views.addPage('plant')!;
		await until(() => quantities(calls.at(-1))[0] === 'drive_psd');
		expect(((calls.at(-1)!.plots as MimoSubscribePlot[]) ?? []).every((p) => p.id.startsWith(`${id}:`))).toBe(true);

		// hidden: nothing is asked for
		showing = false;
		await until(() => calls.at(-1)?.default === true);
		stop();
		control.close();
	});

	it('a heatmap row opens its control in the grid, adding a grid page if there is none', async () => {
		const { s } = server();
		const control = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(s, 1) });
		const views = new MimoViewsState(control, { setupTitle: () => 'T', showing: () => true, storage: null, session: null, events: null, subscribeDelayMs: 5, pixelRatio: 1 });
		views.status = status(72, 24);
		views.openInGrid(40);
		expect(views.page.kind).toBe('grid');
		expect((pagePlots(views.page, views.context(views.page.id))[0].selector as { block: number[] }).block).toContain(40);
		views.removePage('mp-2');
		views.selectPage('mp-1');
		views.openInGrid(5);
		expect(views.page.kind).toBe('grid');
		expect(views.page.name).toBe('Matrix Grid');
	});

	it('two windows share one layout through storage, each on its own page', async () => {
		const store = new Map<string, string>();
		const storage = { getItem: (k: string) => store.get(k) ?? null, setItem: (k: string, v: string) => void store.set(k, v) } as unknown as Storage;
		const events = new EventTarget();
		const { s } = server();
		const c1 = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(s, 1) });
		const c2 = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(s, 1) });
		const a = new MimoViewsState(c1, { setupTitle: () => 'T', showing: () => true, storage, session: null, events: null, monitors: 2, windowIndex: 0 });
		const b = new MimoViewsState(c2, { setupTitle: () => 'T', showing: () => true, storage, session: null, events, monitors: 2, windowIndex: 1 });
		const sa = a.watch();
		const sb = b.watch();
		await until(() => a.key !== '' && b.key !== '');
		expect(a.page.kind).toBe('grid');
		expect(b.page.kind).toBe('heatmap');
		a.renamePage('mp-1', 'Overview');
		await new Promise((r) => setTimeout(r, 10));
		const ev = new Event('storage');
		Object.assign(ev, { key: a.key });
		events.dispatchEvent(ev);
		await until(() => b.page.name === 'Overview');
		expect(b.pageId).toBe('mp-1');
		sa();
		sb();
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
