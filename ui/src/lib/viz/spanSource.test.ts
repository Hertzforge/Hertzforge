import { describe, expect, it } from 'vitest';
import type { SpectrumView } from '../protocol/types';
import { showsWhole, spanSeries } from './spanSource';

const axis = { label: 'Time', unit: 's', scale: 'linear' as const };
function view(names: string[], x: number[]): SpectrumView {
	return {
		view: 'spectrum',
		title: 'Force',
		x: axis,
		y: { ...axis, label: 'Force', unit: 'lbf' },
		series: names.map((name) => ({ name, role: 'measured', x: Float64Array.from(x), y: Float64Array.from(x) }))
	};
}

describe('a zoomed plot takes the series of the span asked for', () => {
	it('when they are as many and named alike', () => {
		const shown = view(['1Z+', '2Z+'], [0, 1, 2]);
		const answer = view(['1Z+', '2Z+'], [0, 0.5, 0.6, 2]);
		expect(spanSeries(shown, answer)).toBe(answer.series);
	});

	it('and not when the answer draws something else', () => {
		const shown = view(['1Z+', '2Z+'], [0, 1, 2]);
		expect(spanSeries(shown, view(['1Z+'], [0, 1]))).toBeNull();
		expect(spanSeries(shown, view(['1Z+', '3Z+'], [0, 1]))).toBeNull();
		expect(spanSeries(shown, null)).toBeNull();
	});

	it('goes back to its own curves when it shows every one whole', () => {
		const shown = view(['1Z+'], [0, 1, 2]);
		expect(showsWhole(shown, 0, 2)).toBe(true);
		expect(showsWhole(shown, -1, 3)).toBe(true);
		expect(showsWhole(shown, 0.5, 2)).toBe(false);
		expect(showsWhole(shown, 0, 1.5)).toBe(false);
	});
});
