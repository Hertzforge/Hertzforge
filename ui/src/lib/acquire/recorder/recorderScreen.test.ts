/** The Recorder on the test screen's frame: its plot types, its first pages with the marks and the recordings as cards, and a kept layout taken over. */
import { describe, expect, it } from 'vitest';
import { RECORDER_PLOT_TYPES } from './recorderLayout';
import { RECORDER_HOOKS } from './recorderViews.svelte';

const shape = (l: ReturnType<typeof RECORDER_HOOKS.defaultLayout>) => l.pages.map((p) => [p.name, p.panels.map((x) => (x.kind === 'plot' ? x.plot.quantity : `card:${x.card}`))]);

describe('the Recorder on the screen frame', () => {
	it('offers its four quantities, the time and the spectra by row with every row the one named choice', () => {
		expect(RECORDER_PLOT_TYPES.map((t) => [t.id, t.group, t.channels, t.time, t.selectors ?? null])).toEqual([
			['time', 'time', true, true, ['all']],
			['spectrum', 'frequency', true, false, ['all']],
			['frf', 'frequency', false, false, null],
			['readings', 'numbers', false, false, null]
		]);
	});

	it('opens on its five pages: the marks with the readings, the recordings a page of their own', () => {
		const l = RECORDER_HOOKS.defaultLayout(1.4);
		expect(shape(l)).toEqual([
			['Time', ['time']],
			['Spectra', ['spectrum']],
			['FRF', ['frf']],
			['Readings', ['card:recmarks', 'readings']],
			['Recordings', ['card:recordings']]
		]);
		expect(l.dock.cards.map((c) => [c.kind, c.shown])).toEqual([
			['run', true],
			['recording', true],
			['recmarks', false],
			['recordings', false],
			['station', true]
		]);
		// the recordings card fills its page
		expect(l.pages[4].panels[0].at).toEqual({ x: 0, y: 0, w: 12, h: 12 });
		expect(l.rate).toBe(10);
	});

	it('takes over a layout the page layer kept, a Recordings page as its card', () => {
		const l = RECORDER_HOOKS.legacy?.read(JSON.stringify({ version: 1, rate: 5, serial: 6, pages: [{ id: 'rc-5', name: 'Saved', kind: 'recordings' }, { id: 'rc-1', name: 'Rows', kind: 'time' }] }));
		expect(l && shape(l)).toEqual([
			['Saved', ['card:recordings']],
			['Rows', ['time']]
		]);
	});
});
