import { describe, expect, it } from 'vitest';
import { flagOn, tableHtml } from './table';
import { blockText, fillTargets, numeric, parseTsv, pasteTargets, spanned, tableText } from './table';

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

	it('copies the selected block, whichever corner was clicked first', () => {
		const rows = [
			['1', 'a', 'x'],
			['2', 'b', 'y'],
			['3', 'c', 'z']
		];
		expect(blockText(rows, spanned({ row: 2, col: 2 }, { row: 1, col: 1 }))).toBe('b\ty\nc\tz');
	});

	it('fills down by repeating the block, alternation kept, read-only columns left alone', () => {
		const rows = [['A', 'p'], ['B', 'q'], ['', ''], ['', ''], ['', '']];
		const keys = ['name', null];
		const block = spanned({ row: 0, col: 0 }, { row: 1, col: 1 });
		expect(fillTargets(rows, block, 4, 1, keys)).toEqual([
			{ row: 2, key: 'name', value: 'A' },
			{ row: 3, key: 'name', value: 'B' },
			{ row: 4, key: 'name', value: 'A' }
		]);
	});

	it('fills up and across as well', () => {
		const rows = [['', '', ''], ['7', '', '']];
		const keys = ['a', 'b', 'c'];
		const block = spanned({ row: 1, col: 0 }, { row: 1, col: 0 });
		expect(fillTargets(rows, block, 0, 2, keys)).toEqual([
			{ row: 0, key: 'a', value: '7' },
			{ row: 0, key: 'b', value: '7' },
			{ row: 0, key: 'c', value: '7' },
			{ row: 1, key: 'b', value: '7' },
			{ row: 1, key: 'c', value: '7' }
		]);
	});
});

describe('tableHtml', () => {
	it('puts the headings in a thead and escapes the cells', () => {
		const html = tableHtml({ columns: ['DOF', 'Note'], rows: [['1Z+', 'a < b & c']] });
		expect(html).toBe('<table><thead><tr><th>DOF</th><th>Note</th></tr></thead><tbody><tr><td>1Z+</td><td>a &lt; b &amp; c</td></tr></tbody></table>');
	});
});

describe('flagOn', () => {
	it('reads the table\'s True and a file\'s yes or 1 as set', () => {
		expect(['True', 'true', 'yes', '1'].every(flagOn)).toBe(true);
		expect(['False', '', 'no', '0'].some(flagOn)).toBe(false);
	});
});
