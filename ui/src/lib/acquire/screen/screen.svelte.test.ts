/**
 * One window's test screen: its pages and panels, the dock, and how the layout is kept and taken
 * over, with Random's hooks (the subscription itself is tested against the mock service in
 * `display.test.ts`).
 */
import { describe, expect, it } from 'vitest';
import { ServiceClient } from '$lib/protocol/client.svelte';
import { createControlServer } from '$lib/protocol/mock/control';
import { mockSocketFactory } from '$lib/protocol/mock/socket';
import { defaultRandomLayout, layoutKey, toSaved as toSavedV1 } from '../display/layout';
import { DisplayState } from '../display/display.svelte';
import { fits } from './grid';
import { lastScreenKey, screenKey, toSaved } from './layout';

/** The effects run (a microtask). */
const effects = async () => {
	await Promise.resolve();
	await Promise.resolve();
};

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

function screen(storage: Storage | null = null, title: () => string | null = () => 'Bench') {
	const client = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(createControlServer({ speed: 20 }), 1) });
	return new DisplayState(client, { setupTitle: title, showing: () => true, storage, session: null, events: null });
}

const cells = (s: DisplayState) => s.page.panels.map((p) => [p.id, p.at.x, p.at.y, p.at.w, p.at.h]);

describe('pages', () => {
	it('adds, renames, orders and removes pages; the last one stays', () => {
		const s = screen();
		const id = s.addPage() as string;
		expect([s.pageId, s.page.name, s.page.panels]).toEqual([id, 'Page 4', []]);
		expect(s.renamePage(id, '  ')).toBe(false);
		expect(s.renamePage(id, '  Drive   Detail ')).toBe(true);
		expect(s.page.name).toBe('Drive Detail');
		s.movePage(id, -1);
		expect(s.pages.map((p) => p.name)).toEqual(['Control', 'Limits', 'Drive Detail', 'Responses']);
		expect(s.removePage(id)).toBe(true);
		expect(s.pageId).toBe('pg-2');
		for (const p of [...s.pages].slice(1)) s.removePage(p.id);
		expect(s.removePage(s.pages[0].id)).toBe(false);
		expect(s.pages.map((p) => p.name)).toEqual(['Control']);
	});

	it('changes a page to a finer or coarser grid, each panel keeping its share', () => {
		const s = screen();
		s.aspect = 1.4;
		expect(cells(s)).toEqual([
			['pl-1', 0, 0, 12, 6],
			['pl-2', 0, 6, 12, 6]
		]);
		s.setGrid(24);
		expect([s.page.cols, s.page.rows]).toEqual([24, 24]);
		expect(cells(s)).toEqual([
			['pl-1', 0, 0, 24, 12],
			['pl-2', 0, 12, 24, 12]
		]);
		s.setGrid(7);
		expect(s.page.cols).toBe(24);
	});
});

describe('panels', () => {
	it('puts a new plot in the largest free space, or tiles the page evenly when there is none', () => {
		const s = screen();
		s.aspect = 1.4;
		s.selectPage('pg-2');
		s.resizePanel('pl-3', 12, 6);
		const a = s.addPlot('time') as string;
		expect(cells(s)).toEqual([
			['pl-3', 0, 0, 12, 6],
			[a, 0, 6, 12, 6]
		]);
		expect(s.plot(a)?.quantity).toBe('time');
		const b = s.addPlot('coherence') as string;
		expect(cells(s)).toEqual([
			['pl-3', 0, 0, 6, 6],
			[a, 6, 0, 6, 6],
			[b, 0, 6, 12, 6]
		]);
		expect(fits(s.page.panels, 12, 12)).toBe(true);
		expect(s.addPlot('sound')).toBeNull();
	});

	it('moves, resizes and steps panels, refusing what has no room', () => {
		const s = screen();
		s.aspect = 1.4;
		// Control above Drive: the keyboard swaps them.
		expect(s.canStep('pl-1', 0, 1)).toBe(true);
		expect(s.stepPanel('pl-1', 0, 1)).toBe(true);
		expect(cells(s)).toEqual([
			['pl-1', 0, 6, 12, 6],
			['pl-2', 0, 0, 12, 6]
		]);
		expect(s.canGrow('pl-2', 1, 0)).toBe(false);
		expect(s.growPanel('pl-2', 0, -2)).toBe(true);
		expect(cells(s)).toEqual([
			['pl-1', 0, 4, 12, 6],
			['pl-2', 0, 0, 12, 4]
		]);
		expect(s.movePanel('pl-2', 6, 0)).toBe(true);
		expect(s.resizePanel('pl-2', 6, 4)).toBe(true);
		expect(s.movePanel('nothing', 0, 0)).toBe(false);
		s.arrangeEvenly();
		expect(cells(s)).toEqual([
			['pl-1', 0, 6, 12, 6],
			['pl-2', 0, 0, 12, 6]
		]);
	});

	it('removes a panel and closes the gap; maximizes one at a time', () => {
		const s = screen();
		s.toggleMaximize('pl-2');
		expect(s.maximized).toBe('pl-2');
		s.toggleMaximize('pl-1');
		expect(s.maximized).toBe('pl-1');
		s.toggleMaximize('pl-1');
		expect(s.maximized).toBeNull();
		s.toggleMaximize('pl-1');
		s.openOptions('pl-1');
		s.removePanel('pl-1');
		expect([s.maximized, s.options]).toEqual([null, null]);
		expect(cells(s)).toEqual([['pl-2', 0, 0, 12, 6]]);
	});

	it('changes a plot by a patch or a whole new choice, and names it', () => {
		const s = screen();
		s.updatePlot('pl-1', { title: 'Main' });
		expect(s.titleOf(s.page.panels[0])).toBe('Main');
		const spec = { ...(s.plot('pl-2') as NonNullable<ReturnType<typeof s.plot>>), quantity: 'response', channels: [2] };
		s.updatePlot('pl-2', spec);
		expect(s.titleOf(s.page.panels[1])).toBe('Response Spectra: channel 2');
	});
});

describe('the dock', () => {
	it('moves to the bottom, folds to a rail and opens again where it was, each side its own size', () => {
		const s = screen();
		expect([s.dock.side, s.dock.size]).toEqual(['right', 320]);
		s.setDockSize(5000);
		expect(s.dock.size).toBe(640);
		s.setDockSide('bottom');
		expect([s.dock.side, s.dock.restore, s.dock.size]).toEqual(['bottom', 'bottom', 230]);
		s.setDockSide('collapsed');
		expect([s.dock.side, s.dock.restore]).toEqual(['collapsed', 'bottom']);
		s.setDockSide(s.dock.restore);
		expect([s.dock.side, s.dock.size]).toEqual(['bottom', 230]);
		s.setDockSize(10);
		expect(s.dock.size).toBe(140);
	});

	it('reorders, folds and hides cards', () => {
		const s = screen();
		s.moveCard('log', 0);
		s.moveCard('run', 99);
		expect(s.dock.cards.map((c) => c.kind)).toEqual(['log', 'level', 'readings', 'inputs', 'run']);
		s.setCardOpen('inputs', false);
		s.showCard('readings', false);
		expect(s.dock.cards.map((c) => [c.kind, c.open, c.shown])).toEqual([
			['log', true, true],
			['level', true, true],
			['readings', true, false],
			['inputs', false, true],
			['run', true, true]
		]);
		expect(s.cardInView('readings')).toBe(false);
		expect(s.cardInView('inputs')).toBe(true);
		s.setDockSide('collapsed');
		expect(s.cardInView('run')).toBe(false);
	});

	it('moves a card onto the page as a panel, and back', () => {
		const s = screen();
		s.aspect = 1.4;
		const id = s.cardToPage('level') as string;
		expect(s.cardToPage('level')).toBeNull();
		expect(s.dock.cards.find((c) => c.kind === 'level')?.shown).toBe(false);
		expect(s.cardPlace('level')?.panel.id).toBe(id);
		expect(fits(s.page.panels, 12, 12)).toBe(true);
		expect(s.page.panels).toHaveLength(3);
		expect(s.cardInView('level')).toBe(true);
		s.selectPage('pg-2');
		expect(s.cardInView('level')).toBe(false);
		s.selectPage('pg-1');
		s.cardToDock(id);
		expect(s.cardPlace('level')).toBeNull();
		expect(s.dock.cards.find((c) => c.kind === 'level')?.shown).toBe(true);
		expect(s.page.panels).toHaveLength(2);
		// Closing a card panel, or removing its page, sends it back to the dock too.
		s.cardToPage('log');
		s.removePage('pg-1');
		expect(s.dock.cards.find((c) => c.kind === 'log')?.shown).toBe(true);
	});
});

describe('keeping the layout', () => {
	it("takes over a setup's layout from before the page grid once, and keeps it in the new form", async () => {
		const storage = memoryStorage();
		const v1 = defaultRandomLayout();
		v1.pages[1].name = 'Old Limits';
		storage.setItem(layoutKey('random', 'Bench'), toSavedV1(v1));
		const s = screen(storage);
		const stop = s.watch();
		await effects();
		expect(s.pages.map((p) => p.name)).toEqual(['Control', 'Old Limits', 'Responses']);
		const kept = storage.getItem(screenKey('random', 'Bench'));
		expect(kept).toBe(toSaved(s.layout));
		expect(storage.getItem(lastScreenKey('random'))).toBe(kept);
		// The old form is left where it was.
		expect(storage.getItem(layoutKey('random', 'Bench'))).toBe(toSavedV1(v1));
		stop();
	});

	it("follows the setup: its own layout, else the test type's last, else the defaults", async () => {
		const storage = memoryStorage();
		let title = $state<string | null>('Bench');
		const s = screen(storage, () => title);
		const stop = s.watch();
		await effects();
		s.renamePage('pg-1', 'Bench Control');
		await effects();
		title = 'Other';
		await effects();
		// A setup with none of its own starts from the last one used, and has it kept for itself.
		expect(s.pages[0].name).toBe('Bench Control');
		expect(storage.getItem(screenKey('random', 'Other'))).toBe(toSaved(s.layout));
		s.renamePage('pg-1', 'Other Control');
		await effects();
		title = 'Bench';
		await effects();
		expect(s.pages[0].name).toBe('Bench Control');
		stop();
		// Nothing kept at all: the defaults.
		const fresh = screen(memoryStorage());
		const stop2 = fresh.watch();
		await effects();
		expect(fresh.pages.map((p) => p.name)).toEqual(['Control', 'Limits', 'Responses']);
		stop2();
	});
});
