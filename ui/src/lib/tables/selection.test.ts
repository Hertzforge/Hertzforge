import { describe, expect, it } from 'vitest';
import { NO_SELECTION, pick, pickHow, reach, step, within } from './selection';

describe('row selection', () => {
	it('a click selects one row, and again clears it', () => {
		const one = pick(NO_SELECTION, 3, 'only');
		expect(one.rows).toEqual([3]);
		expect(pick(one, 5, 'only').rows).toEqual([5]);
		expect(pick(one, 3, 'only').rows).toEqual([]);
	});

	it('Shift selects the run from the anchor, either way', () => {
		const a = pick(NO_SELECTION, 2, 'only');
		expect(pick(a, 5, 'range').rows).toEqual([2, 3, 4, 5]);
		expect(pick(a, 0, 'range').rows).toEqual([0, 1, 2]);
		// with no anchor a Shift click starts one
		expect(pick(NO_SELECTION, 4, 'range').rows).toEqual([4]);
	});

	it('Cmd or Ctrl adds one row and takes one away', () => {
		let sel = pick(NO_SELECTION, 1, 'only');
		sel = pick(sel, 4, 'toggle');
		sel = pick(sel, 2, 'toggle');
		expect(sel.rows).toEqual([1, 2, 4]);
		sel = pick(sel, 1, 'toggle');
		expect(sel.rows).toEqual([2, 4]);
		// a run after a toggle starts at the row toggled
		expect(pick(sel, 0, 'range').rows).toEqual([0, 1]);
	});

	it('reads the keys of a click', () => {
		expect(pickHow({ shiftKey: true, metaKey: true, ctrlKey: false })).toBe('range');
		expect(pickHow({ shiftKey: false, metaKey: true, ctrlKey: false })).toBe('toggle');
		expect(pickHow({ shiftKey: false, metaKey: false, ctrlKey: true })).toBe('toggle');
		expect(pickHow({ shiftKey: false, metaKey: false, ctrlKey: false })).toBe('only');
	});

	it('Shift and an arrow extend the run a row at a time, within the table', () => {
		let sel = pick(NO_SELECTION, 2, 'only');
		sel = step(sel, 1, true, 5);
		sel = step(sel, 1, true, 5);
		expect(sel.rows).toEqual([2, 3, 4]);
		sel = step(sel, 1, true, 5);
		expect(sel.rows).toEqual([2, 3, 4]);
		sel = step(sel, -3, true, 5);
		expect(sel.rows).toEqual([1, 2]);
		// an arrow alone moves to one row
		expect(step(sel, 1, false, 5).rows).toEqual([2]);
		expect(step(NO_SELECTION, 1, false, 5).rows).toEqual([0]);
		expect(step(NO_SELECTION, -1, false, 5).rows).toEqual([4]);
		expect(step(sel, 1, true, 0)).toEqual(NO_SELECTION);
	});

	it('an edit reaches every selected row only from a selected row of two or more', () => {
		const sel = pick(pick(NO_SELECTION, 1, 'only'), 3, 'range');
		expect(reach(sel, 2)).toEqual([1, 2, 3]);
		expect(reach(sel, 5)).toEqual([5]);
		expect(reach(pick(NO_SELECTION, 2, 'only'), 2)).toEqual([2]);
	});

	it('rows removed leave the selection', () => {
		const sel = pick(pick(NO_SELECTION, 1, 'only'), 4, 'range');
		expect(within(sel, 3)).toEqual({ rows: [1, 2], anchor: 1, focus: null });
	});
});
