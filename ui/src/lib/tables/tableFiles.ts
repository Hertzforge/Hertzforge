/**
 * Table files by path, through the analysis service (`tables.save`, `tables.load`): a channel
 * table or a breakpoint table written to, or read from, a `.hforge`, comma-separated, tab-delimited
 * or Excel file where it is. In the desktop app the paths come from the native dialogs; in a
 * browser, which never gives a file's path, the person types one.
 */

import { chooseOpenFile, chooseSaveFile, type DialogFilter } from '$lib/project/files';

export type TableKind = 'channels' | 'breakpoints';
export type TableForm = 'hforge' | 'csv' | 'tsv' | 'xlsx';
/** One cell as the service carries it: a number, text, true or false, or null for none. */
export type TableCell = number | string | boolean | null;

export interface TableFormat {
	form: TableForm;
	/** The words a menu says. */
	label: string;
	/** The suffix a new file is given. */
	suffix: string;
	/** Every suffix the form is read from. */
	extensions: string[];
}

export const TABLE_FORMATS: TableFormat[] = [
	{ form: 'hforge', label: 'Hertzforge Table', suffix: '.hforge', extensions: ['hforge'] },
	{ form: 'csv', label: 'CSV', suffix: '.csv', extensions: ['csv'] },
	{ form: 'tsv', label: 'Tab-Delimited Text', suffix: '.tsv', extensions: ['tsv', 'txt'] },
	{ form: 'xlsx', label: 'Excel Workbook', suffix: '.xlsx', extensions: ['xlsx'] }
];

/** What a table writes: its columns by name and its rows; a breakpoint table's units. */
export interface TableData {
	columns: string[];
	rows: TableCell[][];
	x_unit?: string;
	y_unit?: string;
}

/** A table read from a file, with what the read left out or added. */
export interface LoadedTable extends TableData {
	kind: TableKind;
	table_kind: string | null;
	warnings: string[];
}

/** What a file act did, for the table to show beside it. */
export interface FileResult {
	level: 'error' | 'warning' | 'note';
	message: string;
}

/** What the files go through: the analysis service's client, or anything that answers as it does. */
export interface TableFileClient {
	readonly isOpen: boolean;
	request<R = Record<string, unknown>>(method: string, params?: Record<string, unknown>, options?: { timeoutMs?: number }): Promise<R>;
}

/** Long enough for a workbook of several hundred channels on a slow disk. */
const FILE_TIMEOUT_MS = 60_000;

/** The form a path's suffix names, or null for a suffix no table file has. */
export function formOf(path: string): TableForm | null {
	const dot = path.lastIndexOf('.');
	const slash = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'));
	if (dot <= slash) return null;
	const suffix = path.slice(dot + 1).toLowerCase();
	return TABLE_FORMATS.find((f) => f.extensions.includes(suffix))?.form ?? null;
}

/** `path` in form `form`: its suffix added when it has none, or put in place of another table form's. */
export function withForm(path: string, form: TableForm): string {
	const format = TABLE_FORMATS.find((f) => f.form === form)!;
	const now = formOf(path);
	if (now === form) return path;
	if (now === null) return path + format.suffix;
	return path.slice(0, path.lastIndexOf('.')) + format.suffix;
}

/** A name as a file's name: the characters a path gives a meaning to replaced. */
export const fileStem = (name: string) => name.replace(/[\\/:*?"<>|]/g, '-').trim() || 'Table';

/** The last part of a path, for messages. */
export const pathName = (path: string) => path.split(/[\\/]/).filter(Boolean).pop() ?? path;

/** Write a table to `path`, in the form its suffix names; the path written. */
export async function saveTable(client: TableFileClient, kind: TableKind, tableKind: string | null, data: TableData, path: string): Promise<string> {
	const params: Record<string, unknown> = { kind, path, columns: data.columns, rows: data.rows };
	if (tableKind) params.table_kind = tableKind;
	if (data.x_unit !== undefined) params.x_unit = data.x_unit;
	if (data.y_unit !== undefined) params.y_unit = data.y_unit;
	const res = await client.request<{ path: string }>('tables.save', params, { timeoutMs: FILE_TIMEOUT_MS });
	return res.path;
}

/** Read the table at `path`; a file of another kind is refused by the service in words. */
export async function loadTable(client: TableFileClient, kind: TableKind, tableKind: string | null, path: string): Promise<LoadedTable> {
	const params: Record<string, unknown> = { kind, path };
	if (tableKind) params.table_kind = tableKind;
	return client.request<LoadedTable>('tables.load', params, { timeoutMs: FILE_TIMEOUT_MS });
}

const filterOf = (f: TableFormat): DialogFilter => ({ name: f.label, extensions: f.extensions });

/** A path to write the table to, from the native save dialog; null when cancelled or in a browser. */
export function chooseTableSave(title: string, name: string, form: TableForm | null): Promise<string | null> {
	const formats = form ? TABLE_FORMATS.filter((f) => f.form === form) : TABLE_FORMATS;
	return chooseSaveFile(title, `${fileStem(name)}${formats[0].suffix}`, formats.map(filterOf));
}

/** A table file to open, from the native open dialog; null when cancelled or in a browser. */
export function chooseTableOpen(title: string): Promise<string | null> {
	return chooseOpenFile(title, [{ name: 'Table files', extensions: TABLE_FORMATS.flatMap((f) => f.extensions) }]);
}
