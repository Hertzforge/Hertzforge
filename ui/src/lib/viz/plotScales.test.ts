import { describe, expect, it } from 'vitest';
import { chooseScale, scaleChoice, scaleKey } from './plotScales.svelte';

describe('the scales chosen on a plot', () => {
	it('are kept by what is plotted, so a PSD says nothing of a time history', () => {
		const psd = scaleKey('psd', 'Hz');
		const history = scaleKey('time_history', 's');
		expect(scaleChoice(psd)).toEqual({});
		chooseScale(psd, 'x', false);
		chooseScale(psd, 'y', true);
		expect(scaleChoice(psd)).toEqual({ x: false, y: true });
		expect(scaleChoice(history)).toEqual({});
	});

	it('are kept apart for one kind on two axes units', () => {
		chooseScale(scaleKey('sine_sweep_specification', 's'), 'x', false);
		expect(scaleChoice(scaleKey('sine_sweep_specification', 'Hz'))).toEqual({});
		expect(scaleKey(undefined, 'Hz')).toBe('plot|Hz');
	});
});
