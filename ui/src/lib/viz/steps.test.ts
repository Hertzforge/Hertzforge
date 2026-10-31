import { describe, expect, it } from 'vitest';
import { binValueAt, stepRuns } from './steps';
import { valueAt } from './align';

const edges = Float64Array.of(1.25, 3.75, 6.25, 8.75, 11.25);
const y = Float64Array.of(1, 2, NaN, 4);

describe('stepped spectra', () => {
	it('draws each value flat across its own bin', () => {
		const runs = stepRuns(edges, y, false, false);
		expect(runs).toEqual([
			[
				[1.25, 1],
				[3.75, 1],
				[3.75, 2],
				[6.25, 2]
			],
			[
				[8.75, 4],
				[11.25, 4]
			]
		]);
	});

	it('breaks at a bin with no value instead of bridging it', () => {
		expect(stepRuns(edges, y, false, false)).toHaveLength(2);
	});

	it('drops bins that cannot sit on a log axis', () => {
		const runs = stepRuns(edges, Float64Array.of(1, 0, 3, 4), false, true);
		expect(runs.map((r) => r.length)).toEqual([2, 4]);
	});

	it('reads the bin under the cursor, flat across it', () => {
		expect(binValueAt(edges, y, 2)).toBe(1);
		expect(binValueAt(edges, y, 3.7)).toBe(1);
		expect(binValueAt(edges, y, 3.8)).toBe(2);
		expect(binValueAt(edges, y, 7)).toBeNull();
		expect(binValueAt(edges, y, 0.5)).toBeNull();
		const series = { name: 'c', role: 'measured' as const, x: Float64Array.of(2.5, 5, 7.5, 10), y, shape: 'steps' as const, edges };
		expect(valueAt(series, 4.9, true, true)).toBe(2);
	});
});
