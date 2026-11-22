import { describe, expect, it } from 'vitest';
import type { ActionRow, ActionTable } from '$lib/protocol/types';
import { columnsFor, kindWords, moreIn, nestedRows } from './actionTable';

const row = (verb: string, more: Partial<ActionRow> = {}): ActionRow => ({
	id: `project.${verb.toLowerCase()}`,
	verb,
	kind: 'run',
	ready: true,
	...more
});

const table: ActionTable = {
	columns: [
		{
			kinds: ['geometry'],
			current: true,
			rows: [
				row('Off', { ready: false, reason: 'no' }),
				row('Export'),
				row('Solve', { modules: ['geometry'] }),
				row('Add Node', { part: true }),
				row('Plot', { kind: 'default', id: 'show.plot' }),
				row('Mesh', { modules: ['geometry'] }),
				row('Averaging', { modules: ['signals', 'modal'] })
			]
		},
		{ kinds: ['geometry', '*'], rows: [row('Link', { candidates: ['A'] })] },
		{ kinds: ['geometry', 'psd'], rows: [row('Animate', { ready: false, reason: 'none', candidates: [] })] },
		{ kinds: ['geometry', 'time_history'], rows: [row('Animate', { candidates: ['B'] })] },
		{ kinds: ['geometry', 'report'], rows: [row('Draft', { modules: ['report'], candidates: ['C'] })] }
	]
};

describe('the action table', () => {
	const verbs = (i: number, module: 'modal' | 'report' = 'modal') => columnsFor(table, module)[i].rows.map((r) => r.verb);

	it('puts defaults first, then the part, then the rest, and leaves out what cannot run', () => {
		expect(verbs(0)).toEqual(['Plot', 'Add Node', 'Export', 'Solve', 'Mesh', 'Averaging']);
	});

	it('cuts rows and empty combination columns to the module', () => {
		const kinds = columnsFor(table, 'modal').map((c) => c.kinds.at(-1));
		expect(kinds).not.toContain('report');
		expect(columnsFor(table, 'report').map((c) => c.kinds.at(-1))).toContain('report');
	});

	it('orders combination columns by kind, any object last, and leaves out one with nothing to run', () => {
		expect(columnsFor(table, 'modal').map((c) => c.kinds.at(-1))).toEqual(['geometry', 'time_history', '*']);
	});

	it('keeps the selection column when a module leaves it empty', () => {
		const empty: ActionTable = { columns: [{ kinds: ['frf'], current: true, rows: [row('Fit', { modules: ['modal'] })] }] };
		expect(columnsFor(empty, 'report')).toHaveLength(1);
		expect(columnsFor(empty, 'report')[0].rows).toEqual([]);
	});

	it('offers the analysis module every row an earlier analysis module had', () => {
		const fit: ActionTable = { columns: [{ kinds: ['frf'], current: true, rows: [row('Fit', { modules: ['modal'] })] }] };
		for (const module of ['signals', 'shock', 'geometry'] as const) expect(columnsFor(fit, module)[0].rows.map((r) => r.verb)).toEqual(['Fit']);
	});

	it('counts what other modules offer the selection, by home', () => {
		// nothing an analysis row offers is in another module, whichever earlier id names it
		expect(moreIn(table, 'modal')).toEqual([]);
		expect(moreIn(table, 'signals')).toEqual([]);
		expect(moreIn(table, 'report')).toEqual([{ module: 'signals', count: 3 }]);
	});

	it('words a kind for a sentence', () => {
		expect(kindWords('time_history')).toBe('time history');
		expect(kindWords('*')).toBe('object');
	});
});

describe('rows under the row they work from', () => {
	const rows: ActionRow[] = [
		row('Plot', { kind: 'default', id: 'show.plot' }),
		row('PSDs', { under: 'project.averaging' }),
		row('Integrate'),
		row('Averaging', { open: true }),
		row('FRFs', { under: 'project.averaging' }),
		row('Filter'),
		row('Apply', { under: 'project.filter' }),
		row('Orphan', { under: 'project.missing' }),
		row('Rename', { kind: 'self', id: 'self.rename', under: 'group.object' }),
		{ id: 'group.object', verb: 'Object', kind: 'group', ready: true }
	];
	const drawn = (open: Record<string, boolean> = {}) => nestedRows(rows, (id, atFirst) => open[id] ?? atFirst).map((r) => `${'  '.repeat(r.depth)}${r.verb}${r.holds ? (r.opened ? ' v' : ' >') : ''}`);

	it('lists a group under its row while it is open, and opens the ones that say so', () => {
		expect(drawn()).toEqual(['Plot', 'Integrate', 'Averaging v', '  PSDs', '  FRFs', 'Filter >', 'Orphan', 'Object >']);
	});

	it('takes the choice made by hand over how a group opens at first', () => {
		expect(drawn({ 'project.averaging': false, 'project.filter': true, 'group.object': true })).toEqual(['Plot', 'Integrate', 'Averaging >', 'Filter v', '  Apply', 'Orphan', 'Object v', '  Rename']);
	});

	it('leaves a row whose holder is not there where it was, and a group of nothing out', () => {
		const few = rows.filter((r) => r.id !== 'self.rename' && r.id !== 'project.averaging');
		expect(nestedRows(few, () => false).map((r) => r.verb)).toEqual(['Plot', 'PSDs', 'Integrate', 'FRFs', 'Filter', 'Orphan']);
	});

	it('draws a view listed before its setting under it, as MAC under Project', () => {
		const pair: ActionRow[] = [
			row('MAC', { kind: 'default', id: 'show.mac', under: 'project.set_projection' }),
			row('Project', { id: 'project.set_projection', open: true, live: true }),
			row('Match Modes', { id: 'project.match_modes', under: 'project.set_projection' }),
			row('Link', { kind: 'self', id: 'self.link' })
		];
		expect(nestedRows(pair, (_id, atFirst) => atFirst).map((r) => `${'  '.repeat(r.depth)}${r.verb}`)).toEqual(['Project', '  MAC', '  Match Modes', 'Link']);
	});

	it('reads a row under its holder without the holder\'s own words', () => {
		const six: ActionRow[] = [{ id: 'group.auto_compute', verb: 'Auto Compute', kind: 'group', ready: true }, row('Auto Compute Random', { id: 'project.work_up_random', under: 'group.auto_compute' }), row('Predict', { under: 'group.auto_compute' })];
		expect(nestedRows(six, () => true).map((r) => r.label)).toEqual(['Auto Compute', 'Random', 'Predict']);
	});
});
