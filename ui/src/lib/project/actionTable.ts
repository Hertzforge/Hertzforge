/**
 * The action table's order: what the service answers, cut to the module and to what can run now,
 * and sorted. A row that cannot run is not shown at all (Brandon, 2026-11-10: only what can be
 * done, for simplicity), though the service still says why. A column's defaults come first, then
 * what the picked part of a geometry offers, then the rest; combination columns follow by the
 * tree's order of kinds, with "any object" last, and one with nothing to run is not shown.
 */
import { moduleOf, offeredIn } from '$lib/app/workspaces';
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

/** A row as the table draws it: `depth` 1 under another, `holds` when rows are under it, `opened` when they show. */
export type ShownRow = ActionRow & { depth: number; holds: boolean; opened: boolean; label: string };

/** A row's words under its holder: without the holder's own when they start with them ("Auto Compute Random" is "Random" under Auto Compute). */
function under(holder: ActionRow, row: ActionRow): string {
	return row.verb.startsWith(`${holder.verb} `) ? row.verb.slice(holder.verb.length + 1) : row.verb;
}

/**
 * The rows in the order they are drawn: each row with the rows under it (`under`) right after it
 * while it is open, and not at all while it is closed. A row whose holder is not among the rows
 * stays where it was, at the top level; a `group` row with nothing under it is left out.
 * `isOpen` says whether a holder is open, given its id and how it opens at first.
 */
export function nestedRows(rows: ActionRow[], isOpen: (id: string, atFirst: boolean) => boolean): ShownRow[] {
	const ids = new Set(rows.map((row) => row.id));
	const held = new Map<string, ActionRow[]>();
	for (const row of rows) {
		if (row.under && ids.has(row.under)) held.set(row.under, [...(held.get(row.under) ?? []), row]);
	}
	const out: ShownRow[] = [];
	for (const row of rows) {
		if (row.under && ids.has(row.under)) continue;
		const held_ = held.get(row.id) ?? [];
		if (row.kind === 'group' && !held_.length) continue;
		const opened = held_.length > 0 && isOpen(row.id, !!row.open);
		out.push({ ...row, depth: 0, holds: held_.length > 0, opened, label: row.verb });
		if (opened) for (const child of held_) out.push({ ...child, depth: 1, holds: false, opened: false, label: under(row, child) });
	}
	return out;
}

/** One module's table: its rows only, and no combination column left with none. */
export function columnsFor(table: ActionTable, module: Workspace): ActionColumn[] {
	const kept = table.columns.map((column) => ({
		...column,
		rows: sortedRows(column.rows.filter((row) => (row.ready || row.kind === 'group') && offeredIn(row, module)))
	}));
	const current = kept.filter((column) => column.current);
	const others = kept
		.filter((column) => !column.current && column.rows.some((row) => row.kind !== 'group'))
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
			if (!row.ready || !row.modules || offeredIn(row, module)) continue;
			const home = moduleOf(row.modules[0]);
			counts.set(home, (counts.get(home) ?? 0) + 1);
		}
	}
	return [...counts].map(([home, count]) => ({ module: home, count }));
}

/** The words for an object kind in a sentence: 'time_history' reads 'time history'. */
export function kindWords(kind: string): string {
	return kind === '*' ? 'object' : kind.replace(/_/g, ' ');
}
