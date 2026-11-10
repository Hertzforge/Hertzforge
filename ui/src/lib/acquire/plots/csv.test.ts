import { describe, expect, it } from 'vitest';
import type { SpectrumView } from '$lib/protocol/types';
import { csvCell, toCsv } from './csv';

const view = (title: string, series: SpectrumView['series']): SpectrumView => ({
	view: 'spectrum',
	title,
	x: { label: 'Frequency', unit: 'Hz', scale: 'log' },
	y: { label: 'PSD', unit: 'g²/Hz', scale: 'log' },
	series
});
const s = (name: string, x: number[], y: number[], role: 'measured' | 'reference' = 'measured') => ({ name, role, x: Float64Array.from(x), y: Float64Array.from(y) });

describe('a plot as CSV', () => {
	it('heads each column with its name and unit, and writes values in full', () => {
		const text = toCsv([view('Control', [s('Control', [20, 40], [0.0123456789, 0.04]), s('Reference', [20, 40], [0.04, 0.04], 'reference')])]);
		expect(text).toBe('Frequency (Hz),Control (g²/Hz),Reference (g²/Hz)\n20,0.0123456789,0.04\n40,0.04,0.04\n');
	});

	it('shares the x column between grids, with blanks where a series has no sample', () => {
		const text = toCsv([view('Control', [s('A', [1, 2], [10, 20]), s('B', [2, 3], [5, Number.NaN])])]);
		expect(text.split('\n')).toEqual(['Frequency (Hz),A (g²/Hz),B (g²/Hz)', '1,10,', '2,20,5', '3,,', '']);
	});

	it('names each column for its view when there are several', () => {
		const text = toCsv([view('Channel 1', [s('Response', [1], [2])]), view('Channel 2', [s('Response', [1], [3])])]);
		expect(text.split('\n')[0]).toBe('Frequency (Hz),Channel 1: Response (g²/Hz),Channel 2: Response (g²/Hz)');
	});

	it('writes a decimated series\' minimum beside it', () => {
		const v = view('Time', [{ ...s('Channel 1', [0, 1], [2, 3]), y_min: Float64Array.from([-2, -3]) }]);
		expect(toCsv([v]).split('\n').slice(0, 2)).toEqual(['Frequency (Hz),Channel 1 (g²/Hz),"Channel 1, minimum (g²/Hz)"', '0,2,-2']);
	});

	it('writes only the series chosen', () => {
		const text = toCsv([view('Control', [s('A', [1], [1]), s('B', [1], [2])])], (_, si) => si === 1);
		expect(text).toBe('Frequency (Hz),B (g²/Hz)\n1,2\n');
	});

	it('quotes a cell holding a comma, a quote or a line break', () => {
		expect(csvCell('plain')).toBe('plain');
		expect(csvCell('a, b')).toBe('"a, b"');
		expect(csvCell('say "hi"')).toBe('"say ""hi"""');
	});

	it('writes a header alone when there is nothing to write', () => {
		expect(toCsv([])).toBe('x\n');
	});
});
