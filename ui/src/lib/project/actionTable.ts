/**
 * The action table's order: what the service answers, cut to the module and sorted. A column's
 * defaults come first, then what the picked part of a geometry offers, then what can run, and what
 * cannot goes last. Columns that can do something come before those that cannot, by the tree's order
 * of kinds, with "any object" at the end of the ones that can.
 */
import { offeredIn } from '$lib/app/workspaces';
import type { ActionColumn, ActionRow, ActionTable, Workspace } from '$lib/protocol/types';
import { kindRank } from './icons';

function rowRank(row: ActionRow): number {
	if (row.kind === 'default') return 0;
	if (!row.ready) return 3;
	return row.part ? 1 : 2;
}

function sortedRows(rows: ActionRow[]): ActionRow[] {
	return rows
		.map((row, i) => [row, i] as const)
		.sort((a, b) => rowRank(a[0]) - rowRank(b[0]) || a[1] - b[1])
		.map(([row]) => row);
}

function columnRank(column: ActionColumn): [number, number] {
	const last = column.kinds[column.kinds.length - 1];
	const ready = column.rows.some((row) => row.ready);
	return [ready ? 0 : 1, last === '*' ? 10_000 : kindRank(last)];
}

/** One module's table: its rows only, and no combination column left with none. */
export function columnsFor(table: ActionTable, module: Workspace): ActionColumn[] {
	const kept = table.columns.map((column) => ({
		...column,
		rows: sortedRows(column.rows.filter((row) => offeredIn(row, module)))
	}));
	const current = kept.filter((column) => column.current);
	const others = kept
		.filter((column) => !column.current && column.rows.length > 0)
		.map((column) => [column, columnRank(column)] as const)
		.sort((a, b) => a[1][0] - b[1][0] || a[1][1] - b[1][1])
		.map(([column]) => column);
	return [...current, ...others];
}

/** The verbs the selection could run now in other modules, counted by the module that is their home. */
export function moreIn(table: ActionTable, module: Workspace): { module: Workspace; count: number }[] {
	const counts = new Map<Workspace, number>();
	for (const column of table.columns) {
		if (!column.current) continue;
		for (const row of column.rows) {
			if (!row.ready || !row.modules || row.modules.includes(module)) continue;
			counts.set(row.modules[0], (counts.get(row.modules[0]) ?? 0) + 1);
		}
	}
	return [...counts].map(([home, count]) => ({ module: home, count }));
}

/** The words for an object kind in a sentence: 'time_history' reads 'time history'. */
export function kindWords(kind: string): string {
	return kind === '*' ? 'object' : kind.replace(/_/g, ' ');
}
