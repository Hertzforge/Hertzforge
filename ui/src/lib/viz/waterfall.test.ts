import { describe, expect, it } from 'vitest';
import { axisTicks, fraction, levelRange, linearTicks, namedRecords, recordOf, segmentIndex } from './waterfall';

describe('segmentIndex', () => {
	const offsets = [0, 4, 8];
	const x = [10, 20, 30, 40, 10, 20, 30, 40];

	it('joins consecutive points within a record and never across records', () => {
		const z = [1, 2, 3, 4, 5, 6, 7, 8];
		expect(Array.from(segmentIndex(offsets, x, z, false, false))).toEqual([0, 1, 1, 2, 2, 3, 4, 5, 5, 6, 6, 7]);
	});

	it('breaks the line at a NaN gap', () => {
		const z = [1, 2, NaN, 4, 5, 6, 7, 8];
		expect(Array.from(segmentIndex(offsets, x, z, false, false))).toEqual([0, 1, 4, 5, 5, 6, 6, 7]);
	});

	it('breaks a log axis at a level at or below zero, and keeps it on a linear one', () => {
		const z = [1, 0, 3, -1, 5, 6, 7, 8];
		expect(Array.from(segmentIndex(offsets, x, z, false, true))).toEqual([4, 5, 5, 6, 6, 7]);
		expect(segmentIndex(offsets, x, z, false, false).length).toBe(12);
	});

	it('breaks a log abscissa at zero frequency', () => {
		const xs = [0, 20, 30, 40, 0, 20, 30, 40];
		const z = [1, 2, 3, 4, 5, 6, 7, 8];
		expect(Array.from(segmentIndex(offsets, xs, z, true, false))).toEqual([1, 2, 2, 3, 5, 6, 6, 7]);
	});

	it('draws nothing for a record of one point', () => {
		expect(segmentIndex([0, 1], [1], [1], false, false).length).toBe(0);
	});
});

describe('recordOf', () => {
	it('gives each point its record number', () => {
		expect(Array.from(recordOf([0, 2, 5], 5))).toEqual([0, 0, 1, 1, 1]);
	});
});

describe('levelRange', () => {
	it('spans the drawable values, skipping gaps and, on a log axis, non-positive levels', () => {
		expect(levelRange([3, NaN, -2, 7], false)).toEqual([-2, 7]);
		expect(levelRange([3, NaN, -2, 0, 7], true)).toEqual([3, 7]);
	});

	it('takes the pinned range on a linear axis only', () => {
		expect(levelRange([0.9, 0.95], false, [0, 1.05])).toEqual([0, 1.05]);
		expect(levelRange([0.9, 0.95], true, [0, 1.05])).toEqual([0.9, 0.95]);
	});

	it('widens a flat record so it still stands on the stage', () => {
		const [lo, hi] = levelRange([2, 2], true);
		expect(lo).toBeLessThan(2);
		expect(hi).toBeGreaterThan(2);
		expect(levelRange([NaN], false)).toEqual([0, 1]);
	});
});

describe('fraction', () => {
	it('is linear or in decades', () => {
		expect(fraction(5, [0, 10], false)).toBe(0.5);
		expect(fraction(10, [1, 100], true)).toBeCloseTo(0.5, 12);
	});
});

describe('ticks', () => {
	it('lands linear ticks on round values inside the span', () => {
		expect(linearTicks(0, 2048)).toEqual([0, 500, 1000, 1500, 2000]);
		expect(linearTicks(0, 0.5)).toEqual([0, 0.1, 0.2, 0.3, 0.4, 0.5]);
	});

	it('uses decades on a log axis', () => {
		expect(axisTicks([1e-6, 1e-2], true)).toEqual([1e-6, 1e-5, 1e-4, 1e-3, 1e-2]);
	});
});

describe('namedRecords', () => {
	it('names every record up to the limit, then every nth and the last', () => {
		expect(namedRecords(5, 40)).toEqual([0, 1, 2, 3, 4]);
		const named = namedRecords(900, 40);
		expect(named.length).toBeLessThanOrEqual(41);
		expect(named[0]).toBe(0);
		expect(named[named.length - 1]).toBe(899);
	});
});
