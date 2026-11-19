/**
 * The MIMO display's protocol plumbing: plots by quantity and selector, the subscription a window
 * sends for them, and one window's feed against a mock control service through the real client
 * and codec. No screens and no plots are drawn here.
 */
import { describe, expect, it } from 'vitest';
import { ServiceClient } from '$lib/protocol/client.svelte';
import { MockError, mockSocketFactory, type MockConnection, type MockServer } from '$lib/protocol/mock/socket';
import type { MimoSubscribePlot } from '$lib/protocol/mimo';
import type { HeatmapView } from '$lib/protocol/types';
import { MimoFeed } from './mimoFeed.svelte';
import {
	gridWindow,
	mimoPlotTitle,
	mimoSubscriptionFor,
	MIMO_QUANTITY_ORDER,
	newMimoPlot,
	pairsOf,
	pointsForWidth,
	readMimoPlot,
	SELECTOR_KINDS,
	selectorProblem,
	selectorWire,
	type MimoPlotSpec
} from './mimoLayout';

const until = async (cond: () => boolean, ms = 3000) => {
	const t0 = Date.now();
	while (!cond()) {
		if (Date.now() - t0 > ms) throw new Error('timed out waiting');
		await new Promise((r) => setTimeout(r, 5));
	}
};

describe('MIMO plots and selectors', () => {
	it('every quantity starts with a selector the service takes', () => {
		for (const q of MIMO_QUANTITY_ORDER) {
			const p = newMimoPlot('p', q);
			expect(selectorProblem(q, p.selector), q).toBeNull();
			expect(SELECTOR_KINDS[q]).toContain(p.selector.kind);
		}
	});

	it('pair, row and diagonal selectors reach the wire in the service form', () => {
		expect(selectorWire({ kind: 'pair', pair: [3, 7] })).toEqual({ pair: [3, 7] });
		expect(selectorWire({ kind: 'rows', rows: 'all' })).toEqual({ rows: 'all' });
		expect(selectorWire({ kind: 'rows', rows: [70, 71] })).toEqual({ rows: [70, 71] });
		expect(selectorWire({ kind: 'diagonal' })).toEqual({ diagonal: true });
		expect(selectorWire({ kind: 'none' })).toEqual({});
		expect(selectorWire({ kind: 'key', key: 'condition', index: null })).toEqual({ key: 'condition' });
		expect(selectorWire({ kind: 'key', key: 'held', index: 4 })).toEqual({ key: 'held', index: 4 });
	});

	it('a selector the quantity does not take is named, and the plot is left out of the subscription', () => {
		expect(selectorProblem('coh', { kind: 'pair', pair: [2, 2] })).toBe('Choose two different channels.');
		expect(selectorProblem('frf', { kind: 'pair', pair: [2, 2] })).toBeNull();
		expect(selectorProblem('coh', { kind: 'index', index: 0 })).toBe('Coherence takes pair.');
		expect(selectorProblem('trace', { kind: 'index', index: 0 })).toMatch(/takes no channel choice/);
		expect(selectorProblem('sdm', { kind: 'block', block: [0, 1, 2, 3, 4, 5, 6, 7, 8] })).toMatch(/1 to 8/);
		expect(selectorProblem('sdm', { kind: 'block', block: [1, 1] })).toBe('Choose each control once.');
		expect(selectorProblem('error_db', { kind: 'rows', rows: [] })).toMatch(/at least one/);
		const plots: MimoPlotSpec[] = [
			newMimoPlot('a', 'coh', { kind: 'pair', pair: [0, 1] }),
			newMimoPlot('bad', 'coh', { kind: 'pair', pair: [4, 4] }),
			{ ...newMimoPlot('h', 'error_db'), points: 512 },
			newMimoPlot('d', 'psd', { kind: 'diagonal' }),
			{ ...newMimoPlot('g', 'sdm', { kind: 'block', block: gridWindow(64, 72) }), points: 20000 }
		];
		const sub: MimoSubscribePlot[] = mimoSubscriptionFor(plots);
		expect(sub).toEqual([
			{ id: 'a', quantity: 'coh', pair: [0, 1], points: 1024 },
			{ id: 'h', quantity: 'error_db', rows: 'all', points: 512 },
			{ id: 'd', quantity: 'psd', diagonal: true, points: 1024 },
			{ id: 'g', quantity: 'sdm', block: [64, 65, 66, 67, 68, 69, 70, 71], points: 8192 }
		]);
	});

	it('a grid window and its pairs, and a plot width in points', () => {
		expect(gridWindow(0, 4)).toEqual([0, 1, 2, 3]);
		expect(gridWindow(70, 72)).toEqual([70, 71]);
		expect(pairsOf([0, 1, 2])).toEqual([
			[0, 1],
			[0, 2],
			[1, 2]
		]);
		expect(pairsOf(gridWindow(0, 72))).toHaveLength(28);
		expect(pointsForWidth(400, 2)).toBe(800);
		expect(pointsForWidth(10)).toBe(64);
	});

	it('titles name what the selector picks, and a saved plot reads back checked', () => {
		const labels = ['Control X', 'Control Y'];
		const drives = ['Shaker X', 'Shaker Y'];
		expect(mimoPlotTitle(newMimoPlot('a', 'coh'), labels, drives)).toBe('Coherence: Control X and Control Y');
		expect(mimoPlotTitle(newMimoPlot('a', 'frf', { kind: 'pair', pair: [1, 0] }), labels, drives)).toBe('Frequency Response: Control Y from Shaker X');
		expect(mimoPlotTitle(newMimoPlot('a', 'cap'), labels, drives)).toBe('Drive Caps: all drives');
		expect(mimoPlotTitle(newMimoPlot('a', 'error_db'), labels, drives)).toBe('Error Heatmap');
		const back = readMimoPlot(JSON.parse(JSON.stringify(newMimoPlot('p', 'sdm', { kind: 'block', block: [0, 2] }))));
		expect(back?.selector).toEqual({ kind: 'block', block: [0, 2] });
		// A selector the quantity does not take falls back to the quantity's own.
		expect(readMimoPlot({ id: 'p', quantity: 'coh', selector: { kind: 'diagonal' } })?.selector).toEqual({ kind: 'pair', pair: [0, 1] });
		expect(readMimoPlot({ id: 'p', quantity: 'nonsense' })).toBeNull();
	});
});

/** A control service that answers `mimo_random.subscribe` as the real one does, and sends one frame. */
function mimoServer() {
	const calls: Record<string, unknown>[] = [];
	const conns = new Set<MockConnection>();
	const server: MockServer = {
		hello: { type: 'hello', protocol: 'sds/1', service: 'control', version: '0.1.0' },
		attach: (c) => void conns.add(c),
		detach: (c) => void conns.delete(c),
		methods: {
			'mimo_random.subscribe': (params, conn) => {
				calls.push(params);
				if (params.default === true) return { default: true, rate_hz: 5, plots: null, quantities: MIMO_QUANTITY_ORDER };
				const plots = params.plots as MimoSubscribePlot[];
				if (plots.some((p) => p.quantity === 'coh' && 'pair' in p && p.pair[0] === p.pair[1])) {
					throw new MockError('bad_request', 'Coherence is between two different channels; choose two.', 'bad_params', { param: 'plots[0].pair' });
				}
				const heat: HeatmapView = {
					view: 'heatmap',
					title: 'Control Error',
					rows: { label: 'Control', ticks: ['Control X', 'Control Y'] },
					x: { label: 'Frequency', unit: 'Hz', scale: 'log' },
					x_values: new Float64Array([100, 200, 400]),
					values: new Float32Array([0.5, -0.25, 0.125, 0, -0.5, 0.25]),
					value: { label: 'Error', unit: 'dB' },
					row_bands: [
						{ alarm_db: [3, 3], abort_db: [6, 6] },
						{ alarm_db: [3, 3], abort_db: [6, 6] }
					]
				};
				setTimeout(() =>
					conn.emit('mimo_random.data', {
						run_id: 'm-0001',
						source: 'SIM',
						state: 'running',
						phase: 'full_level',
						loop: 12,
						elapsed_s: 30,
						level_db: 0,
						plots: plots.map((p) => ({ id: p.id, quantity: p.quantity, selector: {}, labels: ['Control X', 'Control Y'], views: p.quantity === 'error_db' ? [heat] : [] }))
					})
				);
				return { default: false, rate_hz: params.rate_hz ?? 5, plots, quantities: MIMO_QUANTITY_ORDER };
			}
		}
	};
	return { server, calls };
}

describe('one window\'s MIMO feed', () => {
	it('subscribes for the plots on screen, follows changes, and drops to the default when none', async () => {
		const { server, calls } = mimoServer();
		const control = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(server, 1) });
		let plots = $state.raw<MimoPlotSpec[]>([newMimoPlot('heat', 'error_db'), newMimoPlot('c01', 'coh')]);
		let rate = $state(5);
		const feed = new MimoFeed(control, { plots: () => plots, rate: () => rate, subscribeDelayMs: 5 });
		const stop = feed.watch();
		control.connect();
		await until(() => feed.accepted !== null && feed.data !== null);
		expect(calls[0]).toEqual({
			rate_hz: 5,
			plots: [
				{ id: 'heat', quantity: 'error_db', rows: 'all', points: 1024 },
				{ id: 'c01', quantity: 'coh', pair: [0, 1], points: 1024 }
			]
		});
		const heat = feed.plotData('heat')!.views[0] as HeatmapView;
		expect(heat.values).toBeInstanceOf(Float32Array);
		expect(Array.from(heat.x_values)).toEqual([100, 200, 400]);

		// A changed pair and rate: one new request with the latest.
		plots = [newMimoPlot('c01', 'coh', { kind: 'pair', pair: [2, 5] })];
		rate = 10;
		await until(() => calls.length === 2);
		expect(calls[1]).toEqual({ rate_hz: 10, plots: [{ id: 'c01', quantity: 'coh', pair: [2, 5], points: 1024 }] });

		// Nothing on screen: back to the default, and the last frame is let go.
		plots = [];
		await until(() => calls.length === 3);
		expect(calls[2]).toEqual({ default: true });
		await until(() => feed.accepted?.default === true);
		expect(feed.data).toBeNull();
		stop();
		control.close();
	});

	it('a refused subscription is said in words and not asked again until something changes', async () => {
		const { server, calls } = mimoServer();
		const control = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(server, 1) });
		// A plot the shell itself would leave out never reaches the service ...
		let plots = $state.raw<MimoPlotSpec[]>([newMimoPlot('bad', 'coh', { kind: 'pair', pair: [1, 1] })]);
		const feed = new MimoFeed(control, { plots: () => plots, rate: () => 5, subscribeDelayMs: 5 });
		const stop = feed.watch();
		control.connect();
		await until(() => calls.length === 1);
		expect(calls[0]).toEqual({ default: true });
		// ... and a refusal from the service is shown as its words.
		server.methods['mimo_random.subscribe'] = () => {
			calls.push({});
			throw new MockError('bad_request', 'A plot takes one selector.', 'bad_params', { param: 'plots[0].index' });
		};
		plots = [newMimoPlot('a', 'trace')];
		await until(() => feed.error !== null);
		expect(feed.error).toMatch(/one selector/);
		const n = calls.length;
		await new Promise((r) => setTimeout(r, 50));
		expect(calls.length).toBe(n);
		stop();
		control.close();
	});
});
