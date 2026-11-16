import { describe, expect, it } from 'vitest';
import { BAND_PEAK, draggedSpan, framesText, glyphBox, previewFrames, railShare, slotHeight, wallGlyph, windowBands, windowGradient, windowPath } from './frames';

describe('averaging frames on a plot', () => {
	it('gives each row a slot, thinner once the rows outgrow the rail', () => {
		expect(slotHeight(1)).toBeCloseTo(0.09);
		expect(slotHeight(2)).toBeCloseTo(0.09);
		expect(slotHeight(4)).toBeCloseTo(0.085);
		const first = glyphBox(0, 2);
		const second = glyphBox(1, 2);
		expect(second.top - first.top).toBeCloseTo(0.09);
		expect(first.top + first.height).toBeLessThanOrEqual(second.top + 1e-9);
	});

	it('shades a frame by its window: nothing at the ends of a Hann, the most in the middle', () => {
		const gradient = windowGradient([0, 0.5, 1, 0.5, 0]);
		expect(gradient).toContain('0.0%, transparent) 0.0%');
		expect(gradient).toContain('25.0%, transparent) 50.0%');
		expect(gradient).toContain('12.5%, transparent) 75.0%');
	});

	it('draws the window as a closed shape on its baseline', () => {
		expect(windowPath([0, 1, 0])).toBe('M0,100 L0.00,100.00 L50.00,0.00 L100.00,100.00 L100,100 Z');
	});

	it('puts the window on the back wall under its top, one segment between each pair of weights', () => {
		const { faces, edges } = wallGlyph([0, 1, 0], 0, 1, 0.4, 0.8, 0.9, 0.5);
		expect(faces.length).toBe(2 * 18);
		// two segments along the window and the baseline
		expect(edges.length).toBe(3 * 6);
		const box = glyphBox(0, 1);
		const peak = Math.max(...edges.filter((_, i) => i % 3 === 2));
		expect(peak).toBeCloseTo(0.5 * (1 - box.top));
		expect(edges.filter((_, i) => i % 3 === 1).every((y) => y === 0.9)).toBe(true);
		expect(wallGlyph([1], 0, 1, 0, 1, 0.9, 0.5).faces).toEqual([]);
	});

	it('shades the stage by the window: each strip of the floor and the wall as dark as the window is there', () => {
		const { faces, alphas } = windowBands([0, 1, 0], 0.4, 0.8, 0.9, 0.5);
		// two strips, each a quad on the floor and a quad on the wall
		expect(faces.length).toBe(2 * 2 * 18);
		expect(alphas.length).toBe(faces.length / 3);
		expect(Math.max(...alphas)).toBeCloseTo(BAND_PEAK);
		expect(Math.min(...alphas)).toBe(0);
		// every corner at the middle of the frame carries the peak, every corner at its ends none
		alphas.forEach((alpha, v) => expect(alpha).toBeCloseTo(Math.abs(faces[3 * v] - 0.6) < 1e-9 ? BAND_PEAK : 0));
		expect(windowBands([1], 0, 1, 0.9, 0.5).faces).toEqual([]);
	});
});

describe('a span of frames being dragged', () => {
	// frames of 2 s, a second apart, on a record from 0 to 30 s; four of them from 10 to 15 s
	const snap = { frame: 2, hop: 1, low: 0, high: 30 };
	const was = { x0: 10, x1: 15 };

	it('slides the same frames, and stops at the ends of the record', () => {
		expect(draggedSpan(snap, 'move', was, 12.3, 17.3)).toEqual({ x0: 12.3, x1: 17.3, count: 4 });
		expect(draggedSpan(snap, 'move', was, 28, 33)).toEqual({ x0: 25, x1: 30, count: 4 });
		expect(draggedSpan(snap, 'move', was, -3, 2)).toEqual({ x0: 0, x1: 5, count: 4 });
	});

	it('adds and removes whole frames at the end dragged, the other end staying', () => {
		expect(draggedSpan(snap, 'high', was, 10, 17.6)).toEqual({ x0: 10, x1: 17, count: 6 });
		expect(draggedSpan(snap, 'high', was, 10, 10.4)).toEqual({ x0: 10, x1: 12, count: 1 });
		expect(draggedSpan(snap, 'high', was, 10, 99)).toEqual({ x0: 10, x1: 30, count: 19 });
		expect(draggedSpan(snap, 'low', was, 7.4, 15)).toEqual({ x0: 8, x1: 15, count: 6 });
		expect(draggedSpan(snap, 'low', was, 14.5, 15)).toEqual({ x0: 13, x1: 15, count: 1 });
		expect(draggedSpan(snap, 'low', was, -50, 15)).toEqual({ x0: 0, x1: 15, count: 14 });
	});

	it('draws the frames a drag would leave with the window and the rows the others have', () => {
		const drawn = [{ x0: 10, x1: 12, kind: 'frame' as const, window: [0, 1, 0], level: 0, rows: 2 }];
		const frames = previewFrames(drawn, snap, { x0: 8, count: 5 });
		expect(frames.map((f) => [f.x0, f.x1, f.level])).toEqual([[8, 10, 0], [9, 11, 1], [10, 12, 0], [11, 13, 1], [12, 14, 0]]);
		expect(frames.every((f) => f.rows === 2 && f.window === drawn[0].window)).toBe(true);
		// with nothing to copy a window from, or too many to see, one shaded span
		expect(previewFrames([], snap, { x0: 8, count: 5 })).toEqual([{ x0: 8, x1: 14, kind: 'frame' }]);
		expect(previewFrames(drawn, snap, { x0: 0, count: 5000 }).length).toBe(1);
		expect(framesText(1)).toBe('1 frame');
		expect(framesText(23)).toBe('23 frames');
	});

	it('leaves the rail the room its rows take, and none with no windows', () => {
		expect(railShare([{ x0: 0, x1: 1, kind: 'span' }])).toBe(0);
		expect(railShare([{ x0: 0, x1: 1, kind: 'frame', window: [1, 1], level: 0, rows: 1 }])).toBeCloseTo(0.14);
		expect(railShare([{ x0: 0, x1: 1, kind: 'frame', window: [1, 1], level: 0, rows: 4 }])).toBeCloseTo(0.39);
	});
});
