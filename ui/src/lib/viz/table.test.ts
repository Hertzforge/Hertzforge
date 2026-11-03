import { describe, expect, it } from 'vitest';
import { numeric, tableText } from './table';

describe('table', () => {
	it('reads numbers as numbers and words as words', () => {
		for (const n of ['439.2000', '-0.5', '1e-3', '0.5-0.25j', '+2.1 %', '12']) expect(numeric(n)).toBe(true);
		for (const w of ['101X+', '—', '', 'Mode 1', '2X-']) expect(numeric(w)).toBe(false);
	});

	it('copies as tab-separated text, headings first, a tab inside a cell made a space', () => {
		expect(tableText({ columns: ['Mode', 'Frequency [Hz]'], rows: [['1', '439.2'], ['2', 'a\tb']] })).toBe(
			'Mode\tFrequency [Hz]\n1\t439.2\n2\ta b'
		);
	});
});
