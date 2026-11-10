/**
 * Classical shock on the test screen's frame: its plot types, its first pages with the pulse table
 * on the Pulses page, a kept layout taken over, what a window asks for, and the picked pulses laid
 * over the Pulse plot.
 */
import { describe, expect, it } from 'vitest';
import type { ServiceClient } from '$lib/protocol/client.svelte';
import type { Series, SpectrumView, View } from '$lib/protocol/types';
import type { PlotPanel } from '../screen/screen.svelte';
import { SHOCK_PLOT_TYPES } from './shockLayout';
import { SHOCK_HOOKS, ShockViewsState } from './shockViews.svelte';

const shape = (l: ReturnType<typeof SHOCK_HOOKS.defaultLayout>) => l.pages.map((p) => [p.name, p.panels.map((x) => (x.kind === 'plot' ? x.plot.quantity : `card:${x.card}`))]);
const plots = (l: ReturnType<typeof SHOCK_HOOKS.defaultLayout>, i: number) => l.pages[i].panels.filter((x): x is PlotPanel => x.kind === 'plot');

describe('classical shock on the screen frame', () => {
	it('offers every quantity it draws as a plot type; the pulse table is a card', () => {
		expect(SHOCK_PLOT_TYPES.map((t) => [t.id, t.group, t.channels, t.time])).toEqual([
			['pulse', 'time', false, true],
			['velocity', 'time', false, true],
			['displacement', 'time', false, true],
			['srs', 'frequency', false, false],
			['spectrum', 'frequency', false, false],
			['identification', 'frequency', false, false],
			['law', 'numbers', false, false],
			['readings', 'numbers', false, false]
		]);
		expect(SHOCK_HOOKS.cards).toEqual(['run', 'level', 'pulses', 'station']);
	});

	it('opens on the six pages it had; the pulse table on the Pulses page and not in the dock', () => {
		const l = SHOCK_HOOKS.defaultLayout(1.4);
		expect(shape(l)).toEqual([
			['Pulse', ['pulse']],
			['SRS', ['srs']],
			['Kinematics', ['velocity', 'displacement']],
			['Identification', ['identification', 'spectrum']],
			['Pulses', ['card:pulses', 'readings']],
			['Law', ['law', 'readings']]
		]);
		expect(l.dock.cards.find((c) => c.kind === 'pulses')?.shown).toBe(false);
		expect(l.rate).toBe(2);
	});

	it('asks for its plots with a point count on the time histories only, or the default when it shows none', () => {
		const l = SHOCK_HOOKS.defaultLayout(1.4);
		expect(SHOCK_HOOKS.feed?.method).toBe('shock.subscribe');
		expect(SHOCK_HOOKS.feed?.params([...plots(l, 3), ...plots(l, 0)], 2)).toEqual({
			rate_hz: 2,
			plots: [
				{ id: plots(l, 3)[0].id, quantity: 'identification' },
				{ id: plots(l, 3)[1].id, quantity: 'spectrum' },
				{ id: plots(l, 0)[0].id, quantity: 'pulse', points: 1024 }
			]
		});
		expect(SHOCK_HOOKS.feed?.params([], 2)).toEqual({ default: true });
	});

	it('takes over a layout the page layer kept', () => {
		const text = JSON.stringify({ version: 1, rate: 5, serial: 7, pages: [{ id: 'sp-2', name: 'SRS', kind: 'srs' }, { id: 'sp-7', name: 'My Pulses', kind: 'pulses' }] });
		const l = SHOCK_HOOKS.legacy?.read(text);
		expect(l && shape(l)).toEqual([
			['SRS', ['srs']],
			['My Pulses', ['card:pulses', 'readings']]
		]);
		expect(l?.rate).toBe(5);
		// a classical shock layout's kinds are not another type's
		expect(SHOCK_HOOKS.legacy?.read('{"version":1,"rate":2,"serial":1,"pages":[{"id":"rp-5","name":"Limit Channels","kind":"limit_channels"}]}')).toBeNull();
	});

	it('lays the pulses picked in the table over the Pulse plot, named for each pulse', () => {
		const s = (name: string, role: Series['role']): Series => ({ name, role, x: Float64Array.from([0, 1]), y: Float64Array.from([0, 1]) });
		const view = (title: string, unit: string, series: Series[]): SpectrumView => ({ view: 'spectrum', title, x: { label: 'Time', unit, scale: 'linear' }, y: { label: 'Acceleration', unit: 'g', scale: 'linear' }, series });
		const latest: View[] = [view('Pulse', 'ms', [s('Reference', 'reference'), s('Measured', 'measured')]), view('Drive', 'ms', [s('Drive', 'drive')])];
		const picked = { index: 1, views: [view('Pulse', 'ms', [s('Measured', 'measured')])] };
		const store = { lastViews: latest, overlaid: [picked] };
		const control = { status: 'closed', on: () => () => {}, request: async () => ({}) } as unknown as ServiceClient;
		const views = new ShockViewsState(control, { setupTitle: () => null, showing: () => false, storage: null, session: null, events: null }, store);
		const pulse = plots(views.layout, 0)[0].id;
		const kin = plots(views.layout, 2)[0].id;
		// before the window's first frame, the last pulse's views stand in
		expect(views.views(pulse).map((v) => v.title)).toEqual(['Pulse', 'Drive']);
		expect((views.views(pulse)[0] as SpectrumView).series.map((x) => x.name)).toEqual(['Reference', 'Measured', 'Pulse 2, measured']);
		views.data = { plots: [{ id: kin, views: [view('Velocity', 'ms', [s('Measured', 'measured')])] }] } as never;
		expect(views.views(kin).map((v) => v.title)).toEqual(['Velocity']);
	});
});
