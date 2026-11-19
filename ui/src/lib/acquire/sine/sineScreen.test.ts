/** Sine on the test screen's frame: its plot types, its first pages and a kept layout taken over. */
import { describe, expect, it } from 'vitest';
import { ServiceClient } from '$lib/protocol/client.svelte';
import { createControlServer } from '$lib/protocol/mock/control';
import { mockSocketFactory } from '$lib/protocol/mock/socket';
import { layoutKey } from '../display/layout';
import { screenKey } from '../screen/layout';
import { SINE_PLOT_TYPES } from './sineLayout';
import { SINE_HOOKS, SineViewsState } from './sineViews.svelte';

const kinds = (l: ReturnType<typeof SINE_HOOKS.defaultLayout>) => l.pages.map((p) => [p.name, p.panels.map((x) => (x.kind === 'plot' ? `${x.plot.quantity}${x.plot.channels === 'all' ? '' : `:${String(x.plot.channels)}`}` : `card:${x.card}`))]);

function memoryStorage(): Storage & { map: Map<string, string> } {
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

describe('Sine on the screen frame', () => {
	it('offers every quantity the service draws as a plot type', () => {
		expect(SINE_PLOT_TYPES.map((t) => [t.id, t.words, t.group, t.channels, t.time])).toEqual([
			['control', 'Control', 'frequency', false, false],
			['drive', 'Drive', 'frequency', false, false],
			['response', 'Response', 'frequency', true, false],
			['phase', 'Phase', 'frequency', true, false],
			['thd', 'Distortion', 'frequency', false, false],
			['time', 'Time History', 'time', true, true],
			['identification', 'Identification', 'frequency', false, false],
			['law', 'Law Values', 'numbers', false, false],
			['readings', 'Readings', 'numbers', false, false]
		]);
		for (const t of SINE_PLOT_TYPES) expect(t.help.length).toBeGreaterThan(10);
	});

	it('opens on the five pages it had, each of its quantities a plot; the cards in the dock', () => {
		const l = SINE_HOOKS.defaultLayout(1.4);
		expect(kinds(l)).toEqual([
			['Control', ['control', 'drive']],
			['Limits', ['response:limited']],
			['Responses', ['response', 'phase']],
			['Identification', ['identification']],
			['Law', ['law', 'readings']]
		]);
		expect(l.dock.cards.map((c) => [c.kind, c.shown])).toEqual([
			['run', true],
			['level', true],
			['law', true],
			['station', true]
		]);
		expect(l.rate).toBe(5);
	});

	it('takes over a layout the page layer kept, the first time it is read, and keeps it in the new form', async () => {
		const storage = memoryStorage();
		const v1 = { version: 1, rate: 2, serial: 9, pages: [{ id: 'np-1', name: 'Control', kind: 'control' }, { id: 'np-7', name: 'Distortion Check', kind: 'distortion' }, { id: 'np-9', name: 'Samples', kind: 'time' }] };
		storage.setItem(layoutKey('sine', 'Bench'), JSON.stringify(v1));
		const client = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(createControlServer({ speed: 20 }), 1) });
		const views = new SineViewsState(client, { setupTitle: () => 'Bench', showing: () => false, storage, session: null, events: null });
		const stop = views.watch();
		await Promise.resolve();
		await Promise.resolve();
		expect(kinds(views.layout)).toEqual([
			['Control', ['control', 'drive']],
			['Distortion Check', ['thd']],
			['Samples', ['time']]
		]);
		expect(views.layout.rate).toBe(2);
		expect(storage.getItem(screenKey('sine', 'Bench'))).not.toBeNull();
		stop();
	});
});
