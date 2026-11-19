/** TWR on the test screen's frame: its plot types, its first pages with the play table on the Plays page, and a kept layout taken over. */
import { describe, expect, it } from 'vitest';
import { TWR_POINTS_QUANTITIES } from '$lib/protocol/twr';
import { TWR_PLOT_TYPES } from './twrLayout';
import { TWR_HOOKS } from './twrViews.svelte';

const shape = (l: ReturnType<typeof TWR_HOOKS.defaultLayout>) => l.pages.map((p) => [p.name, p.panels.map((x) => (x.kind === 'plot' ? x.plot.quantity : `card:${x.card}`))]);

describe('TWR on the screen frame', () => {
	it('offers every quantity it draws as a plot type, the records as time plots; the play table is a card', () => {
		expect(TWR_PLOT_TYPES.filter((t) => t.time).map((t) => t.id).sort()).toEqual([...TWR_POINTS_QUANTITIES].sort());
		expect(TWR_PLOT_TYPES.map((t) => t.id)).not.toContain('plays');
		expect(TWR_PLOT_TYPES.every((t) => !t.channels && t.help.length > 10)).toBe(true);
		expect(TWR_HOOKS.cards).toEqual(['run', 'level', 'plays', 'station']);
	});

	it('opens on the seven pages it had, the play table on the Plays page', () => {
		const l = TWR_HOOKS.defaultLayout(1.4);
		expect(shape(l)).toEqual([
			['Replication', ['response']],
			['Error', ['error']],
			['Spectra', ['spectra']],
			['Identification', ['frf', 'coherence', 'mask']],
			['Convergence', ['convergence']],
			['Law', ['law', 'readings']],
			['Plays', ['card:plays', 'readings']]
		]);
		expect(l.dock.cards.find((c) => c.kind === 'plays')?.shown).toBe(false);
		expect(l.rate).toBe(5);
	});

	it('takes over a layout with the pages a new one does not open with', () => {
		const text = JSON.stringify({ version: 1, rate: 10, serial: 9, pages: [{ id: 'tp-8', name: 'Target', kind: 'target' }, { id: 'tp-11', name: 'Scope', kind: 'scope' }, { id: 'tp-9', name: 'Drive', kind: 'drive' }] });
		const l = TWR_HOOKS.legacy?.read(text);
		expect(l && shape(l)).toEqual([
			['Target', ['target']],
			['Scope', ['scope']],
			['Drive', ['drive']]
		]);
		expect(l?.rate).toBe(10);
	});
});
