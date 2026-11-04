import type { TableView } from '$lib/protocol/types';

/** Whether a cell reads as a number ('439.2', '-0.5', '1e-3', '0.5-0.25j', '+2.1 %'). */
export function numeric(cell: string): boolean {
	return /^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?([+-](\d+\.?\d*|\.\d+)([eE][+-]?\d+)?j)?( ?%)?$/.test(cell.trim());
}

/** The table as tab-separated text, headings first: what pastes into a spreadsheet. */
export function tableText(view: Pick<TableView, 'columns' | 'rows'>): string {
	const clean = (cell: string) => cell.replace(/[\t\r\n]+/g, ' ');
	return [view.columns, ...view.rows].map((row) => row.map(clean).join('\t')).join('\n');
}

/** The table as HTML, headings in a `thead`: what a document or an email pastes as a table. */
export function tableHtml(view: Pick<TableView, 'columns' | 'rows'>): string {
	const escape = (cell: string) => cell.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
	const cells = (tag: string, row: string[]) => row.map((c) => `<${tag}>${escape(c)}</${tag}>`).join('');
	const head = `<thead><tr>${cells('th', view.columns)}</tr></thead>`;
	const body = `<tbody>${view.rows.map((r) => `<tr>${cells('td', r)}</tr>`).join('')}</tbody>`;
	return `<table>${head}${body}</table>`;
}

/** Whether a flag cell reads as set: the table writes True and False, a file may say yes or 1. */
export function flagOn(cell: string): boolean {
	return ['true', 'yes', '1', 'on', 'x'].includes(cell.trim().toLowerCase());
}

/** Tab-separated text as rows of cells, a trailing line break dropped: what a spreadsheet copies. */
export function parseTsv(text: string): string[][] {
	const lines = text.replace(/\r\n?/g, '\n').replace(/\n$/, '').split('\n');
	return lines.map((line) => line.split('\t'));
}

/**
 * The cells a pasted block sets, starting at (row, column): each pasted column lands on the next
 * editable column, read-only columns skipped; rows past the table's end and columns past its
 * editable ones are left out.
 */
export function pasteTargets(
	block: string[][],
	row: number,
	column: number,
	keys: (string | null)[],
	rowCount: number
): { row: number; key: string; value: string }[] {
	const editable = keys.map((k, i) => (k ? i : -1)).filter((i) => i >= column);
	const out: { row: number; key: string; value: string }[] = [];
	block.forEach((cells, r) => {
		if (row + r >= rowCount) return;
		cells.forEach((value, c) => {
			const at = editable[c];
			if (at !== undefined) out.push({ row: row + r, key: keys[at] as string, value });
		});
	});
	return out;
}

/** A rectangle of cells, inclusive, rows and columns from 0. */
export interface Block {
	r0: number;
	c0: number;
	r1: number;
	c1: number;
}

/** The block two corners span, whichever way round. */
export function spanned(a: { row: number; col: number }, b: { row: number; col: number }): Block {
	return { r0: Math.min(a.row, b.row), c0: Math.min(a.col, b.col), r1: Math.max(a.row, b.row), c1: Math.max(a.col, b.col) };
}

export function inside(block: Block | null, row: number, col: number): boolean {
	return !!block && row >= block.r0 && row <= block.r1 && col >= block.c0 && col <= block.c1;
}

/** The selected block as tab-separated text, what Copy puts on the clipboard. */
export function blockText(rows: string[][], block: Block): string {
	return rows
		.slice(block.r0, block.r1 + 1)
		.map((row) => row.slice(block.c0, block.c1 + 1).join('\t'))
		.join('\n');
}

/**
 * The cells a fill handle dragged from `block` to (row, col) sets: the rectangle the block and that
 * cell span, less the block itself, each cell taking the block's value at the same place counted
 * cyclically, so two alternating rows carry the alternation, as a spreadsheet fills. Read-only
 * columns are left out.
 */
export function fillTargets(
	rows: string[][],
	block: Block,
	row: number,
	col: number,
	keys: (string | null)[]
): { row: number; key: string; value: string }[] {
	const out: { row: number; key: string; value: string }[] = [];
	const r0 = Math.min(block.r0, row);
	const r1 = Math.max(block.r1, row);
	const c0 = Math.min(block.c0, col);
	const c1 = Math.max(block.c1, col);
	const height = block.r1 - block.r0 + 1;
	const width = block.c1 - block.c0 + 1;
	const mod = (n: number, m: number) => ((n % m) + m) % m;
	for (let r = r0; r <= r1; r++) {
		for (let c = c0; c <= c1; c++) {
			if (inside(block, r, c) || !keys[c]) continue;
			const value = rows[block.r0 + mod(r - block.r0, height)]?.[block.c0 + mod(c - block.c0, width)] ?? '';
			out.push({ row: r, key: keys[c] as string, value });
		}
	}
	return out;
}
