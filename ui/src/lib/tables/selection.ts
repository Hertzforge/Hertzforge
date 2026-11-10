/**
 * Rows selected in a table by their number: a click picks one, Shift extends a run from the
 * anchor, Cmd or Ctrl adds or takes away one, Shift with an arrow key extends the run a row at a
 * time. Pure: every function returns a new selection. Rows are positions in the table as shown.
 */

export interface RowSelection {
	/** The selected positions, ascending. */
	rows: number[];
	/** Where a Shift run starts from. */
	anchor: number | null;
	/** The row the keyboard is on. */
	focus: number | null;
}

export const NO_SELECTION: RowSelection = { rows: [], anchor: null, focus: null };

const run = (a: number, b: number) => Array.from({ length: Math.abs(b - a) + 1 }, (_, k) => Math.min(a, b) + k);
const sorted = (rows: Iterable<number>) => [...new Set(rows)].sort((a, b) => a - b);

/**
 * A click on row `at`: `only` selects it alone (or clears it when it was the one row selected),
 * `toggle` adds or takes it away, `range` selects the run from the anchor to it.
 */
export function pick(sel: RowSelection, at: number, how: 'only' | 'toggle' | 'range'): RowSelection {
	if (how === 'range' && sel.anchor !== null) return { rows: run(sel.anchor, at), anchor: sel.anchor, focus: at };
	if (how === 'toggle') {
		const has = sel.rows.includes(at);
		return { rows: sorted(has ? sel.rows.filter((r) => r !== at) : [...sel.rows, at]), anchor: at, focus: at };
	}
	if (sel.rows.length === 1 && sel.rows[0] === at) return { rows: [], anchor: at, focus: at };
	return { rows: [at], anchor: at, focus: at };
}

/** How a click's keys pick: Shift for a run, Cmd or Ctrl for one more or one less. */
export function pickHow(e: { shiftKey: boolean; metaKey: boolean; ctrlKey: boolean }): 'only' | 'toggle' | 'range' {
	if (e.shiftKey) return 'range';
	if (e.metaKey || e.ctrlKey) return 'toggle';
	return 'only';
}

/**
 * An arrow key from the focused row: the focus moves by `delta` within `count` rows; with
 * `extend` the run from the anchor follows it, and without, the row reached is selected alone.
 */
export function step(sel: RowSelection, delta: number, extend: boolean, count: number): RowSelection {
	if (count === 0) return NO_SELECTION;
	const from = sel.focus ?? sel.anchor ?? (delta > 0 ? -1 : count);
	const to = Math.max(0, Math.min(count - 1, from + delta));
	if (extend) {
		const anchor = sel.anchor ?? (sel.focus ?? to);
		return { rows: run(anchor, to), anchor, focus: to };
	}
	return { rows: [to], anchor: to, focus: to };
}

/** The selection with the rows past `count` taken out, as after rows are removed. */
export function within(sel: RowSelection, count: number): RowSelection {
	const keep = (r: number | null) => (r !== null && r < count ? r : null);
	return { rows: sel.rows.filter((r) => r < count), anchor: keep(sel.anchor), focus: keep(sel.focus) };
}

/**
 * The rows an edit made at row `at` reaches: every selected row when two or more are selected
 * and `at` is one of them, else `at` alone.
 */
export function reach(sel: RowSelection, at: number): number[] {
	return sel.rows.length >= 2 && sel.rows.includes(at) ? sel.rows : [at];
}

export const isSelected = (sel: RowSelection, at: number) => sel.rows.includes(at);
