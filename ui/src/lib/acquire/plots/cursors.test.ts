import { describe, expect, it } from 'vitest';
import type { SpectrumView } from '$lib/protocol/types';
import { difference, harmonics, paneMarks, peak, placeCursor, separationFor, topPeaks } from './cursors';

const xy = (x: number[], y: number[]) => ({ x: Float64Array.from(x), y: Float64Array.from(y) });

describe('the peak', () => {
	it('is the largest finite value, within a span when one is given', () => {
		const s = xy([1, 2, 3, 4, 5], [1, 9, Number.NaN, 4, 7]);
		expect(peak(s)).toEqual({ x: 2, y: 9, index: 1 });
		expect(peak(s, 3, 5)).toEqual({ x: 5, y: 7, index: 4 });
		expect(peak(xy([1], [Number.NaN]))).toBeNull();
	});
});

describe('the top peaks', () => {
	const s = xy([1, 2, 3, 4, 5, 6, 7, 8, 9], [0, 5, 0, 3, 3.5, 0, 8, 0, 1]);

	it('are local maxima, highest first', () => {
		expect(topPeaks(s, 3, 0, false).map((m) => m.x)).toEqual([7, 2, 5]);
	});

	it('counts an end against its one neighbour', () => {
		expect(topPeaks(s, 9, 0, false).map((m) => m.x)).toEqual([7, 2, 5, 9]);
	});

	it('keeps the higher of two peaks closer than the separation', () => {
		// 5 (at x 2) and 3.5 (at x 5) are 3 apart; with a separation of 4 only one of them stays
		expect(topPeaks(s, 3, 4, false).map((m) => m.x)).toEqual([7, 2]);
	});

	it('measures the separation in decades on a log axis', () => {
		const log = xy([10, 20, 30, 100, 200, 1000], [0, 5, 0, 4, 0, 0]);
		// 20 and 100 are 0.7 decades apart
		expect(topPeaks(log, 2, 0.5, true).map((m) => m.x)).toEqual([20, 100]);
		expect(topPeaks(log, 2, 0.8, true).map((m) => m.x)).toEqual([20]);
	});

	it('takes a plateau once, at its last point', () => {
		expect(topPeaks(xy([1, 2, 3, 4], [0, 2, 2, 0]), 5, 0, false).map((m) => m.x)).toEqual([3]);
	});

	it('scales a default separation to the span', () => {
		expect(separationFor(0, 100, false)).toBeCloseTo(2);
		expect(separationFor(10, 1000, true)).toBeCloseTo(0.04);
		expect(separationFor(0, 1000, true)).toBe(0);
	});
});

describe('harmonics', () => {
	it('are the fundamental and its whole multiples, n in all, up to the end of the axis', () => {
		expect(harmonics(100, 4)).toEqual([100, 200, 300, 400]);
		expect(harmonics(100, 10, 350)).toEqual([100, 200, 300]);
		expect(harmonics(0, 3)).toEqual([]);
		expect(harmonics(Number.NaN, 3)).toEqual([]);
	});
});

describe("two cursors' difference", () => {
	it('is a change on a linear axis and a ratio on a log axis', () => {
		expect(difference(10, 30, 1, 4, false)).toEqual({ dx: 20, dy: 3, ratio: null });
		expect(difference(10, 30, 2, 8, true)).toEqual({ dx: 20, dy: null, ratio: 4 });
		expect(difference(10, 30, null, 8, false)).toEqual({ dx: 20, dy: null, ratio: null });
	});
});

describe('placing cursors', () => {
	it('moves a single cursor to the click', () => {
		expect(placeCursor([5, null], 9, 'one', false)).toEqual([9, null]);
		expect(placeCursor([5, 7], 9, 'harmonics', false)).toEqual([9, 7]);
	});

	it('places A, then B, then moves the nearer', () => {
		let at = placeCursor([null, null], 10, 'two', false);
		expect(at).toEqual([10, null]);
		at = placeCursor(at, 50, 'two', false);
		expect(at).toEqual([10, 50]);
		expect(placeCursor(at, 45, 'two', false)).toEqual([10, 45]);
		expect(placeCursor(at, 20, 'two', false)).toEqual([20, 50]);
	});

	it('judges nearness in decades on a log axis, and ignores a click it cannot place', () => {
		// 40 is 0.6 decades from 10 and 0.4 from 100
		expect(placeCursor([10, 100], 40, 'two', true)).toEqual([10, 40]);
		expect(placeCursor([10, 100], -1, 'two', true)).toEqual([10, 100]);
	});
});

describe("a pane's marks", () => {
	const view: SpectrumView = {
		view: 'spectrum',
		title: 'Control',
		x: { label: 'Frequency', unit: 'Hz', scale: 'linear' },
		y: { label: 'PSD', unit: 'g²/Hz', scale: 'linear' },
		series: [
			{ name: 'Reference', role: 'reference', x: Float64Array.from([0, 10, 20, 30]), y: Float64Array.from([1, 1, 1, 1]) },
			{ name: 'Control', role: 'measured', x: Float64Array.from([0, 10, 20, 30]), y: Float64Array.from([0, 4, 2, 6]) }
		]
	};

	it('draws nothing with no cursor', () => {
		expect(paneMarks(view, { cursor: 'none', cursorAt: [5, 6], count: 3 })).toEqual({ lines: [], points: [], box: [] });
	});

	it('reads every shown series at two cursors, and their difference', () => {
		const m = paneMarks(view, { cursor: 'two', cursorAt: [10, 30], count: 3 }, (i) => i === 1);
		expect(m.lines.map((l) => [l.x, l.label])).toEqual([
			[10, 'A'],
			[30, 'B']
		]);
		expect(m.box).toEqual(['A  10.00 Hz', '   Control  4.000 g²/Hz', 'B  30.00 Hz', '   Control  6.000 g²/Hz', 'B-A  20.00 Hz', '   Control  2.000 g²/Hz']);
	});

	it('marks the peak of each measured series, and the top peaks of the first', () => {
		expect(paneMarks(view, { cursor: 'peak', cursorAt: [null, null], count: 3 }).points).toEqual([{ x: 30, y: 6, label: 'Peak' }]);
		expect(paneMarks(view, { cursor: 'peaks', cursorAt: [null, null], count: 2 }).points).toEqual([
			{ x: 30, y: 6, label: '1' },
			{ x: 10, y: 4, label: '2' }
		]);
	});

	it('hangs harmonics from cursor A, within the axis', () => {
		const m = paneMarks(view, { cursor: 'harmonics', cursorAt: [10, null], count: 5 });
		expect(m.lines.map((l) => l.label)).toEqual(['f', '2f', '3f']);
		expect(paneMarks(view, { cursor: 'harmonics', cursorAt: [null, null], count: 5 }).lines).toEqual([]);
	});
});
