/**
 * The pulse page layer on the test screen's frame: quantities as plot types, the first pages, a
 * layout kept by the page layer taken over, what a window asks for, and a window's screen asking
 * for the page it shows.
 */
import { describe, expect, it } from 'vitest';
import type { ServiceClient } from '$lib/protocol/client.svelte';
import type { SpectrumView, View } from '$lib/protocol/types';
import { fits } from '../screen/grid';
import { fromSaved, toSaved } from '../screen/layout';
import type { PlotPanel } from '../screen/screen.svelte';
import type { PulsePageSet } from './pulseLayout';
import { defaultPulseScreen, pulseFromSaved, pulseHooks, pulsePlotTypes, pulseScreenLayout, pulseSubscription, type PulseScreenSpec } from './pulseScreen';
import { PulseScreenState } from './pulseScreenState.svelte';

type K = 'main' | 'kin' | 'table' | 'spare';
type Q = 'wave' | 'vel' | 'resp' | 'gain' | 'readings';

const PAGES: PulsePageSet<K, Q> = {
	testType: 'demo',
	prefix: 'dp',
	kinds: ['main', 'kin', 'table', 'spare'],
	words: { main: 'Main', kin: 'Kinematics', table: 'Table', spare: 'Spare' },
	help: { main: '', kin: '', table: '', spare: '' },
	quantities: { main: ['wave', 'resp'], kin: ['vel', 'gain'], table: ['readings'], spare: ['resp'] },
	timeQuantities: ['wave', 'vel'],
	rates: [1, 2, 5],
	defaultRate: 2,
	maxPages: 12,
	defaults: ['main', 'kin', 'table'],
	channelQuantities: ['resp'],
	channels: { spare: 'limited' }
};

const SPEC: PulseScreenSpec<K, Q> = {
	pages: PAGES,
	plots: [
		['wave', { words: 'Wave', help: 'The wave.', group: 'time', families: ['measured', 'reference'] }],
		['vel', { words: 'Velocity', help: 'Its velocity.', group: 'time' }],
		['resp', { words: 'Response', help: 'Each input.', group: 'frequency' }],
		['gain', { words: 'Gain', help: 'The gain.', group: 'frequency' }],
		['readings', { words: 'Readings', help: 'The run in numbers.', group: 'numbers' }]
	],
	cards: ['run', 'level', 'rows', 'station'],
	pageCards: { table: ['rows'] },
	selectors: ['all', 'control']
};

const TYPES = pulsePlotTypes(SPEC);
const offer = { types: TYPES, cards: SPEC.cards };

describe('plot types', () => {
	it('offers every quantity, with channels, a point count and the channel choices where they apply', () => {
		expect(TYPES.map((t) => [t.id, t.words, t.group, t.channels, t.time])).toEqual([
			['wave', 'Wave', 'time', false, true],
			['vel', 'Velocity', 'time', false, true],
			['resp', 'Response', 'frequency', true, false],
			['gain', 'Gain', 'frequency', false, false],
			['readings', 'Readings', 'numbers', false, false]
		]);
		expect(TYPES.find((t) => t.id === 'resp')?.selectors).toEqual(['all', 'control']);
		expect(TYPES.find((t) => t.id === 'gain')?.selectors).toBeUndefined();
		expect(TYPES[0].families).toEqual(['measured', 'reference']);
	});
});

describe('first pages', () => {
	it('makes a page of each default kind, its quantities as plots tiled evenly and its cards on it', () => {
		const l = defaultPulseScreen(SPEC, TYPES, 1.4);
		expect(l.pages.map((p) => [p.id, p.name])).toEqual([
			['pg-1', 'Main'],
			['pg-2', 'Kinematics'],
			['pg-3', 'Table']
		]);
		expect(l.pages.map((p) => p.panels.map((x) => (x.kind === 'plot' ? x.plot.quantity : `card:${x.card}`)))).toEqual([['wave', 'resp'], ['vel', 'gain'], ['card:rows', 'readings']]);
		for (const p of l.pages) expect(fits(p.panels, p.cols, p.rows)).toBe(true);
		// two panels on a page wider than tall are stacked, as Arrange Evenly puts them
		expect(l.pages[0].panels.map((x) => [x.at.x, x.at.y, x.at.w, x.at.h])).toEqual([
			[0, 0, 12, 6],
			[0, 6, 12, 6]
		]);
		// a card on a page is not in the dock as well
		expect(l.dock.cards.map((c) => [c.kind, c.shown])).toEqual([
			['run', true],
			['level', true],
			['rows', false],
			['station', true]
		]);
		expect(l.rate).toBe(2);
		const ids = l.pages.flatMap((p) => [p.id, ...p.panels.map((x) => x.id)]);
		expect(new Set(ids).size).toBe(ids.length);
		expect(l.serial).toBeGreaterThanOrEqual(6);
		// what is kept reads back the same
		expect(fromSaved(toSaved(l), offer, 1.4)).toEqual(l);
	});

	it('asks for the channels a kind names, every input otherwise; a card goes on its first page only', () => {
		const l = pulseScreenLayout(SPEC, TYPES, [
			{ name: 'Spare', kind: 'spare' },
			{ name: 'Main', kind: 'main' },
			{ name: 'Table', kind: 'table' },
			{ name: 'Table 2', kind: 'table' }
		]);
		const resp = (i: number) => l.pages[i].panels.find((x): x is PlotPanel => x.kind === 'plot' && x.plot.quantity === 'resp')?.plot.channels;
		expect([resp(0), resp(1)]).toEqual(['limited', 'all']);
		expect(l.pages[3].panels.map((x) => x.kind)).toEqual(['plot']);
	});
});

describe('layouts kept by the page layer', () => {
	it('takes one over with its pages, names and rate', () => {
		const text = JSON.stringify({
			version: 1,
			rate: 5,
			serial: 4,
			pages: [
				{ id: 'dp-3', name: 'Kin First', kind: 'kin' },
				{ id: 'dp-1', name: 'Main', kind: 'main' },
				{ id: 'dp-4', name: 'More Main', kind: 'main' }
			]
		});
		const l = pulseFromSaved(SPEC, TYPES, text) as NonNullable<ReturnType<typeof pulseFromSaved>>;
		expect(l.pages.map((p) => [p.name, p.panels.map((x) => (x.kind === 'plot' ? x.plot.quantity : x.card))])).toEqual([
			['Kin First', ['vel', 'gain']],
			['Main', ['wave', 'resp']],
			['More Main', ['wave', 'resp']]
		]);
		expect(l.rate).toBe(5);
		expect(l.dock.cards.every((c) => c.shown)).toBe(true);
	});

	it('takes nothing that is not one, nor another type\'s', () => {
		expect(pulseFromSaved(SPEC, TYPES, null)).toBeNull();
		expect(pulseFromSaved(SPEC, TYPES, 'not json')).toBeNull();
		expect(pulseFromSaved(SPEC, TYPES, '{"version":1,"rate":2,"serial":1,"pages":[{"id":"x-1","name":"SRS","kind":"srs"}]}')).toBeNull();
		expect(pulseFromSaved(SPEC, TYPES, '{"version":2,"pages":[]}')).toBeNull();
	});

	it('is read from where the page layer kept it', () => {
		const hooks = pulseHooks(SPEC, { method: 'demo.subscribe', topic: 'demo.data' });
		expect(hooks.legacy?.key('Bench Run')).toBe('hertzforge.display.v1:demo:bench run');
		expect(hooks.legacy?.last).toBe('hertzforge.display.v1:demo:*last');
		expect(hooks.legacy?.read(null)).toBeNull();
	});
});

describe('what a window asks for', () => {
	const plot = (id: string, quantity: string, channels: PlotPanel['plot']['channels'] = 'all', points = 1024): PlotPanel => {
		const l = defaultPulseScreen(SPEC, TYPES);
		const base = l.pages[0].panels[0] as PlotPanel;
		return { ...base, id, plot: { ...base.plot, quantity, channels, points } };
	};

	it('asks for the default when it shows no plot', () => {
		expect(pulseSubscription(TYPES, [], 5)).toEqual({ default: true });
	});

	it('sends channels with the plots that take them and a point count with the time histories only', () => {
		expect(pulseSubscription(TYPES, [plot('pl-1', 'wave', 'all', 600), plot('pl-2', 'resp', [3, 1]), plot('pl-3', 'gain', [2]), plot('pl-4', 'readings')], 2)).toEqual({
			rate_hz: 2,
			plots: [
				{ id: 'pl-1', quantity: 'wave', points: 600 },
				{ id: 'pl-2', quantity: 'resp', channels: [3, 1] },
				{ id: 'pl-3', quantity: 'gain' },
				{ id: 'pl-4', quantity: 'readings' }
			]
		});
	});

	it('sends a named choice the service does not take for the quantity as every channel', () => {
		const sent = pulseSubscription(TYPES, [plot('pl-1', 'resp', 'limited'), plot('pl-2', 'resp', 'control')], 1) as { plots: { channels: unknown }[] };
		expect(sent.plots.map((p) => p.channels)).toEqual(['all', 'control']);
	});
});

/** A control service that answers every subscription and remembers what it was asked. */
function fakeControl() {
	const asked: { method: string; params: Record<string, unknown> }[] = [];
	const control = {
		status: 'open',
		on: () => () => {},
		request: async (method: string, params: Record<string, unknown>) => {
			asked.push({ method, params });
			return { default: params.default === true, rate_hz: params.rate_hz ?? 5, plots: params.plots ?? null, quantities: ['wave', 'vel', 'resp', 'gain', 'readings'] };
		}
	};
	return { control: control as unknown as ServiceClient, asked };
}

const until = async (cond: () => boolean, ms = 2000) => {
	const t0 = Date.now();
	while (!cond()) {
		if (Date.now() - t0 > ms) throw new Error('timed out');
		await new Promise((r) => setTimeout(r, 5));
	}
};

describe("a window's screen", () => {
	it('asks for exactly the plots on the page it shows, and nothing while it shows none', async () => {
		const { control, asked } = fakeControl();
		let showing = true;
		const hooks = pulseHooks(SPEC, { method: 'demo.subscribe', topic: 'demo.data' });
		const screen = new PulseScreenState(control, hooks, { setupTitle: () => 'Bench', showing: () => showing, storage: null, session: null, events: null, subscribeDelayMs: 0 });
		const stop = screen.watch();
		await until(() => asked.length === 1);
		const last = () => asked[asked.length - 1];
		expect(last().method).toBe('demo.subscribe');
		expect((last().params.plots as { quantity: string }[]).map((p) => p.quantity)).toEqual(['wave', 'resp']);
		screen.selectPage('pg-2');
		await until(() => asked.length === 2);
		expect(last().params).toEqual({ rate_hz: 2, plots: [{ id: 'pl-3', quantity: 'vel', points: 1024 }, { id: 'pl-4', quantity: 'gain' }] });
		screen.addPlot('readings');
		await until(() => asked.length === 3);
		expect((last().params.plots as { quantity: string }[]).map((p) => p.quantity)).toEqual(['vel', 'gain', 'readings']);
		expect(screen.quantities).toEqual(['wave', 'vel', 'resp', 'gain', 'readings']);
		showing = false;
		screen.selectPage('pg-3');
		await until(() => asked.length === 4);
		expect(last().params).toEqual({ default: true });
		stop();
	});

	it('brings forward the page holding a quantity', () => {
		const { control } = fakeControl();
		const screen = new PulseScreenState(control, pulseHooks(SPEC, { method: 'demo.subscribe', topic: 'demo.data' }), { setupTitle: () => null, showing: () => false, storage: null, session: null, events: null });
		expect(screen.pageWith('gain')).toBe('pg-2');
		expect(screen.bringForward('gain')).toBe(true);
		expect(screen.pageId).toBe('pg-2');
		expect(screen.bringForward('nothing')).toBe(false);
		expect(screen.pageId).toBe('pg-2');
	});

	it('draws a plot with what the test type lays over its views', () => {
		const { control } = fakeControl();
		const extra: View = { view: 'spectrum', title: 'Over', x: { label: 'x', unit: null, scale: 'linear' }, y: { label: 'y', unit: null, scale: 'linear' }, series: [] } as SpectrumView;
		const screen = new PulseScreenState(
			control,
			pulseHooks(SPEC, { method: 'demo.subscribe', topic: 'demo.data' }),
			{ setupTitle: () => null, showing: () => false, storage: null, session: null, events: null },
			(quantity, views) => (quantity === 'wave' ? [...views, extra] : views)
		);
		screen.data = { plots: [] };
		expect(screen.views('pl-1').map((v) => v.title)).toEqual(['Over']);
		expect(screen.views('pl-2')).toEqual([]);
	});
});
