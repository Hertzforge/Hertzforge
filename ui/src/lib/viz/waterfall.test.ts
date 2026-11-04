import { describe, expect, it } from 'vitest';
import {
	STAGE,
	axisTicks,
	exceedTriangles,
	fraction,
	levelRange,
	linearTicks,
	namedRecords,
	onStage,
	pickedPoint,
	regionMarks,
	recordOf,
	segmentIndex,
	stageMap,
	stationOf,
	surfaceIndex,
	unfraction,
	wallTriangles,
	zoneTriangles
} from './waterfall';

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

	it('spans at most LOG_DECADES on a log axis, below the highest level', () => {
		expect(levelRange([1e-14, 1e-3, 125], true)).toEqual([125e-8, 125]);
		expect(levelRange([1e-14, 125], false)).toEqual([1e-14, 125]);
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

describe('pickedPoint', () => {
	/** A 5 x 5 read-back with point ids (plus one) at the given pixels. */
	function pixels(hits: [number, number, number][]): Uint8Array {
		const out = new Uint8Array(4 * 25);
		for (const [i, j, id] of hits) {
			const k = 4 * (j * 5 + i);
			out[k] = id & 255;
			out[k + 1] = (id >> 8) & 255;
			out[k + 2] = (id >> 16) & 255;
			out[k + 3] = (id >>> 24) & 255;
		}
		return out;
	}

	it('is null when nothing is under the cursor', () => {
		expect(pickedPoint(pixels([]), 5)).toBeNull();
	});

	it('takes the hit nearest the center, less one', () => {
		expect(pickedPoint(pixels([[0, 0, 7], [3, 2, 12], [4, 4, 9]]), 5)).toBe(11);
	});

	it('reads all four bytes, for scenes of millions of points', () => {
		expect(pickedPoint(pixels([[2, 2, 9_876_543 + 1]]), 5)).toBe(9_876_543);
		expect(pickedPoint(pixels([[2, 2, 2 ** 31 + 5]]), 5)).toBe(2 ** 31 + 4);
	});
});

describe('stages', () => {
	const map = stageMap([0, 10], false, [0, 1], false, 3);

	it('stands records at their stations', () => {
		expect(Array.from(stationOf([0, 2, 4, 6], 6, [0, 0, 1]))).toEqual([0, 0, 0, 0, 1, 1]);
		expect(Array.from(stationOf([0, 2, 4], 4))).toEqual([0, 0, 1, 1]);
	});

	it('joins two rows of one clock into a surface and skips a gap', () => {
		const offsets = [0, 3, 6];
		const x = [0, 1, 2, 0, 1, 2];
		expect(Array.from(surfaceIndex(offsets, x, [1, 1, 1, 1, 1, 1], false, false))).toEqual([0, 1, 4, 0, 4, 3, 1, 2, 5, 1, 5, 4]);
		expect(surfaceIndex(offsets, x, [1, NaN, 1, 1, 1, 1], false, false).length).toBe(0);
		expect(surfaceIndex([0, 3, 5], x, [1, 1, 1, 1, 1, 1], false, false).length).toBe(0);
	});

	it('runs a band with no outer edge to the top or the floor', () => {
		const upper = zoneTriangles({ station: 1, side: 'upper', x: [0, 10], inner: [0.5, 0.5] }, map);
		expect(upper).toHaveLength(18);
		const heights = upper.filter((_v, i) => i % 3 === 2);
		expect(Math.max(...heights)).toBeCloseTo(STAGE.z);
		expect(upper[1]).toBeCloseTo(STAGE.y / 2);
		const lower = zoneTriangles({ station: 0, side: 'lower', x: [0, 10], inner: [0.5, 0.5] }, map);
		expect(Math.min(...lower.filter((_v, i) => i % 3 === 2))).toBe(0);
		const between = zoneTriangles({ station: 0, side: 'upper', x: [0, 10], inner: [0.2, 0.2], outer: [0.4, 0.4] }, map);
		expect(Math.max(...between.filter((_v, i) => i % 3 === 2))).toBeCloseTo(0.4 * STAGE.z);
	});

	it('draws a rectangle across each bin past the limit, and skips a gap', () => {
		const hits = exceedTriangles({ station: 2, side: 'upper', x0: [1, 3], x1: [2, 4], z: [0.5, NaN] }, map);
		expect(hits).toHaveLength(18);
		expect(hits[0]).toBeCloseTo(0.1 * STAGE.x);
		expect(hits[3]).toBeCloseTo(0.2 * STAGE.x);
	});

	it('stands a wall through one abscissa a station', () => {
		expect(wallTriangles([1, 2, 3], map)).toHaveLength(36);
		expect(wallTriangles([1, NaN, 3], map)).toHaveLength(0);
	});
});

describe('regions on the stage', () => {
	it('maps a place back to its value, linear or in decades', () => {
		expect(unfraction(0.5, [0, 10], false)).toBeCloseTo(5);
		expect(unfraction(0.5, [1, 100], true)).toBeCloseTo(10);
		expect(unfraction(fraction(42, [1, 1000], true), [1, 1000], true)).toBeCloseTo(42);
	});

	it('meets the floor, or the back wall when the view looks along the floor', () => {
		expect(onStage([0.5, 0.2, 1], [0, 0, -1])).toEqual([0.5, 0.2, 0]);
		const wall = onStage([0.5, -1, 0.2], [0, 1, 0])!;
		expect(wall[1]).toBeCloseTo(STAGE.y);
		expect(onStage([0.5, 0.2, 1], [0, 0, 1])).toBeNull();
	});

	it('shades the floor and back wall between its edges, held on the stage', () => {
		const { faces, edges } = regionMarks(0.2, 0.4);
		expect(faces).toHaveLength(36);
		expect(edges).toHaveLength(24);
		expect(regionMarks(-1, -0.5).faces).toHaveLength(0);
		expect(Math.max(...regionMarks(1, 9).faces.filter((_v, i) => i % 3 === 0))).toBeCloseTo(STAGE.x);
	});
});
