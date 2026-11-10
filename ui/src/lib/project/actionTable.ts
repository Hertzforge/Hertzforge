/**
 * The action table's order: what the service answers, cut to the module and to what can run now,
 * and sorted. A row that cannot run is not shown at all (Brandon, 2026-11-10: only what can be
 * done, for simplicity), though the service still says why. A column's defaults come first, then
 * what the picked part of a geometry offers, then the rest; combination columns follow by the
 * tree's order of kinds, with "any object" last, and one with nothing to run is not shown.
 */
import { offeredIn } from '$lib/app/workspaces';
import type { ActionColumn, ActionRow, ActionTable, Workspace } from '$lib/protocol/types';
import { kindRank } from './icons';

function rowRank(row: ActionRow): number {
	if (row.kind === 'default') return 0;
	return row.part ? 1 : 2;
}

function sortedRows(rows: ActionRow[]): ActionRow[] {
	return rows
		.map((row, i) => [row, i] as const)
		.sort((a, b) => rowRank(a[0]) - rowRank(b[0]) || a[1] - b[1])
		.map(([row]) => row);
}

function columnRank(column: ActionColumn): number {
	const last = column.kinds[column.kinds.length - 1];
	return last === '*' ? 10_000 : kindRank(last);
}

/** One module's table: its rows only, and no combination column left with none. */
export function columnsFor(table: ActionTable, module: Workspace): ActionColumn[] {
	const kept = table.columns.map((column) => ({
		...column,
		rows: sortedRows(column.rows.filter((row) => row.ready && offeredIn(row, module)))
	}));
	const current = kept.filter((column) => column.current);
	const others = kept
		.filter((column) => !column.current && column.rows.length > 0)
		.map((column) => [column, columnRank(column)] as const)
		.sort((a, b) => a[1] - b[1])
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
