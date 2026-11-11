/**
 * A tone's amplitude history drawn against the bands its alarm and abort judge it on. The service's
 * amplitude view draws each tone's configured bands; a tone too close to the floor is judged against
 * bands widened to its reading's scatter, which its row states (`judged_alarm_db`,
 * `judged_abort_db`). Where a row states them, the view's alarm and abort lines are redrawn from its
 * Target line at those bands, so the plot shows the limits the run actually applies.
 */

import type { MixedPartRow } from '$lib/protocol/mixed';
import type { NumericArray, Series, View } from '$lib/protocol/types';

const scaled = (y: NumericArray, db: number): NumericArray => Float64Array.from(y as ArrayLike<number>, (v) => v * 10 ** (db / 20));

function redraw(v: View, row: MixedPartRow | undefined): View {
	if (v.view !== 'spectrum' || !row || row.kind !== 'tone') return v;
	const alarm = row.judged_alarm_db;
	const abort = row.judged_abort_db;
	if (!alarm && !abort) return v;
	const target = v.series.find((s) => s.role === 'reference');
	if (!target) return v;
	const band = (s: Series): Series => {
		const high = /high/i.test(s.name);
		const b = s.role === 'tolerance' ? alarm : s.role === 'abort' ? abort : null;
		if (!b || !(high || /low/i.test(s.name)) || s.y.length !== target.y.length) return s;
		return { ...s, y: scaled(target.y, high ? b[1] : -b[0]) };
	};
	return { ...v, series: v.series.map(band) };
}

/** An `amplitude` plot's views, one per resolved part in `parts` order, against each tone's judged bands. */
export function judgedTolerance(views: View[], parts: readonly string[] | null | undefined, rows: readonly MixedPartRow[]): View[] {
	if (!parts?.length) return views;
	return views.map((v, i) => redraw(v, rows.find((r) => r.id === parts[i])));
}
