import { describe, expect, it } from 'vitest';
import {
	barHeight,
	FOOTPRINT,
	fullHeight,
	homeCamera,
	MARK_LIFT,
	named,
	outlineSegments,
	pairOf,
	pairsText,
	PLINTH,
	togglePick
} from './macBars';

describe('bar heights', () => {
	it('stand a full MAC at 0.4 of the larger side and a zero on a plinth', () => {
		expect(fullHeight(5, 12)).toBeCloseTo(4.8, 9);
		expect(barHeight(1, 5, 12)).toBeCloseTo(4.8, 9);
		expect(barHeight(0, 5, 12)).toBeCloseTo(PLINTH * 4.8, 9);
		expect(barHeight(Number.NaN, 5, 12)).toBeCloseTo(PLINTH * 4.8, 9);
		expect(barHeight(1.3, 5, 12)).toBeCloseTo(4.8, 9);
	});
});

describe('the home view', () => {
	it('looks from the front left at the grid, row 0 nearest', () => {
		const { position, target } = homeCamera(4, 6);
		expect(position[0]).toBeCloseTo(3 - 6.6, 9);
		expect(position[1]).toBeCloseTo(-8.4, 9);
		expect(position[2]).toBeCloseTo(7.2, 9);
		expect(target[0]).toBeCloseTo(3, 9);
		expect(target[1]).toBeCloseTo(1.68, 9);
		expect(target[2]).toBeCloseTo(0.72, 9);
		expect(position[1]).toBeLessThan(0);
	});
});

describe('instances and labels', () => {
	it('lay bars row-major and name every nth mode within the limit', () => {
		expect(pairOf(0, 6)).toEqual({ row: 0, column: 0 });
		expect(pairOf(7, 6)).toEqual({ row: 1, column: 1 });
		expect(named(5)).toEqual([0, 1, 2, 3, 4]);
		expect(named(139).length).toBeLessThanOrEqual(40);
	});
});

describe('outlines', () => {
	it('trace twelve lifted edges per bar', () => {
		const segs = outlineSegments([{ row: 2, column: 1 }], () => 0.5);
		expect(segs.length).toBe(12 * 2 * 3);
		const margin = (1 - FOOTPRINT) / 2;
		// float32 on the wire: five places
		expect(segs[0]).toBeCloseTo(1 + margin - MARK_LIFT, 5);
		expect(segs[1]).toBeCloseTo(2 + margin - MARK_LIFT, 5);
		expect(Math.max(...segs.filter((_v, i) => i % 3 === 2))).toBeCloseTo(
			0.5 + MARK_LIFT,
			5
		);
	});
});

describe('picking squares', () => {
	it('picks alone on a plain click and toggles with the modifier, the last one active', () => {
		const a = { row: 0, column: 0 };
		const b = { row: 1, column: 2 };
		expect(togglePick([a], b, false)).toEqual({ picks: [b], active: b });
		expect(togglePick([a], b, true)).toEqual({ picks: [a, b], active: b });
		expect(togglePick([a, b], b, true)).toEqual({ picks: [a], active: a });
		expect(togglePick([a], a, true)).toEqual({ picks: [], active: null });
		expect(pairsText([a, b])).toBe('1,1; 2,3');
	});
});
