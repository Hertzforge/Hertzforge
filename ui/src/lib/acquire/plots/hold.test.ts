import { describe, expect, it } from 'vitest';
import type { Series, SpectrumView, View } from '$lib/protocol/types';
import { gridKey, Holder, holdWords, withHolds } from './hold';

const series = (y: number[], x = y.map((_, i) => i + 1), name = 'Control'): Series => ({ name, role: 'measured', x: Float64Array.from(x), y: Float64Array.from(y) });
const ys = (s: Series) => Array.from(s.y);

describe('holds over successive frames', () => {
	it('peak hold keeps the largest value seen at each point', () => {
		const h = new Holder();
		h.configure('peak', 8);
		h.step('c', series([1, 5, 2]));
		const held = h.step('c', series([3, 4, 2.5]));
		expect(ys(held)).toEqual([3, 5, 2.5]);
		expect(held.name).toBe('Control, peak hold');
		expect(held.role).toBe('measured');
	});

	it('minimum hold keeps the smallest', () => {
		const h = new Holder();
		h.configure('min', 8);
		h.step('c', series([1, 5, 2]));
		expect(ys(h.step('c', series([3, 4, 2.5])))).toEqual([1, 4, 2]);
	});

	it('averages the last N frames, and says how many it has', () => {
		const h = new Holder();
		h.configure('average', 2);
		expect(h.step('c', series([2, 4])).name).toBe('Control, average of 1');
		expect(ys(h.step('c', series([4, 8])))).toEqual([3, 6]);
		const third = h.step('c', series([6, 0]));
		// only the last two frames: (4 + 6) / 2 and (8 + 0) / 2
		expect(ys(third)).toEqual([5, 4]);
		expect(third.name).toBe('Control, average of 2');
	});

	it('exponential average moves 1/N of the way to each new frame', () => {
		const h = new Holder();
		h.configure('exponential', 4);
		expect(ys(h.step('c', series([8])))).toEqual([8]);
		expect(ys(h.step('c', series([0])))).toEqual([6]);
		expect(ys(h.step('c', series([0])))).toEqual([4.5]);
		expect(h.step('c', series([0])).name).toBe('Control, exponential average, 4 frames');
	});

	it('starts again when the x grid changes', () => {
		const h = new Holder();
		h.configure('peak', 8);
		h.step('c', series([9, 9], [1, 2]));
		expect(ys(h.step('c', series([1, 1], [1, 3])))).toEqual([1, 1]);
		expect(ys(h.step('c', series([1, 1, 1], [1, 2, 3])))).toEqual([1, 1, 1]);
	});

	it('starts again on Reset and on a change of kind or frames', () => {
		const h = new Holder();
		h.configure('peak', 8, 0);
		h.step('c', series([9]));
		h.configure('peak', 8, 0);
		expect(ys(h.step('c', series([1])))).toEqual([9]);
		h.configure('peak', 8, 1);
		expect(ys(h.step('c', series([1])))).toEqual([1]);
		h.configure('min', 8, 1);
		expect(ys(h.step('c', series([5])))).toEqual([5]);
		h.configure('min', 16, 1);
		expect(ys(h.step('c', series([7])))).toEqual([7]);
	});

	it('leaves out values that are not finite, so a gap does not wipe the hold', () => {
		const h = new Holder();
		h.configure('peak', 8);
		h.step('c', series([3, Number.NaN]));
		expect(ys(h.step('c', series([Number.NaN, 2])))).toEqual([3, 2]);
		const a = new Holder();
		a.configure('average', 4);
		a.step('c', series([2, Number.NaN]));
		expect(ys(a.step('c', series([Number.NaN, Number.NaN])))).toEqual([2, Number.NaN]);
	});

	it('keeps the bins of a stepped series', () => {
		const h = new Holder();
		h.configure('peak', 8);
		const s: Series = { ...series([1, 2]), shape: 'steps', edges: Float64Array.from([0.5, 1.5, 2.5]) };
		const held = h.step('c', s);
		expect(held.shape).toBe('steps');
		expect(Array.from(held.edges!)).toEqual([0.5, 1.5, 2.5]);
	});

	it('fingerprints a grid by its length and values', () => {
		expect(gridKey(Float64Array.from([1, 2, 3]))).not.toBe(gridKey(Float64Array.from([1, 2, 4])));
		expect(gridKey(new Float64Array(0))).toBe('0');
		expect(holdWords('average', 8, 20)).toBe('average of 8');
	});
});

describe('holds on views', () => {
	const view = (control: number[]): SpectrumView => ({
		view: 'spectrum',
		title: 'Control',
		x: { label: 'Frequency', unit: 'Hz', scale: 'log' },
		y: { label: 'PSD', unit: 'g²/Hz', scale: 'log' },
		series: [series(control), { ...series([1, 1]), name: 'Reference', role: 'reference' }]
	});

	it('draws a hold beside each measured series, never beside a specification line', () => {
		const h = new Holder();
		withHolds(h, [view([1, 4])], 'peak', 8);
		const [v] = withHolds(h, [view([2, 3]), { view: 'reading', title: 'Run', rows: [] }], 'peak', 8) as [SpectrumView, View];
		expect(v.series.map((s) => s.name)).toEqual(['Control', 'Reference', 'Control, peak hold']);
		expect(ys(v.series[2])).toEqual([2, 4]);
	});

	it('gives the views back untouched, and forgets every hold, with none', () => {
		const h = new Holder();
		withHolds(h, [view([1, 4])], 'peak', 8);
		expect(h.size).toBe(1);
		const views = [view([1, 2])];
		expect(withHolds(h, views, 'none', 8)).toBe(views);
		expect(h.size).toBe(0);
	});

	it('forgets a series taken off the plot', () => {
		const h = new Holder();
		const two = { ...view([1, 2]), series: [series([1, 2]), series([3, 4], undefined, 'Other')] };
		withHolds(h, [two], 'peak', 8);
		expect(h.size).toBe(2);
		withHolds(h, [view([1, 2])], 'peak', 8);
		expect(h.size).toBe(1);
	});
});
