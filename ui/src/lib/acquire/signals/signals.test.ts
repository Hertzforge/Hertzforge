/**
 * Signals on a test screen: the plot type every test offers, what a window asks for its Signals
 * plots, and the screen's two feeds against the mock control service: Signals plots go to
 * `signals.subscribe`, the rest to the test's own feed, and both reach their plots by id.
 */
import { describe, expect, it } from 'vitest';
import { ServiceClient } from '$lib/protocol/client.svelte';
import { createControlServer } from '$lib/protocol/mock/control';
import { mockSocketFactory, type MockServer } from '$lib/protocol/mock/socket';
import type { SignalsData, SignalsSubscribeParams, SignalsSubscribeResult } from '$lib/protocol/signals';
import type { SpectrumView, SubscribePlot } from '$lib/protocol/types';
import { newPlotSpec } from '../plots/types';
import { defaultSetup } from '../setup';
import { AcquireState } from '../store.svelte';
import { DisplayState, RANDOM_SCREEN } from '../display/display.svelte';
import type { PlotPanel } from '../screen/screen.svelte';
import { isSignals, SIGNALS_FEED, SIGNALS_TYPE, signalsChoices, signalsEmpty, signalsHooks, signalsParams } from './signals';

const until = async (cond: () => boolean, ms = 8000) => {
	const t0 = Date.now();
	while (!cond()) {
		if (Date.now() - t0 > ms) throw new Error('timed out waiting');
		await new Promise((r) => setTimeout(r, 5));
	}
};

const panel = (id: string, patch: Partial<PlotPanel['plot']> = {}): PlotPanel => ({ id, kind: 'plot', at: { x: 0, y: 0, w: 1, h: 1 }, plot: { ...newPlotSpec(SIGNALS_TYPE), ...patch } });

/** Every request a method was sent, in order. */
function recorded(server: MockServer, methods: string[]) {
	const calls: { method: string; params: Record<string, unknown> }[] = [];
	for (const m of methods) {
		const fn = server.methods[m];
		server.methods[m] = (params, conn) => {
			calls.push({ method: m, params: structuredClone(params) });
			return fn(params, conn);
		};
	}
	return calls;
}

describe('the Signals plot type', () => {
	it('is offered by Random after its own plot types, as a time plot of any channel', () => {
		expect(RANDOM_SCREEN.types.at(-1)).toBe(SIGNALS_TYPE);
		expect(SIGNALS_TYPE).toMatchObject({ id: 'signals', words: 'Signals', help: 'Any input or drive channel against time.', group: 'time', channels: true, time: true });
		expect(RANDOM_SCREEN.others).toEqual([SIGNALS_FEED]);
		// Offered once, however often a screen's hooks are given it.
		const twice = signalsHooks(RANDOM_SCREEN);
		expect(twice.types.filter((t) => isSignals(t.id))).toHaveLength(1);
		expect(twice.others).toEqual([SIGNALS_FEED]);
	});

	it("asks for each plot's channels, window, points and arrangement, within the service's bounds", () => {
		const params = signalsParams(
			[
				panel('a'),
				panel('b', { channels: [4, 2], points: 20000, arrange: 'split', display: { ...newPlotSpec(SIGNALS_TYPE).display, window_s: 0.5 } }),
				panel('c', { channels: 'limited', display: { ...newPlotSpec(SIGNALS_TYPE).display, window_s: 60 } }),
				panel('d', { channels: 'control', points: 10 })
			],
			20
		);
		expect(params).toEqual({
			rate_hz: 10,
			plots: [
				{ id: 'a', channels: 'all', points: 1024, arrange: 'overlay' },
				{ id: 'b', channels: [4, 2], points: 8192, arrange: 'split', window_s: 0.5 },
				{ id: 'c', channels: 'all', points: 1024, arrange: 'overlay', window_s: 10 },
				{ id: 'd', channels: 'control', points: 64, arrange: 'overlay' }
			]
		} satisfies SignalsSubscribeParams);
		expect(signalsParams([], 0.1).rate_hz).toBe(0.5);
	});

	it("offers the run's channels to choose from, else the table's enabled rows with the drives", () => {
		const rows = [
			{ index: 1, enabled: true, description: 'Table', role: 'control' },
			{ index: 2, enabled: true, description: '', role: 'drive' },
			{ index: 3, enabled: false, description: 'Off', role: 'monitor' }
		];
		expect(signalsChoices(null, rows)).toEqual([
			{ index: 1, name: 'Table' },
			{ index: 2, name: 'drive' }
		]);
		expect(signalsChoices([{ index: 7, name: 'Head', unit: 'g', kind: 'input' }], rows)).toEqual([{ index: 7, name: 'Head' }]);
		expect(signalsEmpty(null)).toBe('Appears while a run acquires.');
		expect(signalsEmpty({ channels: [] })).toBe('None of the chosen channels is in the run.');
	});
});

describe("a screen's two feeds", () => {
	it('sends Signals plots to signals.subscribe and the rest to the test, and draws both by id', async () => {
		const server = createControlServer({ speed: 20, timeoutS: 30 });
		const calls = recorded(server, ['random.subscribe', 'signals.subscribe']);
		const client = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(server, 1) });
		const acq = new AcquireState(client, { session: null, local: null, checkDelayMs: 0 });
		const display = new DisplayState(client, { setupTitle: () => 'Bench', showing: () => true, storage: null, session: null, events: null, subscribeDelayMs: 0 });
		const stop = display.watch();
		client.connect();
		await client.whenOpen();
		await acq.sync();

		// A page without a Signals plot asks Random alone, and Signals nothing.
		await until(() => display.accepted !== null);
		expect(calls.filter((c) => c.method === 'signals.subscribe')).toEqual([]);

		// A Signals plot added: it goes to Signals, and Random's list leaves it out.
		const id = display.addPlot('signals') as string;
		expect(display.linkFor('signals')?.feed).toBe(SIGNALS_FEED);
		expect(display.linkFor('control')).toBe(display.main);
		await until(() => calls.some((c) => c.method === 'signals.subscribe'));
		const sig = calls.filter((c) => c.method === 'signals.subscribe').at(-1)!.params as unknown as SignalsSubscribeParams;
		expect(sig.plots).toEqual([{ id, channels: 'all', points: 1024, arrange: 'overlay' }]);
		await until(() => (display.others[0].accepted as SignalsSubscribeResult | null) !== null);
		expect((display.others[0].accepted as SignalsSubscribeResult).available).toBe(false);
		const random = calls.filter((c) => c.method === 'random.subscribe').at(-1)!.params as { plots: SubscribePlot[] };
		expect(random.plots.map((p) => p.id)).toEqual(['pl-1', 'pl-2']);
		expect(display.accepted?.plots?.map((p) => p.id)).toEqual(['pl-1', 'pl-2']);

		// A run: the test's plots and the Signals plot each fill from their own feed.
		expect(await acq.claim()).toBe(true);
		const d = defaultSetup();
		d.acquisition = { fs_hz: 8192, n_pts: 1024, n_avg: 2 };
		d.schedule = { mode: 'manual', levels: [{ db: -6, dwell_s: 1 }, { db: 0, dwell_s: 0 }], full_level_s: 30 };
		d.control.review_after_sysid = false;
		acq.setup = d;
		expect(await acq.apply()).toBe(true);
		expect(await acq.start()).toBe(true);
		await until(() => (display.frame(id)?.views.length ?? 0) > 0 && (display.frame(id)?.views[0] as SpectrumView).series.length > 0);
		await until(() => (display.frame('pl-1')?.views[0] as SpectrumView | undefined)?.series.some((s) => s.name === 'Control') ?? false);
		const views = display.frame(id)!.views as SpectrumView[];
		expect(display.frame(id)?.channels).toEqual([1, 2]);
		expect(views.map((v) => [v.title, v.y.unit, v.series.map((s) => [s.name, s.role])])).toEqual([
			['Channel 1: Control', 'g', [['Channel 1', 'measured']]],
			['Channel 2: Drive', 'V', [['Channel 2', 'drive']]]
		]);
		const signals = display.others[0].data as SignalsData;
		expect(signals.available).toBe(true);
		expect(signals.channels.map((c) => [c.index, c.kind])).toEqual([
			[1, 'input'],
			[2, 'drive']
		]);
		expect(display.error).toBeNull();

		// The last Signals plot removed: Signals is told, and the test's plots go on.
		display.removePanel(id);
		await until(() => (calls.filter((c) => c.method === 'signals.subscribe').at(-1)!.params as unknown as SignalsSubscribeParams).plots.length === 0);
		expect(display.frame('pl-1')).not.toBeNull();
		expect(await acq.abort()).toBe(true);
		stop();
	}, 20000);

	it('names a Signals subscription the service refused', async () => {
		const server = createControlServer({ speed: 20 });
		const client = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(server, 1) });
		const display = new DisplayState(client, { setupTitle: () => 'Bench', showing: () => true, storage: null, session: null, events: null, subscribeDelayMs: 0 });
		const stop = display.watch();
		client.connect();
		await client.whenOpen();
		const id = display.addPlot('signals') as string;
		display.updatePlot(id, { arrange: 'stacked' as never });
		await until(() => display.error !== null);
		expect(display.error).toMatch(/arrange/);
		stop();
	});
});
