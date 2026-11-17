import { describe, expect, it } from 'vitest';
import type { ActionRow, ActionTable } from '$lib/protocol/types';
import { columnsFor, kindWords, moreIn } from './actionTable';

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
