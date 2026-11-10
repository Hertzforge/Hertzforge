/**
 * The live display: its layout (pages, plots, saved per setup), how plots present the service's
 * views, the level card's words, and one window's subscription against the mock control service,
 * through the real client and codec.
 */
import { describe, expect, it } from 'vitest';
import { ServiceClient } from '$lib/protocol/client.svelte';
import { createControlServer } from '$lib/protocol/mock/control';
import { mockSocketFactory, type MockServer } from '$lib/protocol/mock/socket';
import type { DataPlot, RandomLiveHead, RandomState, SpectrumView } from '$lib/protocol/types';
import { defaultSetup } from '../setup';
import { AcquireState } from '../store.svelte';
import { levelStatus, ramping } from '../words';
import { screenKey, toSaved as toSavedScreen } from '../screen/layout';
import { DisplayState } from './display.svelte';
import { defaultRandomLayout, fromSaved, layoutKey, newPlot, nextId, plotTitle, readLayout, subscriptionFor, toSaved } from './layout';
import { fixedY, overlay, presented, withScales } from './views';

/** The effects run (a microtask), but nothing queued as a task, such as another window's event. */
const effects = async () => {
	await Promise.resolve();
	await Promise.resolve();
};

const until = async (cond: () => boolean, ms = 8000) => {
	const t0 = Date.now();
	while (!cond()) {
		if (Date.now() - t0 > ms) throw new Error('timed out waiting');
		await new Promise((r) => setTimeout(r, 5));
	}
};

function memoryStorage(): Storage {
	const m = new Map<string, string>();
	return {
		get length() {
			return m.size;
		},
		clear: () => m.clear(),
		getItem: (k) => m.get(k) ?? null,
		key: (i) => [...m.keys()][i] ?? null,
		removeItem: (k) => void m.delete(k),
		setItem: (k, v) => void m.set(k, v)
	};
}

/**
 * One browser's local storage, shared by its windows: a write reaches every other window a moment
 * later as a storage event, as a browser delivers it, and never the window that wrote it.
 */
function sharedStorage() {
	const m = new Map<string, string>();
	const targets: EventTarget[] = [];
	const writes = new Map<string, number>();
	return {
		/** How many times `key` has been written, by any window. */
		writes: (key: string) => writes.get(key) ?? 0,
		window(): { storage: Storage; events: EventTarget } {
			const events = new EventTarget();
			targets.push(events);
			const storage: Storage = {
				get length() {
					return m.size;
				},
				clear: () => m.clear(),
				getItem: (k) => m.get(k) ?? null,
				key: (i) => [...m.keys()][i] ?? null,
				removeItem: (k) => void m.delete(k),
				setItem: (k, v) => {
					const old = m.get(k) ?? null;
					if (old === v) return;
					m.set(k, v);
					writes.set(k, (writes.get(k) ?? 0) + 1);
					for (const t of targets) {
						if (t !== events) setTimeout(() => t.dispatchEvent(Object.assign(new Event('storage'), { key: k, oldValue: old, newValue: v })), 0);
					}
				}
			};
			return { storage, events };
		}
	};
}

const spectrum = (title: string, names: [string, 'measured' | 'limit'][]): SpectrumView => ({
	view: 'spectrum',
	title,
	x: { label: 'Frequency', unit: 'Hz', scale: 'log' },
	y: { label: 'PSD', unit: 'g²/Hz', scale: 'log' },
	series: names.map(([name, role]) => ({ name, role, x: Float64Array.of(10, 20), y: Float64Array.of(1, 2) })),
	link: 'frequency'
});

describe('the layout', () => {
	it('starts Random on the Control, Limits and Responses pages', () => {
		const l = defaultRandomLayout();
		expect(l.pages.map((p) => p.name)).toEqual(['Control', 'Limits', 'Responses']);
		expect(l.pages[0].plots.map((p) => p.quantity)).toEqual(['control', 'drive']);
		expect(l.pages[1].plots.map((p) => [p.quantity, p.channels])).toEqual([['response', 'limited']]);
		expect(l.pages[2].plots.map((p) => [p.quantity, p.channels])).toEqual([['response', 'all']]);
	});

	it('asks for channels only on the quantities that have them, and points only for a time history', () => {
		const plots = [newPlot('a', 'control'), newPlot('b', 'response', [3, 1]), newPlot('c', 'time', 'control'), newPlot('d', 'readings')];
		expect(subscriptionFor(plots)).toEqual([
			{ id: 'a', quantity: 'control' },
			{ id: 'b', quantity: 'response', channels: [3, 1] },
			{ id: 'c', quantity: 'time', channels: 'control', points: 1024 },
			{ id: 'd', quantity: 'readings' }
		]);
		expect(plotTitle(plots[1])).toBe('Response Spectra: channels 3, 1');
	});

	it('reads back what it saved, and refuses what is not a layout', () => {
		const l = defaultRandomLayout();
		l.pages[1].name = 'Fixture Limits';
		l.pages[1].plots[0].y = 'linear';
		l.pages[1].plots[0].yMin = 1e-6;
		l.pages[1].plots[0].yMax = 1;
		expect(fromSaved(toSaved(l))).toEqual(l);
		// One saved form, whatever order the fields were set in: a layout read back saves the same.
		expect(toSaved(fromSaved(toSaved(defaultRandomLayout()))!)).toBe(toSaved(defaultRandomLayout()));
		expect(readLayout({ version: 2, pages: [] })).toBeNull();
		expect(fromSaved('{not json')).toBeNull();
		// Duplicate ids, unknown quantities and empty names are dropped, not trusted.
		const odd = readLayout({
			version: 1,
			pages: [
				{ id: 'p', name: '  ', plots: [] },
				{ id: 'q', name: 'Ok', plots: [{ id: 'x', quantity: 'control' }, { id: 'x', quantity: 'drive' }, { id: 'y', quantity: 'sound' }] }
			]
		});
		expect(odd?.pages.map((p) => [p.name, p.plots.map((x) => x.id)])).toEqual([['Ok', ['x']]]);
	});

	it('keeps a layout per test type and setup, and numbers new pages and plots past the old ones', () => {
		expect(layoutKey('random', '  Qualification   Random ')).toBe(layoutKey('random', 'qualification random'));
		expect(layoutKey('random', '')).toMatch(/untitled$/);
		// Numbers are never given twice: a plot removed does not hand its number to the next.
		const l = defaultRandomLayout();
		expect(nextId(l, 'pl')).toBe('pl-5');
		l.pages[0].plots.push(newPlot('pl-5', 'time'));
		l.pages[0].plots.pop();
		expect(nextId(l, 'pl')).toBe('pl-6');
		expect(nextId(l, 'pg')).toBe('pg-7');
		expect(readLayout(JSON.parse(toSaved(l)))?.serial).toBe(7);
	});
});

describe('presenting a plot', () => {
	it("puts the operator's axis scales on the service's view, and keeps the service's on auto", () => {
		const v = spectrum('Control', [['Control', 'measured']]);
		expect(withScales(v, { x: 'auto', y: 'auto' })).toBe(v);
		const lin = withScales(v, { x: 'linear', y: 'auto' });
		expect([lin.x.scale, lin.y.scale]).toEqual(['linear', 'log']);
		expect(lin.series).toBe(v.series);
	});

	it('draws several channels one pane each, or on one pane with every line named by channel', () => {
		const data: DataPlot = {
			id: 'r',
			quantity: 'response',
			channels: [1, 2],
			views: [spectrum('Channel 1: Control', [['Channel 1', 'measured']]), spectrum('Channel 2: Mount', [['Channel 2', 'measured'], ['Limit', 'limit']])]
		};
		const split = newPlot('r', 'response');
		expect(presented(split, data)).toHaveLength(2);
		const over = { ...split, arrange: 'overlay' as const };
		const one = overlay(over, data);
		expect(one?.series.map((s) => s.name)).toEqual(['Channel 1', 'Channel 2', 'Limit, channel 2']);
		expect(presented(over, data)).toHaveLength(1);
	});

	it('uses a fixed y range only when it is a range the axis can draw', () => {
		const v = spectrum('Control', [['Control', 'measured']]);
		expect(fixedY({ y: 'auto', yMin: 1e-6, yMax: 1 }, v)).toEqual([1e-6, 1]);
		expect(fixedY({ y: 'auto', yMin: 0, yMax: 1 }, v)).toBeNull();
		expect(fixedY({ y: 'auto', yMin: 2, yMax: 1 }, v)).toBeNull();
		expect(fixedY({ y: 'auto', yMin: null, yMax: 1 }, v)).toBeNull();
	});
});

describe('the level card status', () => {
	const run = (over: Partial<RandomState>): RandomState => ({
		run_id: 'r-1',
		state: 'running',
		phase: 'engaged',
		title: 'T',
		source: 'SIM',
		station: null,
		grid: null,
		clock: null,
		mode: 'auto',
		levels_db: [-12, -6, 0],
		level_db: -6,
		full_level_s: 60,
		elapsed_s: 10,
		at_level_s: 3,
		remaining_at_level_s: 27,
		alarm: false,
		feasibility: null,
		end: null,
		...over
	});
	const frame = (level: number | null, commanded: number | null): RandomLiveHead =>
		({ run_id: 'r-1', level_db: level, commanded_db: commanded }) as unknown as RandomLiveHead;

	it('says in plain words what the run is doing', () => {
		expect(levelStatus(run({}), frame(-6, -6)).text).toBe('Controlling at -6.0 dB');
		expect(levelStatus(run({ level_db: 0 }), frame(-3.2, 0)).text).toBe('Ramping to 0.0 dB');
		expect(levelStatus(run({ phase: 'full_level', level_db: 0 }), frame(0, 0)).text).toBe('Controlling at full level, 0.0 dB');
		expect(levelStatus(run({ state: 'review', phase: 'awaiting_review' }), null).text).toBe('Holding for review');
		expect(levelStatus(run({ state: 'stopping', phase: 'ramping_down' }), frame(-20, -46)).text).toBe('Stopping, ramping down from -20.0 dB');
		expect(levelStatus(run({ phase: 'sysid', level_db: -20 }), null).text).toMatch(/^Identifying the system at -20\.0 dB/);
		const ended = levelStatus(run({ state: 'finished', phase: 'aborted', end: { outcome: 'aborted', message: 'x', at_s: 1, level_db: 0, trip: null } }), null);
		expect([ended.text, ended.tone, ended.mark]).toEqual(['Aborted by a Protection', 'fail', 'ended']);
	});

	it('reads a ramp from the commanded level, not from a phase', () => {
		expect(ramping(frame(-6, -6))).toBe(false);
		expect(ramping(frame(-5.9, -6))).toBe(true);
		expect(ramping(frame(null, -6))).toBe(false);
		// A frame of another run says nothing about this one.
		expect(levelStatus(run({}), { ...frame(-1, 0), run_id: 'r-0' }).text).toBe('Controlling at -6.0 dB');
	});
});

describe('one window against the control service', () => {
	/** A window; its plots are on screen while its store shows the Run step, as in the shell. */
	async function window_(server: MockServer, storage: Storage, page?: string) {
		const client = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(server, 1) });
		const acq = new AcquireState(client, { session: null, local: null, checkDelayMs: 0 });
		const showing = () => acq.tab === 'live';
		const display = new DisplayState(client, { setupTitle: () => acq.held?.config.title ?? null, showing, storage, session: null, page, subscribeDelayMs: 0 });
		const stop = display.watch();
		client.connect();
		await client.whenOpen();
		await acq.sync();
		return { client, acq, display, stop };
	}

	it('asks only for the plots on screen, gets them by id, and fills a plot added mid-run at once', async () => {
		const server = createControlServer({ speed: 20, timeoutS: 30 });
		const storage = memoryStorage();
		const a = await window_(server, storage);
		const sent: unknown[] = [];
		a.client.on('random.live', () => sent.push('live'));

		// Off screen: the numbers only.
		await until(() => a.display.accepted !== null);
		expect(a.display.accepted?.plots).toEqual([]);

		expect(await a.acq.claim()).toBe(true);
		const d = defaultSetup();
		d.acquisition = { fs_hz: 8192, n_pts: 1024, n_avg: 2 };
		d.schedule = { mode: 'manual', levels: [{ db: -6, dwell_s: 1 }, { db: 0, dwell_s: 0 }], full_level_s: 30 };
		d.control.review_after_sysid = false;
		a.acq.setup = d;
		expect(await a.acq.apply()).toBe(true);
		expect(await a.acq.start()).toBe(true);
		a.acq.tab = 'live';
		await until(() => a.display.accepted?.plots?.length === 2);
		expect(a.display.accepted?.plots?.map((p) => p.quantity)).toEqual(['control', 'drive']);
		await until(() => (a.display.plotData('pl-1')?.views[0] as SpectrumView | undefined)?.series.some((s) => s.name === 'Control') ?? false);
		// A subscribed window is sent its own frames, never the default `random.live`.
		const before = sent.length;
		await new Promise((r) => setTimeout(r, 400));
		expect(sent.length).toBe(before);
		// The store's numbers come from the same frames.
		expect(a.acq.frame?.control_rms_g).not.toBeNull();

		// A plot added during the run fills without waiting, and the run is untouched.
		const runId = a.acq.run?.run_id;
		const id = a.display.addPlot('time') as string;
		await until(() => (a.display.plotData(id)?.views.length ?? 0) > 0);
		expect(a.display.plotData(id)?.channels).toEqual([1]);
		a.display.removePanel(id);
		await until(() => a.display.accepted?.plots?.length === 2);
		expect(a.acq.run?.run_id).toBe(runId);
		expect(a.acq.run?.state).toBe('running');

		// A page with a response plot of limited channels: none are limited, so none come.
		a.display.selectPage('pg-2');
		await until(() => a.display.plotData('pl-3') !== null);
		expect(a.display.plotData('pl-3')?.channels).toEqual([]);

		// The layout was kept for this setup, and a second window opened on a page reads it back.
		a.display.renamePage('pg-2', 'Fixture Limits');
		const b = await window_(server, storage, 'pg-2');
		b.acq.tab = 'live';
		await until(() => b.display.page.name === 'Fixture Limits');
		await until(() => b.display.plotData('pl-3') !== null);
		// The second window is an observer, and its Abort ends the run.
		expect(b.acq.role).toBe('observer');
		expect(b.acq.actions.abort.enabled).toBe(true);
		expect(await b.acq.abort()).toBe(true);
		await until(() => a.acq.run?.state === 'finished');
		expect(a.acq.run?.end?.outcome).toBe('operator_abort');
		a.stop();
		b.stop();
	});

	it('keeps the last of several quick changes: back to the setup and straight back to the run', async () => {
		const server = createControlServer({ speed: 20 });
		const client = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(server, 1) });
		const acq = new AcquireState(client, { session: null, local: null, checkDelayMs: 0 });
		const display = new DisplayState(client, { setupTitle: () => null, showing: () => acq.tab === 'live', storage: memoryStorage(), session: null, subscribeDelayMs: 40 });
		const stop = display.watch();
		client.connect();
		await client.whenOpen();
		await until(() => display.accepted !== null);
		acq.tab = 'live';
		await until(() => display.accepted?.plots?.length === 2);
		// Within one debounce: away and back. The window shows its plots, so it must end asking for them.
		acq.tab = 'station';
		await new Promise((r) => setTimeout(r, 0));
		acq.tab = 'live';
		await new Promise((r) => setTimeout(r, 300));
		expect(display.accepted?.plots?.length).toBe(2);
		stop();
	});

	it('sends the latest subscription within a debounce or two, however fast the changes come', async () => {
		const server = createControlServer({ speed: 20 });
		const client = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(server, 1) });
		const sent: number[] = [];
		const request = client.request.bind(client);
		client.request = ((method: string, params?: Record<string, unknown>, options?: { timeoutMs?: number }) => {
			if (method === 'random.subscribe') sent.push(Date.now());
			return request(method, params, options);
		}) as typeof client.request;
		const display = new DisplayState(client, { setupTitle: () => null, showing: () => true, storage: memoryStorage(), session: null, events: null, subscribeDelayMs: 40 });
		const stop = display.watch();
		try {
			client.connect();
			await client.whenOpen();
			const id = display.addPlot('time') as string;
			const points = () => display.accepted?.plots?.find((p) => p.id === id)?.points;
			await until(() => points() === 1024);
			// A new length every 10 ms for 0.4 s, four to a debounce: the subscription must not wait
			// for the changes to stop.
			const t0 = Date.now();
			const from = sent.length;
			for (let i = 1; i <= 40; i++) {
				display.updatePlot(id, { points: 512 + 8 * i });
				await new Promise((r) => setTimeout(r, 10));
			}
			const during = sent.slice(from);
			expect(during.length).toBeGreaterThan(1);
			expect(during[0] - t0).toBeLessThan(150);
			// And the last change is the one the service ends on.
			display.updatePlot(id, { points: 2048 });
			await until(() => points() === 2048, 1000);
		} finally {
			stop();
		}
	});

	it('two windows sharing a layout: quick edits in one reach the other once, and each asks for its own page', async () => {
		const server = createControlServer({ speed: 20 });
		const shared = sharedStorage();
		const stops: (() => void)[] = [];
		const open = async (page?: string) => {
			const { storage, events } = shared.window();
			const client = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(server, 1) });
			const display = new DisplayState(client, { setupTitle: () => 'Bench', showing: () => true, storage, session: null, events, page, subscribeDelayMs: 40 });
			stops.push(display.watch());
			client.connect();
			await client.whenOpen();
			await until(() => display.accepted !== null);
			return display;
		};
		const ids = (d: DisplayState) => d.accepted?.plots?.map((p) => p.id).join() ?? null;
		try {
			const a = await open();
			const b = await open('pg-2');
			await new Promise((r) => setTimeout(r, 50));
			const key = screenKey('random', 'Bench');
			const before = shared.writes(key);
			// Two edits in a row, both written before the other window hears of the first (as a plot
			// added, then its quantity changed, are on screen): the plot, then its quantity.
			const id = a.addPlot('response') as string;
			await effects();
			a.updatePlot(id, { quantity: 'time' });
			await effects();
			// The editing window asks for its page with the new plot at once.
			await until(() => ids(a) === `pl-1,pl-2,${id}` && a.plot(id)?.quantity === 'time', 1000);
			await new Promise((r) => setTimeout(r, 150));
			// The two edits were written once each; neither window wrote back what it read.
			expect(shared.writes(key) - before).toBe(2);
			// Both end on the same layout, and the other window still asks for its own page only.
			expect(toSavedScreen(b.layout)).toBe(toSavedScreen(a.layout));
			expect(b.plot(id)?.quantity).toBe('time');
			expect(ids(b)).toBe('pl-3');
		} finally {
			for (const stop of stops) stop();
		}
	});

	it('refuses a subscription the service cannot read, and names the field', async () => {
		const server = createControlServer({ speed: 20 });
		const client = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(server, 1) });
		client.connect();
		await client.whenOpen();
		const refusal = (params: Record<string, unknown>) => client.request('random.subscribe', params).then(() => null, (e: { reason?: string; detail?: { param?: string } }) => [e.reason, e.detail?.param]);
		expect(await refusal({ plots: [{ id: 'a', quantity: 'sound' }] })).toEqual(['bad_params', 'plots[0].quantity']);
		expect(await refusal({ plots: [{ id: 'a', quantity: 'control', channels: [1] }] })).toEqual(['bad_params', 'plots[0].channels']);
		expect(await refusal({ plots: [{ id: 'a', quantity: 'drive' }, { id: 'a', quantity: 'control' }] })).toEqual(['bad_params', 'plots[1].id']);
		expect(await refusal({ rate_hz: 20, plots: [] })).toEqual(['bad_params', 'rate_hz']);
		expect(await client.request('random.subscribe', { default: true })).toMatchObject({ default: true, plots: null });
	});
});
