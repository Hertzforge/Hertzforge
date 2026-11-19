import { describe, expect, it } from 'vitest';
import { abortRuns, cellAt, columnOf, labelStride, rasterize, rowOrder, rowSummary, summaries, type HeatData } from './heatmap';
import { lightness, type RowBand } from './scales';

const band: RowBand = { alarm_db: [3, 3], abort_db: [6, 6] };

function synthetic(rows: number, cols: number, seed = 1): HeatData {
	let s = seed;
	const rand = () => ((s = (s * 1664525 + 1013904223) >>> 0) / 2 ** 32) * 2 - 1;
	const values = new Float32Array(rows * cols);
	for (let r = 0; r < rows; r++) for (let c = 0; c < cols; c++) values[r * cols + c] = rand() * (1 + (r % 9));
	return { rows, cols, values, bands: Array.from({ length: rows }, () => band) };
}

describe('the heatmap raster', () => {
	it('colors each cell by its row band and leaves a missing value transparent', () => {
		const d: HeatData = { rows: 2, cols: 3, values: Float32Array.from([0, 4, 8, NaN, -4, -8]), bands: [band, band] };
		const out = new Uint8ClampedArray(2 * 3 * 4);
		rasterize(d, 'error', 'light', [0, 1], out);
		const L = (i: number) => lightness(out.subarray(i * 4, i * 4 + 3));
		expect(L(0)).toBeGreaterThan(L(1));
		expect(L(1)).toBeGreaterThan(L(2));
		expect(out[3 * 4 + 3]).toBe(0);
		expect(out[0 + 3]).toBe(255);
		expect(Math.abs(L(4) - L(1))).toBeLessThan(3);
	});

	it('draws rows in the display order asked for', () => {
		const d: HeatData = { rows: 2, cols: 1, values: Float32Array.from([0, 9]), bands: [band, band] };
		const a = new Uint8ClampedArray(8);
		const b = new Uint8ClampedArray(8);
		rasterize(d, 'error', 'light', [0, 1], a);
		rasterize(d, 'error', 'light', [1, 0], b);
		expect([...b.subarray(0, 4)]).toEqual([...a.subarray(4, 8)]);
	});

	it('summarizes a row: rms, worst, cells beyond alarm and abort', () => {
		const d: HeatData = { rows: 1, cols: 5, values: Float32Array.from([1, -2, 4, -7, NaN]), bands: [band] };
		const s = rowSummary(d, 0, 'error');
		expect(s.rms).toBeCloseTo(Math.sqrt((1 + 4 + 16 + 49) / 4), 6);
		expect(s.worst).toBe(-7);
		expect(s.alarm).toBe(2);
		expect(s.abort).toBe(1);
		expect(abortRuns({ rows: 1, cols: 6, values: Float32Array.from([7, 7, 1, -9, -9, -9]), bands: [band] }, 0)).toEqual([
			[0, 2],
			[3, 6]
		]);
	});

	it('orders rows worst first on request, ties in the setup order', () => {
		const d: HeatData = { rows: 4, cols: 1, values: Float32Array.from([1, 5, 5, 2]), bands: [] };
		expect([...rowOrder(4, 'worst', summaries(d, 'error'))]).toEqual([1, 2, 3, 0]);
		expect([...rowOrder(4, 'setup')]).toEqual([0, 1, 2, 3]);
	});

	it('finds the cell under the cursor and the column of a frequency', () => {
		const l = { x: 100, y: 20, w: 720, h: 360, rows: 72, cols: 720 };
		expect(cellAt(100, 20, l)).toEqual({ row: 0, col: 0 });
		expect(cellAt(819.9, 379.9, l)).toEqual({ row: 71, col: 719 });
		expect(cellAt(99, 30, l)).toBeNull();
		expect(cellAt(500, 381, l)).toBeNull();
		const x = Float64Array.from({ length: 101 }, (_, i) => 10 * 10 ** (i / 50));
		expect(columnOf(100, x)).toBeCloseTo(50, 9);
		expect(columnOf(5, x)).toBeNull();
	});

	it('spaces row labels so they never overlap', () => {
		expect(labelStride(14)).toBe(1);
		expect(labelStride(6)).toBe(2);
		expect(labelStride(4)).toBe(3);
	});

	it('frame time: 72 controls by 1,024 columns rasterize and summarize well inside a 30 frames per second budget', () => {
		const d = synthetic(72, 1024);
		const out = new Uint8ClampedArray(72 * 1024 * 4);
		const order = rowOrder(72, 'worst', summaries(d, 'error'));
		// warm up
		rasterize(d, 'error', 'dark', order, out);
		const runs = 20;
		const t0 = performance.now();
		for (let i = 0; i < runs; i++) {
			const sums = summaries(d, 'error');
			rasterize(d, 'error', 'dark', rowOrder(72, 'worst', sums), out);
			for (let r = 0; r < 72; r++) abortRuns(d, r);
		}
		const ms = (performance.now() - t0) / runs;
		console.info(`heatmap frame, 72 x 1024: ${ms.toFixed(2)} ms`);
		expect(ms).toBeLessThan(33);
	});
});
