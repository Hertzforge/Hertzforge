import { describe, expect, it } from 'vitest';
import { alignSeries, valueAt } from './align';
import { formatFixed, formatNumber, formatTick } from './format';
import { logRange, logTicks } from './logTicks';
import type { Series } from '$lib/protocol/types';

const s = (x: number[], y: number[]): Series => ({ name: 'n', role: 'measured', x: Float64Array.from(x), y: Float64Array.from(y) });

describe('log ticks', () => {
	it('are bounded and finite on degenerate input', () => {
		for (const [a, b] of [
			[0, 10],
			[-5, 5],
			[NaN, NaN],
			[10, 1],
			[1e-300, 1e300]
		]) {
			const t = logTicks(a, b);
			expect(t.length).toBeGreaterThan(0);
			expect(t.length).toBeLessThanOrEqual(120);
			expect(t.every((v) => Number.isFinite(v) && v > 0)).toBe(true);
		}
	});

	it('label a frequency band with 1, 2, 5', () => {
		expect(logTicks(20, 2000, [1, 2, 5])).toEqual([20, 50, 100, 200, 500, 1000, 2000]);
	});

	it('snap ranges to decades', () => {
		expect(logRange(0.0021, 0.16)).toEqual([0.001, 1]);
	});
});

describe('alignment', () => {
	it('merges different x grids and leaves gaps', () => {
		const [x, a, b] = alignSeries([s([1, 2, 3], [10, 20, 30]), s([1, 3], [5, 7])], false, false);
		expect(Array.from(x)).toEqual([1, 2, 3]);
		expect(Array.from(a)).toEqual([10, 20, 30]);
		expect(Array.from(b)).toEqual([5, null, 7]);
	});

	it('drops values a log axis cannot show', () => {
		const [x, a] = alignSeries([s([0, 1, 2], [1, 0, 3])], true, true);
		expect(Array.from(x)).toEqual([1, 2]);
		expect(Array.from(a)).toEqual([null, 3]);
	});

	it('reads values along the drawn segment, in log-log space when the axes are log', () => {
		const spec = s([20, 80], [0.01, 0.04]);
		expect(valueAt(spec, 40, true, true)).toBeCloseTo(0.02, 10);
		expect(valueAt(spec, 50, false, false)).toBeCloseTo(0.025, 10);
		expect(valueAt(spec, 10, true, true)).toBeNull();
	});
});

describe('formatting', () => {
	it('keeps readouts compact and stable', () => {
		expect(formatNumber(0.041234)).toBe('0.04123');
		expect(formatNumber(1250)).toBe('1250');
		expect(formatFixed(6.1, 4)).toBe('6.100');
		expect(formatTick(1000)).toBe('1000');
		expect(formatTick(20000)).toBe('20,000');
		expect(formatTick(0.003)).toBe('0.003');
		expect(formatTick(1e-7)).toBe('1e-7');
	});
});
