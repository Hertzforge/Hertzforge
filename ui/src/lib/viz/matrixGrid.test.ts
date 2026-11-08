import { describe, expect, it } from 'vitest';
import type { SdmView, SpectrumView } from '$lib/protocol/types';
import {
	cellAtPoint,
	cellKind,
	cellRect,
	curvePoints,
	curveValueAt,
	edgesFromCenters,
	gridCells,
	gridGeometry,
	maxOffset,
	prepareGrid,
	revealControl,
	scrollWindow,
	togglePin,
	windowControls,
	type GridWindowSpec
} from './matrixGrid';

const F = (a: number[]) => Float64Array.from(a);
const labels = Array.from({ length: 72 }, (_, i) => `A${i + 1}`);
const w0: GridWindowSpec = { pinned: [], offset: 0, size: 8 };

/** A packed n by n block on `lines` lines with known elements: G_rr = r + 1, G_rc = 0.5 sqrt(G_rr G_cc) at 90 degrees (r < c). */
function block(n: number, lines: number): SdmView {
	const per = (n * (n + 1)) / 2;
	const re = new Float64Array(lines * per);
	const im = new Float64Array(lines * per);
	for (let l = 0; l < lines; l++) {
		let k = l * per;
		for (let r = 0; r < n; r++) {
			for (let c = r; c < n; c++, k++) {
				if (r === c) re[k] = (r + 1) * (l + 1);
				else im[k] = 0.5 * Math.sqrt((r + 1) * (c + 1)) * (l + 1);
			}
		}
	}
	return {
		view: 'sdm',
		title: 'Spectral Matrix',
		channels: Array.from({ length: n }, (_, i) => labels[i]),
		packing: 'upper',
		unit: 'g²/Hz',
		x: { label: 'Frequency', unit: 'Hz', scale: 'log' },
		x_values: Float64Array.from({ length: lines }, (_, i) => 20 * 10 ** (i / (lines - 1 || 1) * 2)),
		measured: { re, im },
		reference: { re: re.map((v) => v * 1.1), im: im.map((v) => v * 1.1) }
	};
}

describe('the grid window', () => {
	it('shows 8 controls from the scroll offset, and scrolls within the run', () => {
		expect(windowControls(labels, w0)).toEqual([0, 1, 2, 3, 4, 5, 6, 7]);
		expect(maxOffset(labels, w0)).toBe(64);
		const w = scrollWindow(labels, w0, 100);
		expect(w.offset).toBe(64);
		expect(windowControls(labels, w)).toEqual([64, 65, 66, 67, 68, 69, 70, 71]);
		expect(scrollWindow(labels, w, -200).offset).toBe(0);
	});

	it('keeps pinned controls by name, first, while the rest scroll', () => {
		let w = togglePin(w0, 'A40');
		w = togglePin(w, 'A3');
		expect(windowControls(labels, w)).toEqual([39, 2, 0, 1, 3, 4, 5, 6]);
		w = scrollWindow(labels, w, 10);
		expect(windowControls(labels, w).slice(0, 2)).toEqual([39, 2]);
		expect(windowControls(labels, w)).toEqual([39, 2, 11, 12, 13, 14, 15, 16]);
		expect(maxOffset(labels, w)).toBe(70 - 6);
		w = togglePin(w, 'A40');
		expect(w.pinned).toEqual(['A3']);
	});

	it('a full window of pins takes no more, and a name the run does not have is skipped', () => {
		let w: GridWindowSpec = { pinned: [], offset: 0, size: 2 };
		w = togglePin(togglePin(w, 'A9'), 'A10');
		expect(togglePin(w, 'A11')).toBe(w);
		expect(windowControls(labels, w)).toEqual([8, 9]);
		expect(windowControls(labels, { pinned: ['Z1', 'A2'], offset: 0, size: 3 })).toEqual([1, 0, 2]);
	});

	it('fewer controls than the window shows them all', () => {
		expect(windowControls(['X', 'Y', 'Z'], w0)).toEqual([0, 1, 2]);
		expect(maxOffset(['X', 'Y', 'Z'], w0)).toBe(0);
	});

	it('reveals a control chosen elsewhere (the heatmap) by scrolling to it', () => {
		const w = revealControl(labels, w0, 50);
		expect(windowControls(labels, w)[0]).toBe(50);
		expect(revealControl(labels, w0, 3)).toBe(w0);
		const end = revealControl(labels, w0, 71);
		expect(windowControls(labels, end)).toContain(71);
	});
});

describe('what each cell draws', () => {
	it('diagonal spectrum, coherence above, phase below, or cross spectrum both sides', () => {
		expect(cellKind(2, 2, 'coh_phase')).toBe('psd');
		expect(cellKind(1, 3, 'coh_phase')).toBe('coh');
		expect(cellKind(3, 1, 'coh_phase')).toBe('phase');
		expect(cellKind(3, 1, 'csd')).toBe('csd');
		expect(cellKind(1, 3, 'csd')).toBe('csd');
	});

	it('reads coherence, phase and magnitude from the packed block element by element', () => {
		const sdm = block(3, 4);
		const cells = gridCells(3, 'coh_phase', sdm, [null, null, null]);
		const at = (r: number, c: number) => cells.find((x) => x.r === r && x.c === c)!;
		const meas = (r: number, c: number) => at(r, c).curves.find((cv) => cv.role === 'measured')!;
		// coherence 0.25 everywhere above the diagonal
		for (const v of meas(0, 2).y) expect(v).toBeCloseTo(0.25, 12);
		// phase of G_10 = conj(G_01) = -90 degrees below the diagonal
		for (const v of meas(1, 0).y) expect(v).toBeCloseTo(-90, 9);
		// diagonal: G_rr on each line, stepped
		expect([...meas(2, 2).y]).toEqual([3, 6, 9, 12]);
		expect(meas(2, 2).edges!.length).toBe(5);
		const csd = gridCells(3, 'csd', sdm, [null, null, null]);
		const m01 = csd.find((x) => x.r === 0 && x.c === 1)!.curves.find((cv) => cv.role === 'measured')!;
		expect(m01.y[1]).toBeCloseTo(0.5 * Math.sqrt(2) * 2, 12);
		// the reference is drawn too
		expect(at(0, 1).curves.map((c) => c.role)).toEqual(['reference', 'measured']);
	});

	it("a diagonal cell draws the control's own spectrum, with its bands, once it has come", () => {
		const own: SpectrumView = {
			view: 'spectrum',
			title: 'PSD',
			x: { label: 'Frequency', unit: 'Hz', scale: 'log' },
			y: { label: 'PSD', unit: 'g²/Hz', scale: 'log' },
			series: ['measured', 'reference', 'tolerance', 'tolerance', 'abort', 'abort'].map((role, i) => ({
				name: `s${i}`,
				role: role as 'measured',
				x: Float64Array.from([20, 40]),
				y: Float64Array.from([1, 1]),
				shape: 'steps' as const,
				edges: Float64Array.from([15, 30, 50])
			}))
		};
		const cells = gridCells(2, 'coh_phase', block(2, 2), [own, null]);
		expect(cells[0].curves.map((c) => c.role)).toEqual(['measured', 'reference', 'tolerance', 'tolerance', 'abort', 'abort']);
		expect(cells[3].curves.map((c) => c.role)).toEqual(['reference', 'measured']);
	});

	it('edges sit between the centers, geometrically on a log axis', () => {
		const e = edgesFromCenters(Float64Array.from([10, 100, 1000]), true);
		expect(e[1]).toBeCloseTo(Math.sqrt(1000), 9);
		expect(e[0]).toBeCloseTo(10 / Math.sqrt(10), 9);
		expect(e[3]).toBeCloseTo(1000 * Math.sqrt(10), 6);
	});
});

describe('layout and cursor', () => {
	it('square cells fill the space after the gutters; a point maps to its cell', () => {
		const g = gridGeometry(900, 700, 8, { left: 90, top: 24, bottom: 30 });
		expect(g.cell).toBeGreaterThan(70);
		const r = cellRect(g, 2, 5);
		expect(cellAtPoint(g, r.x + 1, r.y + 1)).toEqual({ r: 2, c: 5, fx: 1 / g.cell });
		expect(cellAtPoint(g, r.x + r.w + g.gap / 2, r.y + 2)).toBeNull();
		expect(cellAtPoint(g, 10, 10)).toBeNull();
	});

	it('the cursor reads a stepped curve by bin and a line by nearest point', () => {
		const steps = { role: 'measured' as const, name: 'm', x: F([20, 40]), y: F([1, 2]), edges: F([15, 30, 50]) };
		expect(curveValueAt(steps, 29)).toBe(1);
		expect(curveValueAt(steps, 31)).toBe(2);
		expect(curveValueAt(steps, 60)).toBeNull();
		const line = { role: 'measured' as const, name: 'c', x: F([10, 100, 1000]), y: F([0.1, 0.5, 0.9]) };
		expect(curveValueAt(line, 40)).toBe(0.5);
		expect(curveValueAt(line, 25)).toBe(0.1);
	});

	it('a missing value breaks the polyline', () => {
		const pts = curvePoints({ role: 'measured', name: 'c', x: F([10, 100, 1000]), y: F([0.5, NaN, 0.5]) }, { x: 0, y: 0, w: 100, h: 100 }, { lo: 10, hi: 1000, log: true }, { lo: 0, hi: 1, log: false });
		expect(pts[0]).toBeCloseTo(0, 9);
		expect(Number.isNaN(pts[2])).toBe(true);
		expect(pts[4]).toBeCloseTo(100, 9);
		expect(pts[5]).toBeCloseTo(50, 9);
	});

	it('frame time: an 8 by 8 window with 256 points per cell prepares well inside a 30 frames per second budget', () => {
		const sdm = block(8, 256);
		const g = gridGeometry(1100, 760, 8, { left: 90, top: 24, bottom: 30 });
		prepareGrid(gridCells(8, 'coh_phase', sdm, Array(8).fill(null)), g);
		const runs = 20;
		const t0 = performance.now();
		let paths = 0;
		for (let i = 0; i < runs; i++) {
			const prepared = prepareGrid(gridCells(8, i % 2 ? 'csd' : 'coh_phase', sdm, Array(8).fill(null)), g);
			paths += prepared.reduce((n, p) => n + p.paths.length, 0);
		}
		const ms = (performance.now() - t0) / runs;
		console.info(`grid frame, 8 x 8 x 256: ${ms.toFixed(2)} ms`);
		expect(paths / runs).toBe(128);
		expect(ms).toBeLessThan(33);
	});
});
