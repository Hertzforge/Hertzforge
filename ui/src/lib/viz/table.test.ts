import { describe, expect, it } from 'vitest';
import { numeric, parseTsv, pasteTargets, tableText } from './table';

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

	it('reads a pasted block, a trailing line break dropped', () => {
		expect(parseTsv('1\t2\r\n3\t4\n')).toEqual([
			['1', '2'],
			['3', '4']
		]);
	});

	it('pastes onto the editable columns only, and not past the last row', () => {
		const keys = ['channel', null, 'node', 'direction'];
		expect(pasteTargets([['7', 'X+'], ['8', 'Y+'], ['9', 'Z+']], 0, 2, keys, 2)).toEqual([
			{ row: 0, key: 'node', value: '7' },
			{ row: 0, key: 'direction', value: 'X+' },
			{ row: 1, key: 'node', value: '8' },
			{ row: 1, key: 'direction', value: 'Y+' }
		]);
		expect(pasteTargets([['a', 'b']], 0, 0, keys, 1)).toEqual([
			{ row: 0, key: 'channel', value: 'a' },
			{ row: 0, key: 'node', value: 'b' }
		]);
	});
});
