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
				row('Solve', { modules: ['model'] }),
				row('Add Node', { part: true }),
				row('Plot', { kind: 'default', id: 'show.plot' }),
				row('Mesh', { modules: ['model'] }),
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
		expect(verbs(0)).toEqual(['Plot', 'Add Node', 'Export', 'Averaging']);
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
		expect(columnsFor(empty, 'shock')).toHaveLength(1);
		expect(columnsFor(empty, 'shock')[0].rows).toEqual([]);
	});

	it('counts what other modules offer the selection, by home', () => {
		expect(moreIn(table, 'modal')).toEqual([{ module: 'model', count: 2 }]);
		expect(moreIn(table, 'shock')).toEqual([
			{ module: 'model', count: 2 },
			{ module: 'signals', count: 1 }
		]);
	});

	it('words a kind for a sentence', () => {
		expect(kindWords('time_history')).toBe('time history');
		expect(kindWords('*')).toBe('object');
	});
});
