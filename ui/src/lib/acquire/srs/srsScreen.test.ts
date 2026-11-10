/** SRS shock on the test screen's frame: its plot types, its first pages, a kept layout taken over, and the picked pulses laid over the SRS plot. */
import { describe, expect, it } from 'vitest';
import type { ServiceClient } from '$lib/protocol/client.svelte';
import type { Series, SpectrumView, View } from '$lib/protocol/types';
import type { PlotPanel } from '../screen/screen.svelte';
import { SRS_PLOT_TYPES } from './srsLayout';
import { SRS_HOOKS, SrsViewsState } from './srsViews.svelte';

const shape = (l: ReturnType<typeof SRS_HOOKS.defaultLayout>) => l.pages.map((p) => [p.name, p.panels.map((x) => (x.kind === 'plot' ? x.plot.quantity : `card:${x.card}`))]);

describe('SRS shock on the screen frame', () => {
	it('offers every quantity it draws as a plot type; the pulse table is a card', () => {
		expect(SRS_PLOT_TYPES.map((t) => [t.id, t.group, t.time])).toEqual([
			['srs', 'frequency', false],
			['pulse', 'time', true],
			['velocity', 'time', true],
			['displacement', 'time', true],
			['identification', 'frequency', false],
			['components', 'frequency', false],
			['limit_channels', 'frequency', false],
			['limit_frf', 'frequency', false],
			['law', 'numbers', false],
			['readings', 'numbers', false]
		]);
		expect(SRS_PLOT_TYPES.every((t) => !t.channels)).toBe(true);
	});

	it('opens on the eight pages it had, the pulse table on the Pulses page', () => {
		const l = SRS_HOOKS.defaultLayout(1.4);
		expect(shape(l)).toEqual([
			['SRS', ['srs']],
			['Pulse', ['pulse']],
			['Kinematics', ['velocity', 'displacement']],
			['Identification', ['identification']],
			['Limit Channels', ['limit_channels', 'limit_frf']],
			['Components', ['components']],
			['Pulses', ['card:pulses', 'readings']],
			['Law', ['law', 'readings']]
		]);
		expect(SRS_HOOKS.feed?.method).toBe('srs.subscribe');
	});

	it('takes over a layout the page layer kept, and no classical shock one', () => {
		const l = SRS_HOOKS.legacy?.read(JSON.stringify({ version: 1, rate: 1, serial: 3, pages: [{ id: 'rp-5', name: 'Limits', kind: 'limit_channels' }] }));
		expect(l && shape(l)).toEqual([['Limits', ['limit_channels', 'limit_frf']]]);
		expect(SRS_HOOKS.legacy?.read('{"version":1,"rate":2,"serial":1,"pages":[{"id":"sp-1","name":"Spectrum","kind":"spectrum"}]}')).toBeNull();
	});

	it('lays the pulses picked in the table over the SRS plot', () => {
		const s = (name: string, role: Series['role']): Series => ({ name, role, x: Float64Array.from([10, 100]), y: Float64Array.from([1, 2]) });
		const srsView = (series: Series[]): SpectrumView => ({ view: 'spectrum', title: 'SRS', x: { label: 'Frequency', unit: 'Hz', scale: 'log' }, y: { label: 'SRS', unit: 'g', scale: 'log' }, series });
		const store = { lastViews: [] as View[], overlaid: [{ index: 0, views: [srsView([s('Measured', 'measured')])] }] };
		const control = { status: 'closed', on: () => () => {}, request: async () => ({}) } as unknown as ServiceClient;
		const views = new SrsViewsState(control, { setupTitle: () => null, showing: () => false, storage: null, session: null, events: null }, store);
		const id = (views.layout.pages[0].panels[0] as PlotPanel).id;
		views.data = { plots: [{ id, views: [srsView([s('Target', 'reference'), s('Measured', 'measured')])] }] } as never;
		expect((views.views(id)[0] as SpectrumView).series.map((x) => x.name)).toEqual(['Target', 'Measured', 'Pulse 1, measured']);
	});
});
