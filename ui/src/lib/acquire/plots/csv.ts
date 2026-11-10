/**
 * A plot's lines as comma-separated text: one x column, then one column per series, each headed
 * with its name and unit. Series on different x grids share the column of every x any of them
 * has, with a blank where a series has no sample. A series decimated to minimum and maximum gets
 * a second column for its minimum. Values are written in full, as the service sent them.
 */

import type { Series, SpectrumView } from '$lib/protocol/types';

/** A cell, quoted when it holds a comma, a quote or a line break. */
export function csvCell(text: string): string {
	return /[",\r\n]/.test(text) ? `"${text.replace(/"/g, '""')}"` : text;
}

const headed = (name: string, unit: string | null) => (unit ? `${name} (${unit})` : name);
const value = (v: number | undefined) => (v === undefined || !Number.isFinite(v) ? '' : String(v));

interface Column {
	head: string;
	at: Map<number, number>;
}

function column(head: string, s: Series, y: ArrayLike<number>): Column {
	const at = new Map<number, number>();
	const n = Math.min(s.x.length, y.length);
	for (let i = 0; i < n; i++) if (Number.isFinite(s.x[i])) at.set(s.x[i], y[i]);
	return { head, at };
}

/**
 * The views as CSV. With several views each column also names its view; `keep` chooses which of
 * a view's series are written (all by default).
 */
export function toCsv(views: SpectrumView[], keep: (view: number, series: number) => boolean = () => true): string {
	const several = views.length > 1;
	const columns: Column[] = [];
	views.forEach((v, vi) => {
		v.series.forEach((s, si) => {
			if (!keep(vi, si)) return;
			const name = several ? `${v.title}: ${s.name}` : s.name;
			columns.push(column(headed(name, v.y.unit), s, s.y));
			if (s.y_min) columns.push(column(headed(`${name}, minimum`, v.y.unit), s, s.y_min));
		});
	});
	const first = views[0];
	const xHead = first ? headed(first.x.label || 'x', first.x.unit) : 'x';
	const xs = [...new Set(columns.flatMap((c) => [...c.at.keys()]))].sort((a, b) => a - b);
	const lines = [[xHead, ...columns.map((c) => c.head)].map(csvCell).join(',')];
	for (const x of xs) lines.push([String(x), ...columns.map((c) => value(c.at.get(x)))].join(','));
	return lines.join('\n') + '\n';
}
