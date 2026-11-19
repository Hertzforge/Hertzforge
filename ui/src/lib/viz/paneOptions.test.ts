import { describe, expect, it } from 'vitest';
import { markerIndices, minorSplits, panBy, zoomAbout } from './paneOptions';
import { DASH_STYLES, ROLE_STYLES, TRACE_COLORS, traceColor } from './roles';

describe('minor grid lines', () => {
	it('puts every whole mantissa of each decade on a log axis, leaving the major lines out', () => {
		const minors = minorSplits(10, 1000, true, [10, 100, 1000]);
		expect(minors).toEqual([20, 30, 40, 50, 60, 70, 80, 90, 200, 300, 400, 500, 600, 700, 800, 900]);
	});

	it('divides each major step in five on a linear axis', () => {
		expect(minorSplits(0, 2, false, [0, 1, 2])).toEqual([0.2, 0.4, 0.6, 0.8, 1.2, 1.4, 1.6, 1.8]);
	});

	it('extends below the first major line and stays inside the axis', () => {
		expect(minorSplits(-0.5, 1, false, [0, 1])).toEqual([-0.4, -0.2, 0.2, 0.4, 0.6, 0.8]);
	});

	it('gives nothing on a degenerate or non-positive log axis', () => {
		expect(minorSplits(1, 1, true, [1])).toEqual([]);
		expect(minorSplits(0, 10, true, [1, 10])).toEqual([]);
		expect(minorSplits(Number.NaN, 10, false, [0, 10])).toEqual([]);
		expect(minorSplits(0, 10, false, [5])).toEqual([]);
	});
});

describe('wheel zoom and pan', () => {
	it('zooms about the pointer, keeping the value under it in place', () => {
		const [lo, hi] = zoomAbout(0, 10, 2.5, 0.5, false);
		expect(lo).toBeCloseTo(1.25);
		expect(hi).toBeCloseTo(6.25);
		expect((2.5 - lo) / (hi - lo)).toBeCloseTo(0.25);
	});

	it('zooms evenly in decades on a log axis', () => {
		const [lo, hi] = zoomAbout(10, 1000, 100, 0.5, true);
		expect(lo).toBeCloseTo(10 ** 1.5);
		expect(hi).toBeCloseTo(10 ** 2.5);
	});

	it('pans by a fraction of the width', () => {
		expect(panBy(0, 10, 0.1, false)).toEqual([1, 11]);
		const [lo, hi] = panBy(10, 100, -1, true);
		expect(lo).toBeCloseTo(1);
		expect(hi).toBeCloseTo(10);
	});

	it('leaves a range it cannot move alone', () => {
		expect(zoomAbout(5, 5, 5, 0.5, false)).toEqual([5, 5]);
		expect(zoomAbout(0, 10, 1, 0.5, true)).toEqual([0, 10]);
		expect(panBy(0, 10, Number.NaN, false)).toEqual([0, 10]);
	});
});

describe('marker places', () => {
	it('keeps markers a gap apart however dense the samples are', () => {
		const values = Array.from({ length: 101 }, (_, i) => i);
		expect(markerIndices(values, (i) => i * 2, 50)).toEqual([0, 25, 50, 75, 100]);
	});

	it('starts an offset in, so two lines drawn together interleave', () => {
		const values = Array.from({ length: 101 }, (_, i) => i);
		expect(markerIndices(values, (i) => i * 2, 50, 20)).toEqual([10, 35, 60, 85]);
	});

	it('skips missing values', () => {
		expect(markerIndices([null, 1, Number.NaN, 2, 3], (i) => i * 100, 50)).toEqual([1, 3, 4]);
	});
});

describe('the colours a line may be given', () => {
	it('are named, distinct and matched without regard to case', () => {
		const hexes = TRACE_COLORS.map((c) => c.hex);
		expect(new Set(hexes).size).toBe(hexes.length);
		expect(TRACE_COLORS.every((c) => /^#[0-9a-f]{6}$/.test(c.hex) && c.words.length > 0)).toBe(true);
		expect(traceColor('#E69F00')?.words).toBe('Orange');
		expect(traceColor('#123456')).toBeUndefined();
	});

	it('draws the dashes the roles draw', () => {
		expect(DASH_STYLES.dashed.dash).toEqual(ROLE_STYLES.reference.dash);
		expect(DASH_STYLES.dotted.dash).toEqual(ROLE_STYLES.tolerance.dash);
		expect(DASH_STYLES.dashdot.dash).toEqual(ROLE_STYLES.abort.dash);
	});
});
