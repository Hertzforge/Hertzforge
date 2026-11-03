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
