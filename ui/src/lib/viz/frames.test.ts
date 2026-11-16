import { describe, expect, it } from 'vitest';
import { BAND_PEAK, glyphBox, slotHeight, wallGlyph, windowBands, windowGradient, windowPath } from './frames';

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
