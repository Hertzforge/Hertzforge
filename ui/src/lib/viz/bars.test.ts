import { describe, expect, it } from 'vitest';
import type { BarsView } from '$lib/protocol/types';
import { barSpan, barsAxis, barText, place } from './bars';

function view(over: Partial<BarsView> = {}): BarsView {
	return {
		view: 'bars',
		title: 'RMS Error',
		value: { label: 'RMS error', unit: 'dB' },
		labels: ['101X+', '102X+', '103X+'],
		values: [-4, 0.5, null],
		states: ['under', null, null],
		under: -3,
		over: 3,
		baseline: 0,
		...over
	};
}

describe('the value axis', () => {
	it('spans the bars and both thresholds, padded, and rows with no number have no say', () => {
		const axis = barsAxis(view());
		expect(axis.lo).toBeCloseTo(-4 - 0.84, 9);
		expect(axis.hi).toBeCloseTo(3 + 0.84, 9);
		expect(axis.ticks.every((t) => t >= axis.lo && t <= axis.hi)).toBe(true);
	});

	it('never reaches below the floor', () => {
		const axis = barsAxis(view({ values: [12, 30, 4], states: [null, 'over', null], under: null, over: 20, floor: 0 }));
		expect(axis.lo).toBe(0);
		expect(axis.hi).toBeCloseTo(30 + 3.6, 9);
	});

	it('takes in a baseline the bars grow from', () => {
		const axis = barsAxis(view({ values: [4.2], labels: ['a'], states: ['over'], under: 2.5, over: 3.5, baseline: 3 }));
		expect(axis.lo).toBeLessThan(2.5);
	});
});

describe('the bars', () => {
	it('run from the baseline to the value, either way', () => {
		const axis = { lo: -5, hi: 5, ticks: [], decimals: 0 };
		expect(place(axis, 100, 200, 0)).toBe(200);
		expect(barSpan(axis, 100, 200, 0, -2.5)).toEqual([150, 200]);
		expect(barSpan(axis, 100, 200, 0, 5)).toEqual([200, 300]);
		expect(barSpan(axis, 100, 200, 0, null)).toBeNull();
	});

	it('are said in words, state and note included', () => {
		expect(barText(view(), 0)).toBe('101X+: -4.00 dB, under its limit');
		expect(barText(view({ notes: [null, null, 'not scored: no units'] }), 2)).toBe('103X+: no reading, not scored: no units');
	});
});
