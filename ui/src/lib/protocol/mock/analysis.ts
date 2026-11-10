/**
 * Mock analysis service: the hello, an empty catalog, and table files kept in memory by path. The
 * analysis runs on a project, which only the real service can open, so `?mock=1` demonstrates the
 * shell's other halves (the control mock) and leaves Analyze waiting for a file. A table saved here
 * opens again from the same path for as long as the page is open.
 */

import type { Catalog } from '../types';
import { MockError, type MockServer } from './socket';

type Cell = number | string | boolean | null;

interface StoredTable {
	kind: string;
	table_kind: string | null;
	columns: string[];
	rows: Cell[][];
	x_unit?: string;
	y_unit?: string;
}

const SUFFIXES = ['.hforge', '.csv', '.tsv', '.txt', '.xlsx'];

/** A path as the service writes it: `.hforge` added when there is no suffix; another suffix refused. */
function tablePath(raw: unknown): string {
	if (typeof raw !== 'string' || !raw.trim()) throw new MockError('bad_request', "Give the file's path on the analysis service's machine.");
	const path = raw.trim();
	const name = path.split(/[\\/]/).pop() ?? path;
	if (!name.includes('.')) return `${path}.hforge`;
	if (!SUFFIXES.some((s) => name.toLowerCase().endsWith(s))) throw new MockError('refused', `${name}: a table file is .hforge, .csv, .tsv, .txt or .xlsx`);
	return path;
}

const kindWords = (kind: string) => (kind === 'channels' ? 'a channel table' : 'a breakpoint table');

export function createAnalysisServer(): MockServer {
	const files = new Map<string, StoredTable>();
	return {
		hello: { type: 'hello', protocol: 'sds/1', service: 'analysis', version: '0.1.0-mock' },
		methods: {
			'catalog.list': (): Catalog => ({ operations: [] }),
			'tables.save': (params) => {
				const path = tablePath(params.path);
				const kind = String(params.kind);
				if (kind !== 'channels' && kind !== 'breakpoints') throw new MockError('bad_request', 'tables.save "kind" is one of channels, breakpoints');
				const columns = params.columns as string[];
				const rows = params.rows as Cell[][];
				if (!Array.isArray(columns) || !Array.isArray(rows) || rows.some((r) => !Array.isArray(r) || r.length !== columns.length)) {
					throw new MockError('bad_request', 'tables.save takes columns and rows of one value per column');
				}
				const stored: StoredTable = { kind, table_kind: (params.table_kind as string | undefined) ?? null, columns: [...columns], rows: structuredClone(rows) };
				if (typeof params.x_unit === 'string') stored.x_unit = params.x_unit;
				if (typeof params.y_unit === 'string') stored.y_unit = params.y_unit;
				files.set(path, stored);
				return { path };
			},
			'tables.load': (params) => {
				const path = tablePath(params.path);
				const found = files.get(path);
				if (!found) throw new MockError('not_found', `no file at ${path}`);
				const name = path.split(/[\\/]/).pop();
				if (found.kind !== params.kind) throw new MockError('refused', `${name} holds ${kindWords(found.kind)}, not ${kindWords(String(params.kind))}`);
				if (params.table_kind && found.table_kind && params.table_kind !== found.table_kind) {
					throw new MockError('refused', `${name} holds a ${found.table_kind.replace(/_/g, ' ')}, not a ${String(params.table_kind).replace(/_/g, ' ')}`);
				}
				return { ...structuredClone(found), warnings: [] };
			}
		}
	};
}
