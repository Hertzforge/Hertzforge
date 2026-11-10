/**
 * The table file goldens (schema/golden/, `tables.*`) through the shell's codec and client, and
 * the mock analysis service keeping table files by path.
 */
import { readdirSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { ServiceClient } from '$lib/protocol/client.svelte';
import { decodeFrame, encodeFrame } from '$lib/protocol/codec';
import { createAnalysisServer } from '$lib/protocol/mock/analysis';
import { mockSocketFactory } from '$lib/protocol/mock/socket';
import { formOf, loadTable, saveTable, withForm, type LoadedTable, type TableFileClient } from './tableFiles';

const DIR = fileURLToPath(new URL('../../../../schema/golden/', import.meta.url));
const read = (name: string) => readFileSync(`${DIR}${name}.json`, 'utf-8');
const GOLDENS = readdirSync(DIR)
	.filter((f) => /^(request|response)_tables_.*\.json$/.test(f))
	.map((f) => f.replace(/\.json$/, ''))
	.sort();

/** A client that keeps what it was asked and answers from the goldens. */
function recording(result: unknown) {
	const sent: { method: string; params: Record<string, unknown> }[] = [];
	const client: TableFileClient = {
		isOpen: true,
		request: async <R>(method: string, params: Record<string, unknown> = {}) => {
			sent.push({ method, params });
			return result as R;
		}
	};
	return { client, sent };
}

describe('table file goldens', () => {
	it('are the four this suite checks', () => {
		expect(GOLDENS).toEqual(['request_tables_load', 'request_tables_save', 'response_tables_load', 'response_tables_save']);
	});

	for (const name of GOLDENS) {
		it(`${name} decodes and re-encodes as the same compact text frame`, () => {
			const text = read(name);
			const message = decodeFrame(text);
			expect(encodeFrame(message)).toBe(JSON.stringify(JSON.parse(text)));
		});
	}

	it('saveTable sends the golden save and answers its path', async () => {
		const request = JSON.parse(read('request_tables_save'));
		const response = JSON.parse(read('response_tables_save'));
		const { client, sent } = recording(response.result);
		const p = request.params;
		const path = await saveTable(client, 'channels', null, { columns: p.columns, rows: p.rows }, p.path);
		expect(sent).toEqual([{ method: 'tables.save', params: p }]);
		expect(path).toBe(response.result.path);
	});

	it('loadTable sends the golden load and answers the table as the service read it', async () => {
		const request = JSON.parse(read('request_tables_load'));
		const response = JSON.parse(read('response_tables_load'));
		const { client, sent } = recording(response.result);
		const loaded: LoadedTable = await loadTable(client, 'breakpoints', 'random_profile', request.params.path);
		expect(sent).toEqual([{ method: 'tables.load', params: request.params }]);
		expect(loaded).toEqual(response.result);
		expect([loaded.x_unit, loaded.y_unit, loaded.warnings.length]).toEqual(['Hz', 'g²/Hz', 1]);
	});
});

describe('table file paths', () => {
	it('the form follows the suffix', () => {
		expect(['a.hforge', 'b.CSV', 'c.tsv', 'd.txt', 'e.xlsx', 'f.xls', 'g', '/x.y/z'].map(formOf)).toEqual(['hforge', 'csv', 'tsv', 'tsv', 'xlsx', null, null, null]);
		expect(withForm('/tests/channels', 'csv')).toBe('/tests/channels.csv');
		expect(withForm('/tests/channels.hforge', 'xlsx')).toBe('/tests/channels.xlsx');
		expect(withForm('/tests/channels.txt', 'tsv')).toBe('/tests/channels.txt');
		expect(withForm('/tests/v1.2/channels', 'hforge')).toBe('/tests/v1.2/channels.hforge');
	});
});

describe('the mock analysis service keeps table files by path', () => {
	async function client() {
		const c = new ServiceClient({ service: 'analysis', url: 'ws://mock/', socketFactory: mockSocketFactory(createAnalysisServer(), 1) });
		c.connect();
		await c.whenOpen();
		return c;
	}

	it('opens what was saved, and refuses a file of another kind or none', async () => {
		const c = await client();
		const data = { columns: ['channel', 'enabled'], rows: [[1, true]] };
		expect(await saveTable(c, 'channels', null, data, '/tests/channels')).toBe('/tests/channels.hforge');
		const back = await loadTable(c, 'channels', null, '/tests/channels.hforge');
		expect([back.columns, back.rows, back.warnings]).toEqual([data.columns, data.rows, []]);
		await expect(loadTable(c, 'breakpoints', 'random_profile', '/tests/channels.hforge')).rejects.toThrow('holds a channel table, not a breakpoint table');
		await expect(loadTable(c, 'channels', null, '/tests/none.csv')).rejects.toThrow('no file at /tests/none.csv');
		await expect(saveTable(c, 'channels', null, data, '/tests/channels.json')).rejects.toThrow('a table file is');
		c.close();
	});
});
