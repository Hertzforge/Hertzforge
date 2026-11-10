/** Explorer on the test screen's frame: its plot types, its first pages and a kept layout taken over. */
import { describe, expect, it } from 'vitest';
import { EXPLORER_PLOT_TYPES } from './explorerLayout';
import { EXPLORER_HOOKS } from './explorerViews.svelte';

const shape = (l: ReturnType<typeof EXPLORER_HOOKS.defaultLayout>) => l.pages.map((p) => [p.name, p.panels.map((x) => (x.kind === 'plot' ? x.plot.quantity : `card:${x.card}`))]);

describe('Explorer on the screen frame', () => {
	it('offers its four quantities, the spectra with the channel choices the service takes', () => {
		expect(EXPLORER_PLOT_TYPES.map((t) => [t.id, t.words, t.group, t.channels, t.time])).toEqual([
			['survey', 'Survey', 'frequency', false, false],
			['spectrum', 'Spectra', 'frequency', true, false],
			['time', 'Time History', 'time', false, true],
			['readings', 'Readings', 'numbers', false, false]
		]);
		expect(EXPLORER_PLOT_TYPES[1].selectors).toEqual(['all', 'control']);
		expect(EXPLORER_HOOKS.cards).toEqual(['run', 'level', 'steer', 'station']);
	});

	it('opens on its four pages, at ten frames a second', () => {
		const l = EXPLORER_HOOKS.defaultLayout(1.4);
		expect(shape(l)).toEqual([
			['Survey', ['survey']],
			['Spectra', ['spectrum']],
			['Time', ['time']],
			['Readings', ['readings']]
		]);
		expect(l.rate).toBe(10);
		// the survey is drawn on its own points: no point count is sent for it
		expect(EXPLORER_HOOKS.feed?.params(l.pages.flatMap((p) => p.panels.filter((x) => x.kind === 'plot')) as never, 10)).toMatchObject({
			plots: [{ quantity: 'survey' }, { quantity: 'spectrum', channels: 'all' }, { quantity: 'time', points: 1024 }, { quantity: 'readings' }]
		});
	});

	it('takes over a layout the page layer kept', () => {
		const l = EXPLORER_HOOKS.legacy?.read(JSON.stringify({ version: 1, rate: 2, serial: 2, pages: [{ id: 'xp-2', name: 'Peaks', kind: 'survey' }, { id: 'xp-1', name: 'All Spectra', kind: 'spectra' }] }));
		expect(l && shape(l)).toEqual([
			['Peaks', ['survey']],
			['All Spectra', ['spectrum']]
		]);
	});
});
