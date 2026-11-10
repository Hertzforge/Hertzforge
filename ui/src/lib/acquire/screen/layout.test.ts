/**
 * A test screen's layout as data: Random's own pages, reading a kept layout back field by field,
 * the one saved form, and taking over Random's layouts from before the page grid.
 */
import { describe, expect, it } from 'vitest';
import { defaultRandomLayout, toSaved as toSavedV1 } from '../display/layout';
import { defaultDisplay } from '../plots/types';
import { fits } from './grid';
import {
	defaultRandomScreen,
	fromSaved,
	fromV1,
	lastScreenKey,
	nextId,
	plotTitle,
	RANDOM_CARDS,
	RANDOM_LEGACY,
	RANDOM_TYPES,
	readDisplay,
	readScreen,
	screenKey,
	toSaved
} from './layout';
import type { Panel } from './types';

const offer = { types: RANDOM_TYPES, cards: RANDOM_CARDS };
const quantities = (panels: Panel[]) => panels.map((p) => (p.kind === 'plot' ? p.plot.quantity : `card:${p.card}`));

describe("Random's own layout", () => {
	it('offers its six quantities as plot types, in groups, with the channels and the lines each takes', () => {
		expect(RANDOM_TYPES.map((t) => [t.id, t.group, t.channels, t.time])).toEqual([
			['control', 'frequency', false, false],
			['drive', 'frequency', false, false],
			['response', 'frequency', true, false],
			['time', 'time', true, true],
			['coherence', 'frequency', false, false],
			['readings', 'numbers', false, false]
		]);
		expect(RANDOM_TYPES[0].words).toBe('Control and Reference');
		expect(RANDOM_TYPES[0].families).toEqual(['measured', 'reference', 'tolerance', 'abort']);
		expect(RANDOM_TYPES[1].families).toEqual(['drive']);
		expect(RANDOM_TYPES[2].families).toEqual(['measured', 'limit']);
		expect(RANDOM_TYPES[5].families).toBeUndefined();
		expect(RANDOM_TYPES.every((t) => t.help.length > 0)).toBe(true);
	});

	it('starts on the Control, Limits and Responses pages, each tiled evenly, with the cards in the dock on the right', () => {
		const l = defaultRandomScreen();
		expect(l.pages.map((p) => p.name)).toEqual(['Control', 'Limits', 'Responses']);
		expect(quantities(l.pages[0].panels)).toEqual(['control', 'drive']);
		expect(l.pages.map((p) => p.panels.map((x) => x.id))).toEqual([['pl-1', 'pl-2'], ['pl-3'], ['pl-4']]);
		const [limits, all] = [l.pages[1].panels[0], l.pages[2].panels[0]];
		expect(limits.kind === 'plot' && limits.plot.channels).toBe('limited');
		expect(all.kind === 'plot' && all.plot.channels).toBe('all');
		for (const p of l.pages) {
			expect(fits(p.panels, p.cols, p.rows)).toBe(true);
			expect(p.panels.reduce((s, x) => s + x.at.w * x.at.h, 0)).toBe(p.cols * p.rows);
		}
		expect(l.dock.side).toBe('right');
		expect(l.dock.cards.map((c) => [c.kind, c.open, c.shown])).toEqual(RANDOM_CARDS.map((k) => [k, true, true]));
		expect(plotTitle((limits as Extract<Panel, { kind: 'plot' }>).plot, RANDOM_TYPES[2])).toBe('Response Spectra: limited channels');
	});

	it('numbers new pages and panels past every number used', () => {
		const l = defaultRandomScreen();
		expect(nextId(l, 'pl')).toBe('pl-5');
		expect(nextId(l, 'cd')).toBe('cd-6');
		expect(nextId(l, 'pg')).toBe('pg-7');
	});
});

describe('reading a kept layout', () => {
	it('reads back what it saved, and saves one form whatever order the fields were set in', () => {
		const l = defaultRandomScreen();
		l.pages[1].name = 'Fixture Limits';
		const p = l.pages[0].panels[0];
		if (p.kind === 'plot') {
			p.plot.title = 'Control';
			p.plot.display.y = { ...p.plot.display.y, scale: 'linear', range: 'fixed', min: 1e-6, max: 1 };
			p.plot.display.traces = { Reference: { visible: false, width: 2, dash: 'dashed', color: null }, Control: { visible: true, width: null, dash: null, color: '#123456' } };
			p.plot.display.notes = [{ id: 'n1', text: 'Fixture mode', fx: 0.4, fy: 0.2 }];
		}
		l.pages[0].panels.push({ id: 'cd-5', kind: 'card', at: { x: 0, y: 0, w: 1, h: 1 }, card: 'log' });
		l.pages[0].panels[0].at = { x: 1, y: 0, w: 11, h: 6 };
		l.dock.cards[4].shown = false;
		l.dock.side = 'collapsed';
		l.dock.restore = 'bottom';
		l.dock.size = 260;
		l.serial = 5;
		const back = fromSaved(toSaved(l), offer);
		expect(back).toEqual(l);
		expect(toSaved(back!)).toBe(toSaved(l));
		// The trace styles save in one order however they were added.
		const swapped = structuredClone(l);
		const q = swapped.pages[0].panels[0];
		if (q.kind === 'plot') q.plot.display.traces = { Control: l.pages[0].panels[0].kind === 'plot' ? l.pages[0].panels[0].plot.display.traces.Control : ({} as never), Reference: { visible: false, width: 2, dash: 'dashed', color: null } };
		expect(toSaved(swapped)).toBe(toSaved(l));
	});

	it('refuses what is not a layout', () => {
		expect(readScreen({ version: 1, pages: [] }, offer)).toBeNull();
		expect(readScreen({ version: 2, pages: [] }, offer)).toBeNull();
		expect(readScreen({ version: 2, pages: [{ id: 'p', name: '   ', panels: [] }] }, offer)).toBeNull();
		expect(fromSaved('{not json', offer)).toBeNull();
		expect(fromSaved(null, offer)).toBeNull();
	});

	it('drops what it cannot trust, field by field', () => {
		const l = readScreen(
			{
				version: 2,
				rate: 7,
				serial: -3,
				pages: [
					{ id: 'pg-1', name: '  Odd   page ', cols: 13, rows: 'x', linkTime: 'yes', panels: [
						{ id: 'pl-1', kind: 'plot', at: { x: 0, y: 0, w: 6, h: 6 }, plot: { quantity: 'control', channels: [1], points: 9, title: 7, display: { grid: 'tartan', y: { scale: 'log', min: 'a', max: 3 }, hidden: ['abort', 'sound', 'abort'], traces: { A: { width: 99, dash: 'wavy', color: 'red' } } } } },
						{ id: 'pl-1', kind: 'plot', at: { x: 6, y: 0, w: 6, h: 6 }, plot: { quantity: 'drive' } },
						{ id: 'pl-2', kind: 'plot', at: { x: 6, y: 0, w: 6, h: 6 }, plot: { quantity: 'sound' } },
						{ id: 'pl-3', kind: 'plot', at: { x: 6, y: 0, w: 6, h: 6 }, plot: { quantity: 'response', channels: [3, -1, 3, 2.5, 'x'] } },
						{ id: 'cd-1', kind: 'card', at: { x: 0, y: 6, w: 12, h: 6 }, card: 'level' },
						{ id: 'cd-2', kind: 'card', at: { x: 0, y: 6, w: 12, h: 6 }, card: 'level' },
						{ id: 'cd-3', kind: 'card', at: { x: 0, y: 6, w: 12, h: 6 }, card: 'weather' },
						{ id: 'cd-4', kind: 'shelf', at: { x: 0, y: 6, w: 12, h: 6 } }
					] },
					{ id: 'pg-1', name: 'Twin', panels: [] }
				],
				dock: { side: 'top', size: 9999, cards: [{ kind: 'log', open: false }, { kind: 'log' }, { kind: 'weather' }] }
			},
			offer
		);
		expect(l).not.toBeNull();
		const page = l!.pages[0];
		expect(l!.pages).toHaveLength(1);
		expect([page.name, page.cols, page.rows, page.linkTime]).toEqual(['Odd page', 12, 12, false]);
		expect(page.panels.map((p) => p.id)).toEqual(['pl-1', 'pl-3', 'cd-1']);
		const [control, response] = page.panels as Extract<Panel, { kind: 'plot' }>[];
		// A plot type that takes no channels shows them all, whatever was kept.
		expect([control.plot.channels, control.plot.points, control.plot.title]).toEqual(['all', 1024, null]);
		expect(control.plot.display.grid).toBe('major');
		expect([control.plot.display.y.scale, control.plot.display.y.min, control.plot.display.y.max]).toEqual(['log', null, 3]);
		expect(control.plot.display.hidden).toEqual(['abort']);
		expect(control.plot.display.traces).toEqual({ A: { visible: true, width: null, dash: null, color: null } });
		expect(response.plot.channels).toEqual([3]);
		expect(fits(page.panels, 12, 12)).toBe(true);
		expect([l!.rate, l!.serial]).toEqual([5, 3]);
		// The dock: a side it knows and a size it allows; every card once; a card on a page not in the dock too.
		expect([l!.dock.side, l!.dock.size]).toEqual(['right', 640]);
		expect(l!.dock.cards.map((c) => [c.kind, c.open, c.shown])).toEqual([
			['log', false, true],
			['run', true, true],
			['level', true, false],
			['readings', true, true],
			['inputs', true, true]
		]);
	});

	it('tiles a page evenly again when its panels overlap or leave the grid', () => {
		const l = readScreen(
			{
				version: 2,
				pages: [{ id: 'pg-1', name: 'Over', panels: [
					{ id: 'pl-1', kind: 'plot', at: { x: 0, y: 0, w: 8, h: 8 }, plot: { quantity: 'control' } },
					{ id: 'pl-2', kind: 'plot', at: { x: 4, y: 4, w: 8, h: 8 }, plot: { quantity: 'drive' } }
				] }]
			},
			offer,
			0.8
		);
		expect(l!.pages[0].panels.map((p) => [p.at.x, p.at.y, p.at.w, p.at.h])).toEqual([
			[0, 0, 12, 6],
			[0, 6, 12, 6]
		]);
		// A cell off the grid is brought back onto it.
		const off = readScreen({ version: 2, pages: [{ id: 'a', name: 'A', panels: [{ id: 'pl-1', kind: 'plot', at: { x: 30, y: -4, w: 3, h: 3 }, plot: { quantity: 'time' } }] }] }, offer);
		expect(off!.pages[0].panels[0].at).toEqual({ x: 9, y: 0, w: 3, h: 3 });
	});

	it('reads display options it was never given as the defaults', () => {
		expect(readDisplay(undefined)).toEqual(defaultDisplay());
		expect(readDisplay({ window_s: -1, count: 2.5, average: 0, notes: [{ id: 'a', text: ' ', fx: 0, fy: 0 }, { id: 'b', text: 'x', fx: 4, fy: -1 }] })).toEqual({ ...defaultDisplay(), notes: [{ id: 'b', text: 'x', fx: 1, fy: 0 }] });
	});
});

describe('where a layout is kept', () => {
	it('per test type and per setup, and the last one used', () => {
		expect(screenKey('random', '  Qualification   Random ')).toBe('hertzforge.screen.v2:random:qualification random');
		expect(screenKey('random', '')).toBe('hertzforge.screen.v2:random:untitled');
		expect(screenKey('random', null)).toBe('hertzforge.screen.v2:random:untitled');
		expect(lastScreenKey('random')).toBe('hertzforge.screen.v2:random:*last');
	});
});

describe("Random's layouts from before the page grid", () => {
	it('keep their pages and names, with their plots tiled evenly', () => {
		const v1 = defaultRandomLayout();
		v1.pages[0].name = 'Main';
		v1.pages[2].plots.push({ id: 'pl-9', quantity: 'time', channels: [2, 1], arrange: 'overlay', x: 'linear', y: 'log', yMin: 1e-4, yMax: 10, points: 2048 });
		v1.pages[2].plots.push({ id: 'pl-10', quantity: 'coherence', channels: 'all', arrange: 'split', x: 'auto', y: 'auto', yMin: 5, yMax: 1, points: 1024 });
		v1.serial = 10;
		const l = fromV1(v1, offer, 0.8);
		expect(l.pages.map((p) => [p.id, p.name])).toEqual([
			['pg-1', 'Main'],
			['pg-2', 'Limits'],
			['pg-3', 'Responses']
		]);
		expect(l.pages[2].panels.map((p) => [p.id, p.at.y, p.at.h])).toEqual([
			['pl-4', 0, 4],
			['pl-9', 4, 4],
			['pl-10', 8, 4]
		]);
		for (const p of l.pages) expect(fits(p.panels, 12, 12)).toBe(true);
		expect([l.rate, l.serial, l.version]).toEqual([5, 10, 2]);
		expect(l.dock.cards.map((c) => c.kind)).toEqual(RANDOM_CARDS);
	});

	it("put each plot's axes into its display options: the scales, and a y range when it was a range", () => {
		const v1 = defaultRandomLayout();
		v1.pages[0].plots = [
			{ id: 'pl-1', quantity: 'time', channels: [2, 1], arrange: 'overlay', x: 'linear', y: 'log', yMin: 1e-4, yMax: 10, points: 2048 },
			{ id: 'pl-2', quantity: 'control', channels: 'all', arrange: 'split', x: 'auto', y: 'auto', yMin: 5, yMax: 1, points: 1024 }
		];
		const l = fromV1(v1);
		const [time, control] = l.pages[0].panels as Extract<Panel, { kind: 'plot' }>[];
		expect([time.plot.quantity, time.plot.channels, time.plot.arrange, time.plot.points]).toEqual(['time', [2, 1], 'overlay', 2048]);
		expect([time.plot.display.x.scale, time.plot.display.y.scale]).toEqual(['linear', 'log']);
		expect([time.plot.display.y.range, time.plot.display.y.min, time.plot.display.y.max]).toEqual(['fixed', 1e-4, 10]);
		expect([control.plot.display.y.range, control.plot.display.y.min, control.plot.display.y.max]).toEqual(['auto', null, null]);
	});

	it('are found where they were kept, and read back through the same checks', () => {
		const v1 = defaultRandomLayout();
		v1.pages[1].name = 'Old Limits';
		expect(RANDOM_LEGACY.key('Bench')).toBe('hertzforge.display.v1:random:bench');
		expect(RANDOM_LEGACY.last).toBe('hertzforge.display.v1:random:*last');
		const l = RANDOM_LEGACY.read(toSavedV1(v1));
		expect(l?.pages[1].name).toBe('Old Limits');
		expect(fromSaved(toSaved(l!), offer)).toEqual(l);
		expect(RANDOM_LEGACY.read('{"version":3}')).toBeNull();
	});
});
