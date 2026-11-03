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
